use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity, Task};
use music::{Genre, GenreDetail, GenreSection};
use serde::Deserialize;
use tokio::task::AbortHandle;

use crate::{Io, Session, SessionEvent, join};

/// NetEase throttles anonymous list reads after a burst; give the quota time to refill.
const RETRY_AFTER: Duration = Duration::from_secs(20);
/// How many throttled retry rounds the first fill gets.
const RETRY_ROUNDS: usize = 2;
/// How many of those pictures are asked for at once.
const AT_ONCE: usize = 4;
/// How long the pictures found for a provider's genres are reused before they are looked up again.
const KEEP_COVERS: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How many cards one more-page asks for.
const GENRE_PAGE: usize = 20;

/// The pictures found for a provider's genres by genre id, with `None` for a genre known to have
/// none.
type Covers = HashMap<String, Option<String>>;

/// Serializes cache merges so two rows finishing together do not replace each other's covers.
static SAVE: Mutex<()> = Mutex::new(());

/// The genres the search page browses. Nothing is fetched until a screen calls `load`, and a
/// sign-in fetches them again only after that.
pub struct Genres {
    genres: Rc<Vec<Genre>>,
    /// Whether a screen has asked for the genres, so a sign-in knows to fetch them.
    wanted: bool,
    loading: bool,
    error: Option<String>,
    session: Entity<Session>,
    io: Io,
    cache: storage::Cache,
    task: Option<Task<()>>,
    /// Loads the pictures remembered for this provider.
    dressing: Option<Task<()>>,
    /// The pictures already found, including ones whose genre tile has not drawn yet.
    covers: Covers,
    /// Genre ids with a cover lookup already running, so rows that redraw do not ask twice.
    requested: HashSet<String>,
}

/// The pictures a provider's genres were last dressed with, as `cache.sqlite` keeps them.
#[derive(Deserialize)]
struct Stored {
    saved: u64,
    covers: Covers,
}

impl Genres {
    pub fn new(session: Entity<Session>, io: Io, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&session, |this, _, event, cx| match event {
            SessionEvent::SignedIn if this.wanted => this.load(cx),
            SessionEvent::SignedIn => {}
            SessionEvent::SignedOut => {
                this.task = None;
                this.dressing = None;
                this.covers.clear();
                this.requested.clear();
                this.genres = Rc::new(Vec::new());
                this.loading = false;
                this.error = None;
                cx.notify();
            }
            SessionEvent::Reconnected | SessionEvent::WentOffline | SessionEvent::LocalChanged => {}
        })
        .detach();

        Self {
            genres: Rc::new(Vec::new()),
            wanted: false,
            loading: false,
            error: None,
            session,
            io,
            cache: storage::Cache::standard(),
            task: None,
            dressing: None,
            covers: Covers::new(),
            requested: HashSet::new(),
        }
    }

    pub fn genres(&self) -> Rc<Vec<Genre>> {
        self.genres.clone()
    }

    pub fn adopt(&mut self, id: &str, cover: Option<String>, cx: &mut Context<Self>) {
        let Some(cover) = cover else {
            return;
        };
        let Some(genre) = Rc::make_mut(&mut self.genres)
            .iter_mut()
            .find(|genre| genre.id == id && genre.cover.is_none())
        else {
            return;
        };

        self.covers.insert(id.to_owned(), Some(cover.clone()));
        genre.cover = Some(cover);
        cx.notify();
    }

    pub fn forget(&mut self, id: &str, cx: &mut Context<Self>) {
        let kept = self.genres.len();
        Rc::make_mut(&mut self.genres).retain(|genre| genre.id != id);
        if self.genres.len() != kept {
            cx.notify();
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Fetches the genres unless they are loaded or loading. Before a sign-in it only marks them
    /// wanted, and the sign-in fetches them.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.wanted = true;
        if self.loading || !self.genres.is_empty() {
            return;
        }
        let Some(client) = self.session.read(cx).client() else {
            return;
        };

        self.loading = true;
        self.error = None;
        cx.notify();

        let io = self.io.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let lister = client.clone();
            let loaded = join(io.spawn(async move { lister.genres().await })).await;

            this.update(cx, |this, cx| {
                this.loading = false;
                match crate::settled(loaded, cx) {
                    Ok(genres) => {
                        this.genres = Rc::new(genres);
                        this.covers.clear();
                        this.requested.clear();
                        this.dress(cx);
                    }
                    Err(reason) => this.error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Loads the pictures remembered for this provider. Missing ones are fetched only for rows
    /// the search page actually draws, so the first screen never waits behind the whole catalogue.
    fn dress(&mut self, cx: &mut Context<Self>) {
        let Some(slug) = self.session.read(cx).provider_slug() else {
            return;
        };
        let cache = self.cache.clone();
        let io = self.io.clone();
        self.dressing = Some(cx.spawn(async move |this, cx| {
            let stored = io
                .spawn_blocking(move || remembered(&cache, slug))
                .await
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                for (id, cover) in stored {
                    if let Some(cover) = cover {
                        this.adopt(&id, Some(cover), cx);
                    }
                }
                this.dressing = None;
            })
            .ok();
        }));
    }

    /// Fetches the covers of the genres whose rows are showing, and asks again only for ids
    /// still bare. A throttled batch waits for the provider's quota and retries; whatever was
    /// found is written after each batch so a later visit opens with it.
    pub fn ensure_covers(&mut self, ids: impl IntoIterator<Item = String>, cx: &mut Context<Self>) {
        let Some(client) = self.session.read(cx).client() else {
            return;
        };
        let missing: HashSet<String> = self
            .genres
            .iter()
            .filter(|genre| genre.cover.is_none())
            .map(|genre| genre.id.clone())
            .collect();
        let mut fresh = Vec::new();
        for id in ids {
            if missing.contains(&id) && self.requested.insert(id.clone()) {
                fresh.push(id);
            }
        }
        if fresh.is_empty() {
            return;
        }

        let slug = self.session.read(cx).provider_slug();
        let cache = self.cache.clone();
        let io = self.io.clone();
        cx.spawn(async move |this, cx| {
            let mut pending = fresh;
            for round in 0..=RETRY_ROUNDS {
                if pending.is_empty() {
                    break;
                }
                if round > 0 {
                    cx.background_executor().timer(RETRY_AFTER).await;
                }
                let mut failed = Vec::new();
                for batch in pending.chunks(AT_ONCE) {
                    let asked = batch.iter().map(|id| {
                        let client = client.clone();
                        let id = id.clone();
                        join(io.spawn(async move { client.genre_cover(&id).await }))
                    });
                    let found = futures::future::join_all(asked).await;
                    let kept = this.update(cx, |this, cx| {
                        for (id, cover) in batch.iter().zip(found) {
                            match cover {
                                Ok(cover) => {
                                    let has_cover = cover.is_some();
                                    this.adopt(id, cover.clone(), cx);
                                    this.covers.insert(id.clone(), cover);
                                    if has_cover {
                                        this.requested.remove(id);
                                    }
                                }
                                Err(error) => {
                                    log::debug!("genres: no picture for {id}: {error:#}");
                                    failed.push(id.clone());
                                }
                            }
                        }
                    });
                    if kept.is_err() {
                        return;
                    }
                    let snapshot = this.update(cx, |this, _| this.covers.clone());
                    if let Ok(found) = snapshot {
                        let cache = cache.clone();
                        io.spawn_blocking(move || {
                            if let Some(slug) = slug {
                                remember_more(&cache, slug, &found);
                            }
                        })
                        .await
                        .ok();
                    }
                }
                pending = failed;
            }
            if !pending.is_empty() {
                // Let a row that redraws later try once more; this session has given up on it.
                this.update(cx, |this, _| {
                    for id in &pending {
                        this.requested.remove(id);
                    }
                })
                .ok();
            }
        })
        .detach();
    }
}

pub struct GenreDetails {
    id: Option<String>,
    detail: Option<Arc<GenreDetail>>,
    sections: Rc<Vec<GenreSection>>,
    /// A home shelf shown in the genre page, not a provider genre; refreshes and loads stay away
    /// from the network until a real genre is opened.
    synthetic: bool,
    loading: bool,
    error: Option<String>,
    /// The page offset the provider wants next, and how many cards it counted in total.
    next_offset: Option<usize>,
    total: Option<usize>,
    loading_more: bool,
    more_error: Option<String>,
    more_task: Option<Task<()>>,
    session: Entity<Session>,
    genres: Entity<Genres>,
    io: Io,
    task: Option<Task<()>>,
    request: Option<AbortHandle>,
}

impl GenreDetails {
    pub fn new(
        session: Entity<Session>,
        genres: Entity<Genres>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&session, |this, _, event, cx| match event {
            SessionEvent::SignedIn => {
                if let Some(id) = this.id.clone() {
                    this.clear();
                    this.open(&id, cx);
                }
            }
            SessionEvent::SignedOut => {
                this.clear();
                cx.notify();
            }
            SessionEvent::Reconnected | SessionEvent::WentOffline | SessionEvent::LocalChanged => {}
        })
        .detach();

        Self {
            id: None,
            detail: None,
            sections: Rc::new(Vec::new()),
            synthetic: false,
            loading: false,
            error: None,
            next_offset: None,
            total: None,
            loading_more: false,
            more_error: None,
            more_task: None,
            session,
            genres,
            io,
            task: None,
            request: None,
        }
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn name(&self) -> Option<&str> {
        self.detail.as_ref().map(|detail| detail.name.as_str())
    }

    pub fn sections(&self) -> Rc<Vec<GenreSection>> {
        self.sections.clone()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether another page can be asked for.
    pub fn has_more(&self) -> bool {
        !self.synthetic && self.next_offset.is_some()
    }

    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    pub fn more_error(&self) -> Option<&str> {
        self.more_error.as_deref()
    }

    pub fn open(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.synthetic
            && self.id.as_deref() == Some(id)
            && (self.loading || self.detail.is_some())
        {
            return;
        }

        self.clear();
        self.id = Some(id.to_owned());
        let Some(catalog) = self.session.read(cx).catalog(id) else {
            cx.notify();
            return;
        };
        if let Some(detail) = catalog.peek_genre(id) {
            self.adopt(id, detail, cx);
            cx.notify();
            return;
        }
        self.load(id.to_owned(), false, cx);
    }

    /// Drops the page and fetches it again from the provider, past the catalog cache, so it
    /// shows its loading state until the new sections land.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.synthetic {
            return;
        }
        let Some(id) = self.id.clone() else {
            return;
        };
        self.clear();
        self.id = Some(id.clone());
        self.load(id, true, cx);
    }

    /// Shows a home shelf in this page. The cards come from the home feed, not from the
    /// provider's genre catalogue, so nothing is fetched and a refresh leaves it alone.
    pub fn show_home_section(
        &mut self,
        title: &str,
        sections: Vec<GenreSection>,
        cx: &mut Context<Self>,
    ) {
        self.clear();
        self.synthetic = true;
        self.id = Some(format!("home-section:{title}"));
        self.detail = Some(Arc::new(GenreDetail {
            name: title.to_owned(),
            sections: sections.clone(),
            next_offset: None,
            total: None,
        }));
        self.sections = Rc::new(sections);
        cx.notify();
    }

    /// Asks for the next page and appends it to the section it belongs to. A failed page
    /// leaves what is on screen and waits for the reader to try again.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.synthetic || self.loading_more {
            return;
        }
        let Some(next) = self.next_offset else {
            return;
        };
        let Some(id) = self.id.clone() else {
            return;
        };
        let Some(catalog) = self.session.read(cx).catalog(&id) else {
            return;
        };
        self.loading_more = true;
        self.more_error = None;
        cx.notify();

        let io = self.io.clone();
        self.more_task = Some(cx.spawn(async move |this, cx| {
            let page_id = id.clone();
            let loaded = join(io.spawn(async move {
                catalog.genre_page(&page_id, next, GENRE_PAGE).await
            }))
            .await;
            this.update(cx, |this, cx| {
                if this.id.as_deref() != Some(id.as_str()) || this.synthetic {
                    return;
                }
                this.loading_more = false;
                this.more_task = None;
                match crate::settled(loaded, cx) {
                    Ok(page) => this.append(page, cx),
                    Err(reason) => this.more_error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn append(&mut self, page: GenreDetail, cx: &mut Context<Self>) {
        let mut sections = (*self.sections).clone();
        for mut incoming in page.sections {
            match sections.iter_mut().find(|section| section.title == incoming.title) {
                Some(existing) => existing.items.append(&mut incoming.items),
                None => sections.push(incoming),
            }
        }
        self.sections = Rc::new(sections);
        self.next_offset = page.next_offset;
        self.total = page.total;
        cx.notify();
    }

    /// Fetches the genre's sections. A `fresh` load drops the cached genre first, so the
    /// provider is asked again.
    fn load(&mut self, id: String, fresh: bool, cx: &mut Context<Self>) {
        let Some(catalog) = self.session.read(cx).catalog(&id) else {
            cx.notify();
            return;
        };

        self.loading = true;
        cx.notify();

        let request = self.io.spawn({
            let id = id.clone();
            async move {
                if fresh {
                    catalog.invalidate_genre(&id).await;
                }
                catalog.genre(&id).await
            }
        });
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;

            this.update(cx, |this, cx| {
                if this.id.as_deref() != Some(id.as_str()) {
                    return;
                }

                this.loading = false;
                this.request = None;
                match crate::settled(loaded, cx) {
                    Ok(detail) => this.adopt(&id, detail, cx),
                    Err(reason) => this.error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn adopt(&mut self, id: &str, detail: Arc<GenreDetail>, cx: &mut Context<Self>) {
        let cover = detail.cover();
        self.genres
            .update(cx, |genres, cx| match detail.sections.is_empty() {
                true => genres.forget(id, cx),
                false => genres.adopt(id, cover, cx),
            });
        self.sections = Rc::new(detail.sections.clone());
        self.next_offset = detail.next_offset;
        self.total = detail.total;
        self.detail = Some(detail);
    }

    fn clear(&mut self) {
        self.task = None;
        if let Some(request) = self.request.take() {
            request.abort();
        }
        self.synthetic = false;
        self.id = None;
        self.detail = None;
        self.sections = Rc::new(Vec::new());
        self.loading = false;
        self.error = None;
        self.next_offset = None;
        self.total = None;
        self.loading_more = false;
        self.more_error = None;
        self.more_task = None;
    }
}

/// The genre pictures stored for the provider `slug`, or none when they are missing, unreadable
/// or older than `KEEP_COVERS`. It blocks on sqlite.
fn remembered(cache: &storage::Cache, slug: &str) -> Covers {
    let stored = cache.read(&covers_key(slug), |text| {
        serde_json::from_str::<Stored>(text).ok()
    });
    match stored {
        Ok(Some(Some(stored))) if now().saturating_sub(stored.saved) < KEEP_COVERS.as_secs() => {
            stored
                .covers
                .into_iter()
                .filter(|(_, cover)| cover.is_some())
                .collect()
        }
        Ok(_) => Covers::new(),
        Err(error) => {
            log::warn!("genres: {error:#}");
            Covers::new()
        }
    }
}

/// Stores the genre pictures found for the provider `slug`, replacing what was kept before. It
/// blocks on sqlite.
/// Merges newly found covers with what the provider already has stored, serializing writers so
/// two rows finishing together cannot replace each other's pictures.
fn remember_more(cache: &storage::Cache, slug: &str, covers: &Covers) {
    let _guard = SAVE.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut all = remembered(cache, slug);
    for (id, cover) in covers {
        if cover.is_some() {
            all.insert(id.clone(), cover.clone());
        }
    }
    remember(cache, slug, &all);
}

fn remember(cache: &storage::Cache, slug: &str, covers: &Covers) {
    let saved = now();
    let found: Covers = covers
        .iter()
        .filter(|(_, cover)| cover.is_some())
        .map(|(id, cover)| (id.clone(), cover.clone()))
        .collect();
    let value = serde_json::json!({ "saved": saved, "covers": found }).to_string();
    if let Err(error) = cache.write(&covers_key(slug), &value, saved as i64) {
        log::warn!("genres: {error:#}");
    }
}

fn covers_key(slug: &str) -> String {
    format!("genre-covers/{slug}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
