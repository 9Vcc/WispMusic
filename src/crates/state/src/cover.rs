use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Context, Entity, Task};
use music::{CoverSize, MusicApi};

use crate::{Io, Playback, Session, SessionEvent, join};

/// The artwork of one album: the large cover, and the full-resolution one when the provider
/// serves something bigger.
#[derive(Clone)]
struct Art {
    large: String,
    full: Option<String>,
}

pub struct Cover {
    session: Entity<Session>,
    playback: Entity<Playback>,
    album: Option<String>,
    /// The shown track's own cover, which can differ from its album's cover.
    thumbnail: Option<String>,
    large: Option<String>,
    full: Option<String>,
    cache: HashMap<String, Art>,
    io: Io,
    task: Option<Task<()>>,
    /// Deriving the provider's own sizes of the shown thumbnail, off the drawing thread.
    derive_task: Option<Task<()>>,
}

impl Cover {
    pub fn new(
        session: Entity<Session>,
        playback: Entity<Playback>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&playback, |this, _, cx| this.follow(cx))
            .detach();
        cx.subscribe(&session, |this, _, event, cx| match event {
            SessionEvent::SignedOut => this.forget(cx),
            SessionEvent::SignedIn
            | SessionEvent::Reconnected
            | SessionEvent::WentOffline
            | SessionEvent::LocalChanged => {}
        })
        .detach();

        Self {
            session,
            playback,
            album: None,
            thumbnail: None,
            large: None,
            full: None,
            cache: HashMap::new(),
            io,
            task: None,
            derive_task: None,
        }
    }

    pub fn large(&self) -> Option<&str> {
        self.large.as_deref()
    }

    /// Returns the large artwork only when it belongs to `album`.
    pub fn large_for(&self, album: &str) -> Option<&str> {
        self.large
            .as_deref()
            .filter(|_| self.album.as_deref() == Some(album))
    }

    /// The full-resolution artwork for a cover that fills the screen, only when it belongs to
    /// `album` and the provider serves something bigger than the large one.
    pub fn full_for(&self, album: &str) -> Option<&str> {
        self.full
            .as_deref()
            .filter(|_| self.album.as_deref() == Some(album))
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.derive_task = None;
        self.album = None;
        self.thumbnail = None;
        self.large = None;
        self.full = None;
        self.cache.clear();
        cx.notify();
    }

    /// Follows the shown track's album. The cached album art shows at once; the provider's
    /// own sizes of the track's thumbnail are derived on the blocking pool and win over it
    /// when they land. Deriving on the drawing thread is not an option: a plugin provider
    /// answers `cover_at` behind the same lock as its network calls, so a track change would
    /// wait there for a request already in flight.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let (album, thumbnail) = match self.playback.read(cx).shown_track(cx) {
            Some(track) => (track.album_id, track.cover),
            None => (None, None),
        };
        if album == self.album && thumbnail == self.thumbnail {
            return;
        }
        if album != self.album {
            self.task = None;
        }
        self.album = album.clone();
        self.thumbnail = thumbnail;
        let cached = album.as_ref().and_then(|id| self.cache.get(id)).cloned();
        self.large = cached.as_ref().map(|art| art.large.clone());
        self.full = cached.and_then(|art| art.full);
        self.derive(cx);
        cx.notify();

        let Some(id) = album else {
            return;
        };
        // The album is loaded only when neither the track's own cover nor the cache serves a
        // large one, and not while the track's cover is still being derived.
        if self.derive_task.is_some() || self.large.is_some() || self.task.is_some() {
            return;
        }
        self.load(id, cx);
    }

    /// Derives the provider's own sizes of `thumbnail` on the blocking pool, replacing the
    /// cached album art when they land. One derivation runs at a time: a cover that arrives
    /// while it runs is picked up when it finishes instead of queueing another.
    fn derive(&mut self, cx: &mut Context<Self>) {
        if self.derive_task.is_some() {
            return;
        }
        let (Some(thumbnail), Some(client)) = (
            self.thumbnail.clone(),
            self.album.as_deref().and_then(|id| self.client(id, cx)),
        ) else {
            return;
        };
        let io = self.io.clone();
        let wanted = thumbnail.clone();
        self.derive_task = Some(cx.spawn(async move |this, cx| {
            let derived = io
                .spawn_blocking(move || {
                    (
                        client.cover_at(&thumbnail, CoverSize::Large),
                        client.cover_at(&thumbnail, CoverSize::Full),
                    )
                })
                .await
                .ok();
            this.update(cx, |this, cx| {
                this.derive_task = None;
                if this.thumbnail.as_deref() != Some(wanted.as_str()) {
                    // A newer cover arrived while this one was deriving.
                    this.derive(cx);
                    return;
                }
                let (large, full) = derived.unwrap_or_default();
                if large.is_some() {
                    this.large = large;
                }
                if full.is_some() {
                    this.full = full;
                }
                let Some(id) = this.album.clone() else {
                    return;
                };
                if this.large.is_none() && this.task.is_none() {
                    this.load(id, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn load(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(client) = self.client(&id, cx) else {
            return;
        };

        let io = self.io.clone();
        let wanted = id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let found = join(io.spawn(async move {
                let large = client.album(&wanted).await?.album.cover_large;
                let full = large
                    .as_deref()
                    .and_then(|cover| client.cover_at(cover, CoverSize::Full));
                anyhow::Ok(large.map(|large| Art { large, full }))
            }))
            .await;

            this.update(cx, |this, cx| {
                this.task = None;
                match found {
                    Ok(art) => {
                        let Some(art) = art else {
                            return;
                        };
                        this.cache.insert(id.clone(), art.clone());
                        if this.album.as_deref() == Some(id.as_str()) && this.large.is_none() {
                            this.large = Some(art.large);
                            let kept = this.full.take();
                            this.full = art.full.or(kept);
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        log::warn!("cover: cannot load {id}: {error:#}");
                        crate::noted(&error, cx);
                    }
                }
            })
            .ok();
        }));
    }

    /// The client that serves album `id`: the local library for a local id, the signed-in
    /// provider otherwise.
    fn client(&self, id: &str, cx: &Context<Self>) -> Option<Arc<dyn MusicApi>> {
        let session = self.session.read(cx);
        match music::is_local_id(id) {
            true => session.local_client(),
            false => session.client(),
        }
    }
}
