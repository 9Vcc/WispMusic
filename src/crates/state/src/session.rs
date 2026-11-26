use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use gpui::{Context, Entity, EventEmitter, Task};
use i18n::t;
use music::link::Linked;
use music::{
    Capabilities, Catalog, CheckinReport, Library, MusicApi, MusicProvider, PlaybackFactory,
    PromptSink, ProviderSession, Shape, SignIn, SignInFailure, SignInProblem, SignInPrompt, Track,
    UserProfile,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::catalog::CatalogSource;
use crate::settings::AppSettings;
use crate::{Io, Network, Outcome, Scan, Shelf, Target, Toast, Toasts, join};

const HEARTBEAT: Duration = Duration::from_secs(30);
/// How often the sign-in window is asked whether the user is through.
const WINDOW_POLL: Duration = Duration::from_millis(300);
const BACKOFF: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
    Duration::from_secs(300),
];

fn utc8_today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis() as u64)
        .unwrap_or(0);
    let days = ((now / 1_000) as i64 + 8 * 3_600) / 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub problem: Option<SignInProblem>,
    pub summary: String,
    pub detail: Option<String>,
}

impl Failure {
    fn new(error: &Error) -> Self {
        let reason = format!("{error:#}");
        let problem = error
            .downcast_ref::<SignInFailure>()
            .map(|failure| failure.0)
            .or_else(|| music::trouble::offline(&reason).then_some(SignInProblem::Network));
        let detail = error
            .chain()
            .skip(1)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        Self {
            problem,
            summary: error.to_string(),
            detail: (!detail.is_empty()).then(|| detail.join(": ")),
        }
    }

    /// Whether the sign-in failed because there was no network, rather than because the account
    /// was refused. A provider that was only unreachable is still the user's.
    pub fn offline(&self) -> bool {
        self.problem == Some(SignInProblem::Network)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    SignedOut,
    Restoring,
    Authorizing(Option<SignInPrompt>),
    SignedIn(UserProfile),
    /// The stored account could not be reached. It is still the user's, so nothing is signed
    /// out: the app stays on whatever the library kept and on the local files, and tries the
    /// account again by itself once the network is back.
    Offline(Failure),
    Failed(Failure),
}

pub enum SessionEvent {
    SignedIn,
    SignedOut,
    Reconnected,
    /// Offline mode let go of a live account. What was loaded stays, but nothing reaches the
    /// network until the account signs in again.
    WentOffline,
    LocalChanged,
}

#[derive(Clone, Copy)]
enum SecretInput {
    Browser,
    Manual,
}

pub struct ProviderInfo {
    pub slug: gpui::SharedString,
    pub name: gpui::SharedString,
    pub options: Vec<SignIn>,
    pub web_sign_in: bool,
    pub stored: bool,
    /// Whether what is stored is an anonymous session rather than an account.
    pub guest: bool,
    pub active: bool,
    pub pending: bool,
    pub error: Option<Failure>,
}

/// A provider session kept warm after it has been used, so coming back to it needs no wasm
/// start-up or account round trip.
#[derive(Clone)]
struct Cached {
    api: Arc<dyn MusicApi>,
    playback: Arc<dyn PlaybackFactory>,
    shape: Shape,
    authenticated: bool,
    capabilities: Capabilities,
    profile: UserProfile,
}

pub struct Session {
    state: SessionState,
    providers: Vec<Arc<dyn MusicProvider>>,
    active: Option<usize>,
    awaiting: Option<usize>,
    /// The provider whose sign-in page is open, and the method that page started with. It
    /// outlives a single attempt so the page can offer another method or retry a failure.
    login: Option<&'static str>,
    login_method: Option<SignIn>,
    resume: Option<(usize, UserProfile)>,
    /// The providers a restore has already given up on, so falling back never comes round to
    /// one of them again. A sign-in that lands empties it.
    passed: Vec<usize>,
    error: Option<(usize, Failure)>,
    settings: Entity<AppSettings>,
    /// Signed-in providers kept warm, by slug, so switching back costs no restore.
    sessions: HashMap<String, Cached>,
    /// Providers whose warm-up restore is still running, so it is not started twice.
    warming: HashSet<String>,
    client: Option<Arc<dyn MusicApi>>,
    catalog: Option<Arc<CatalogSource>>,
    playback: Option<Arc<dyn PlaybackFactory>>,
    shape: Shape,
    authenticated: bool,
    capabilities: Capabilities,
    io: Io,
    task: Option<Task<()>>,
    prompt_task: Option<Task<()>>,
    input: Option<UnboundedSender<String>>,
    /// The browser window a `SignInPrompt::Secret` opened, while it is up.
    window: Option<webview::Page>,
    window_task: Option<Task<()>>,
    local_provider: Arc<dyn MusicProvider>,
    local_folders: Vec<PathBuf>,
    plugins: music::plugins::Catalog,
    local_client: Option<Arc<dyn MusicApi>>,
    /// The latest local metadata and exact move aliases, applied to already queued songs.
    local_tracks: HashMap<String, Track>,
    local_moves: Vec<(String, String)>,
    local_issues: Vec<String>,
    local_catalog: Option<Arc<CatalogSource>>,
    local_playback: Option<Arc<dyn PlaybackFactory>>,
    local_capabilities: Capabilities,
    local_task: Option<Task<()>>,
    /// Protects a newer folder selection from a scan that was already finishing when cancelled.
    local_revision: u64,
    /// Whether a local scan is under way, so the UI can show its progress.
    scanning: bool,
    /// Whether the scan under way is one the user started, so its end is reported. An automatic
    /// rescan that cancels it takes the report over.
    scan_asked: bool,
    watch: Option<Task<()>>,
    reconnect: Option<Task<()>>,
    reconnecting: bool,
    attempt: usize,
    /// The server libraries the library picker lists, or why they could not be read. `None`
    /// while a read is under way or when none was asked for.
    libraries: Option<Result<Vec<Library>, String>>,
    libraries_task: Option<Task<()>>,
    checkin_task: Option<Task<()>>,
    checkin_busy: bool,
    /// Whether the sign-in overlay is up, including the provider list between a provider's
    /// card and the app.
    login_page: bool,
    /// Whether this run uses only the local music folders, without a streaming session.
    local_only: bool,
}

impl EventEmitter<SessionEvent> for Session {}

impl Session {
    pub fn new(
        providers: Vec<Arc<dyn MusicProvider>>,
        local_provider: Arc<dyn MusicProvider>,
        plugins: music::plugins::Catalog,
        settings: Entity<AppSettings>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        let remembered = settings.read(cx).provider().to_string();
        let local_folders = settings.read(cx).local_folders().to_vec();
        let local_only = remembered == local_provider.slug();
        // [wisp] 单 provider 场景（网易云）：设置加载时已把旧 slug 改写成 netease（见
        // `Values::settle_provider`），这里只是兜底——设置文件被手改成不认识的 slug 时，
        // 直接选中唯一的 provider，避免用户停在"没有服务"的空登录页。
        let active = match local_only {
            true => None,
            false => providers
                .iter()
                .position(|provider| provider.slug() == remembered)
                .or_else(|| providers.iter().position(|provider| provider.stored()))
                .or_else(|| (providers.len() == 1).then_some(0))
                .or_else(|| {
                    providers.iter().position(|provider| {
                        provider
                            .sign_in_options()
                            .iter()
                            .any(|option| matches!(option, SignIn::Anonymous))
                    })
                }),
        };
        let local_playback = local_provider.playback_factory();
        let mut session = Self {
            state: SessionState::SignedOut,
            providers,
            active,
            awaiting: None,
            login: None,
            login_method: None,
            resume: None,
            passed: Vec::new(),
            error: None,
            settings,
            sessions: HashMap::new(),
            warming: HashSet::new(),
            client: None,
            catalog: None,
            playback: None,
            shape: Shape::Saved,
            authenticated: false,
            capabilities: Capabilities::NONE,
            io,
            task: None,
            prompt_task: None,
            input: None,
            window: None,
            window_task: None,
            local_provider,
            local_folders,
            plugins,
            local_client: None,
            local_tracks: HashMap::new(),
            local_moves: Vec::new(),
            local_issues: Vec::new(),
            local_catalog: None,
            local_playback,
            local_capabilities: Capabilities::NONE,
            local_task: None,
            local_revision: 0,
            scanning: false,
            scan_asked: false,
            watch: None,
            reconnect: None,
            reconnecting: false,
            attempt: 0,
            libraries: None,
            libraries_task: None,
            checkin_task: None,
            checkin_busy: false,
            login_page: false,
            local_only,
        };
        session.restore_local(cx);
        if local_only {
            session.state = SessionState::SignedIn(session.local_profile());
        }
        session
    }

    pub fn checkin_busy(&self) -> bool {
        self.checkin_busy
    }

    /// Claims the provider's daily reward once per day, silently. The heartbeat calls this so a
    /// long-running window crosses midnight as well as a fresh sign-in.
    pub fn auto_checkin(&mut self, cx: &mut Context<Self>) {
        if self.checkin_busy
            || !self.authenticated
            || !self.capabilities.checkin
            || !self.settings.read(cx).auto_checkin()
            || self.settings.read(cx).last_checkin_date() == utc8_today()
        {
            return;
        }
        self.checkin(false, cx);
    }

    /// Claims the reward now. `notify` shows the result; automatic runs stay silent.
    pub fn checkin(&mut self, notify: bool, cx: &mut Context<Self>) {
        if self.checkin_busy {
            return;
        }
        let Some(client) = self.client.clone() else {
            if notify {
                Toasts::show(Outcome::Failed, "toast-checkin-signin", cx);
            }
            return;
        };
        if !self.capabilities.checkin {
            if notify {
                Toasts::show(Outcome::Failed, "toast-checkin-unsupported", cx);
            }
            return;
        }
        self.checkin_busy = true;
        cx.notify();
        let io = self.io.clone();
        self.checkin_task = Some(cx.spawn(async move |this, cx| {
            let result = join(io.spawn(async move { client.checkin(None).await })).await;
            this.update(cx, |this, cx| {
                this.checkin_busy = false;
                match result {
                    Ok(CheckinReport {
                        date,
                        claimed,
                        upgraded,
                        claim_code,
                        upgrade_code,
                    }) => {
                        this.settings.update(cx, |settings, cx| {
                            settings.set_last_checkin(&date, claimed, upgraded, cx)
                        });
                        log::info!(
                            "session: check-in {date} claimed={claimed} upgraded={upgraded} codes={claim_code:?}/{upgrade_code:?}"
                        );
                        if notify {
                            let key = if claimed || upgraded {
                                "toast-checkin-done"
                            } else {
                                "toast-checkin-empty"
                            };
                            Toasts::about(Outcome::Done, key, date, cx);
                        }
                    }
                    Err(error) => {
                        log::warn!("session: check-in failed: {error:#}");
                        // Mark the day attempted so the heartbeat does not hammer a failing
                        // endpoint every 30 seconds; a manual check-in still ignores this.
                        let today = utc8_today();
                        this.settings.update(cx, |settings, cx| {
                            settings.set_last_checkin(&today, false, false, cx)
                        });
                        if notify {
                            Toasts::about(
                                Outcome::Failed,
                                "toast-checkin-failed",
                                format!("{error:#}"),
                                cx,
                            );
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn client(&self) -> Option<Arc<dyn MusicApi>> {
        self.client.clone()
    }

    pub fn playback(&self) -> Option<Arc<dyn PlaybackFactory>> {
        self.playback.clone()
    }

    pub fn local_client(&self) -> Option<Arc<dyn MusicApi>> {
        self.local_client.clone()
    }

    /// Updated metadata for an indexed file, following the exact moves the last scan found.
    pub(crate) fn refreshed_local_track(&self, id: &str) -> Option<&Track> {
        let mut current = id;
        for _ in 0..8 {
            if let Some(track) = self.local_tracks.get(current) {
                return Some(track);
            }
            let (_, next) = self.local_moves.iter().find(|(old, _)| old == current)?;
            current = next;
        }
        None
    }

    /// The client serving a shelf, if that shelf has a provider right now.
    pub fn client_of(&self, shelf: Shelf) -> Option<Arc<dyn MusicApi>> {
        match shelf {
            Shelf::Streaming => self.client.clone(),
            Shelf::Local => self.local_client.clone(),
        }
    }

    /// What a shelf's library is made of. The local shelf is always a fixed catalog.
    pub fn shape_of(&self, shelf: Shelf) -> Shape {
        match shelf {
            Shelf::Streaming => self.shape,
            Shelf::Local => Shape::Catalog(Catalog::Fixed),
        }
    }

    pub(crate) fn catalog(&self, id: &str) -> Option<Arc<CatalogSource>> {
        match music::is_local_id(id) {
            true => self.local_catalog.clone(),
            false => self.catalog.clone(),
        }
    }

    pub fn local_playback(&self) -> Option<Arc<dyn PlaybackFactory>> {
        self.local_playback.clone()
    }

    /// The local provider itself, so a file can be read as a track without waiting for
    /// a scan: an explorer open may point outside every configured folder.
    pub fn local_provider(&self) -> Arc<dyn MusicProvider> {
        self.local_provider.clone()
    }

    pub fn local_paths(&self) -> Vec<String> {
        self.local_folders
            .iter()
            .map(|path| path.display().to_string())
            .collect()
    }

    pub fn providers(&self) -> impl Iterator<Item = ProviderInfo> + '_ {
        self.providers
            .iter()
            .enumerate()
            .map(|(index, provider)| ProviderInfo {
                slug: provider.slug().into(),
                name: self.providers_name(provider.slug(), provider.name()),
                options: provider.sign_in_options(),
                // Asked in this order because answering `supported` starts a child process once
                // on Linux, and only a provider that signs in with cookies is worth it.
                web_sign_in: provider.web_sign_in().is_some() && webview::supported(),
                stored: provider.stored(),
                guest: provider.stored_guest(),
                active: self.active == Some(index),
                pending: self.awaiting == Some(index),
                error: match &self.error {
                    Some((failed, failure)) if *failed == index => Some(failure.clone()),
                    _ => None,
                },
            })
    }

    /// The plugin manifest's display name wins over the provider's compiled-in one, so an
    /// installed plugin can rename a provider without changing the provider trait. The name is
    /// the manifest's translation for the active app language when it has one.
    fn providers_name(&self, slug: &str, fallback: &'static str) -> gpui::SharedString {
        self.plugins
            .find(slug)
            .map(|plugin| i18n::localized(plugin.name, &plugin.name_i18n.others))
            .unwrap_or(fallback)
            .into()
    }

    /// The providers with an account stored. A stored guest session is not an account, so it is
    /// left out even while it is the one in use.
    pub fn connected(&self) -> impl Iterator<Item = ProviderInfo> + '_ {
        self.providers().filter(|info| info.stored && !info.guest)
    }

    /// Whether a browser window matters right now: the current provider signs in through one,
    /// or no account is stored yet and the sign-in screen offers every provider.
    pub fn wants_browser(&self) -> bool {
        if self.local_only {
            return false;
        }
        match self.active {
            Some(index) => self.providers[index].web_sign_in().is_some(),
            None => {
                !self.providers.is_empty()
                    && !self.providers.iter().any(|provider| provider.stored())
            }
        }
    }

    pub fn forget(&mut self, slug: &str, cx: &mut Context<Self>) {
        if self.local_only && slug == self.local_slug() {
            return self.sign_out(cx);
        }
        let Some(index) = self
            .providers
            .iter()
            .position(|provider| provider.slug() == slug)
        else {
            return;
        };
        if self.active == Some(index) {
            return self.sign_out(cx);
        }
        self.providers[index].sign_out();
        cx.notify();
    }

    pub fn switch(&mut self, slug: &str, cx: &mut Context<Self>) {
        if self.is_pending() {
            return;
        }
        let Some(index) = self
            .providers
            .iter()
            .position(|provider| provider.slug() == slug)
        else {
            return;
        };
        if self.active == Some(index) && matches!(self.state, SessionState::SignedIn(_)) {
            return;
        }
        self.local_only = false;
        if let Some(cached) = self
            .sessions
            .get(slug)
            .filter(|cached| cached.api.alive())
            .cloned()
        {
            self.switch_cached(cached, index, cx);
            return;
        }
        self.sessions.remove(slug);
        self.release(cx);
        self.active = Some(index);
        self.restore(cx);
    }

    /// Moves to a provider whose session is still warm. Nothing is released, so switching back
    /// and forth costs neither a wasm start-up nor an account round trip.
    fn switch_cached(&mut self, cached: Cached, index: usize, cx: &mut Context<Self>) {
        self.local_only = false;
        self.task = None;
        self.watch = None;
        self.reconnect = None;
        self.reconnecting = false;
        self.attempt = 0;
        self.prompt_task = None;
        self.input = None;
        self.window = None;
        self.window_task = None;
        self.awaiting = None;
        self.login = None;
        self.login_method = None;
        self.resume = None;
        self.libraries = None;
        self.libraries_task = None;
        self.checkin_task = None;
        self.checkin_busy = false;

        self.active = Some(index);
        self.catalog = Some(Arc::new(CatalogSource::new(cached.api.clone())));
        self.client = Some(cached.api);
        self.playback = Some(cached.playback);
        self.shape = cached.shape;
        self.authenticated = cached.authenticated;
        self.capabilities = cached.capabilities;
        self.state = SessionState::SignedIn(cached.profile);
        let slug = self.providers[index].slug();
        self.settings.update(cx, |settings, cx| {
            settings.set_provider(slug, cx);
        });
        log::info!("session: switched to {slug} from the warm cache");
        self.start_heartbeat(cx);
        self.auto_checkin(cx);
        cx.notify();
        cx.emit(SessionEvent::SignedIn);
        self.warm_others(cx);
    }

    /// Replaces the streaming provider list after a plugin was installed, enabled, disabled or
    /// removed. The active provider keeps its live client when its slug is still present; a
    /// provider that just appeared is restored from its stored account, so installing or enabling
    /// a plugin signs in and starts serving without an app restart.
    pub fn reload_providers(
        &mut self,
        providers: Vec<Arc<dyn MusicProvider>>,
        cx: &mut Context<Self>,
    ) {
        let previous: Vec<&'static str> = self
            .providers
            .iter()
            .map(|provider| provider.slug())
            .collect();
        let present: HashSet<&str> = providers.iter().map(|provider| provider.slug()).collect();
        self.sessions
            .retain(|slug, _| present.contains(slug.as_str()));
        self.warming.retain(|slug| present.contains(slug.as_str()));
        let guest_added = providers.iter().position(|provider| {
            !previous.contains(&provider.slug())
                && provider
                    .sign_in_options()
                    .iter()
                    .any(|option| matches!(option, SignIn::Anonymous))
        });
        let current = self
            .active
            .and_then(|index| self.providers.get(index).map(|provider| provider.slug()));
        if let Some(slug) = current
            && let Some(index) = providers
                .iter()
                .position(|provider| provider.slug() == slug)
        {
            let pending = self.is_pending();
            self.providers = providers;
            self.active = Some(index);
            if pending {
                self.release(cx);
                self.restore(cx);
            } else if matches!(self.state, SessionState::SignedOut)
                && let Some(index) = guest_added
            {
                // A read-only provider installed while the app sits signed out should serve
                // its guest session now, instead of leaving the page on the old empty one.
                self.active = Some(index);
                self.release(cx);
                self.restore(cx);
            } else {
                cx.notify();
            }
            return;
        }

        let was_live = self.active.is_some()
            || self.client.is_some()
            || matches!(
                self.state,
                SessionState::Restoring | SessionState::SignedIn(_) | SessionState::Offline(_)
            );
        self.providers = providers;
        self.active = None;
        if was_live {
            self.release(cx);
        } else {
            self.state = SessionState::SignedOut;
        }
        self.passed.clear();
        self.error = None;
        self.libraries = None;
        self.libraries_task = None;

        let remembered = self.settings.read(cx).provider().to_owned();
        self.active = self
            .providers
            .iter()
            .position(|provider| provider.slug() == remembered)
            .or_else(|| self.providers.iter().position(|provider| provider.stored()))
            .or_else(|| {
                self.providers.iter().position(|provider| {
                    provider
                        .sign_in_options()
                        .iter()
                        .any(|option| matches!(option, SignIn::Anonymous))
                })
            })
            .or_else(|| (!self.providers.is_empty()).then_some(0));
        if self.active.is_some() {
            self.restore(cx);
        } else {
            self.state = SessionState::SignedOut;
            cx.notify();
        }
    }

    pub fn provider_name(&self) -> Option<&'static str> {
        let provider = &self.providers[self.active?];
        Some(provider.name())
    }

    pub fn provider_slug(&self) -> Option<&'static str> {
        let provider = &self.providers[self.active?];
        Some(provider.slug())
    }

    /// The provider whose sign-in page is open, if any.
    pub fn login_slug(&self) -> Option<&'static str> {
        self.login
    }

    /// Whether the sign-in overlay is up, which stays true on the provider list after leaving
    /// a provider's card.
    pub fn login_page(&self) -> bool {
        self.login_page
    }

    /// Whether Offline was chosen as the music source. Streaming accounts remain stored.
    pub fn local_only(&self) -> bool {
        self.local_only
    }

    /// The shelf the library shows and searches in this run.
    pub fn primary_shelf(&self) -> Shelf {
        match self.local_only {
            true => Shelf::Local,
            false => Shelf::Streaming,
        }
    }

    /// The client for browsing and searching, without exposing a concrete provider.
    pub fn browsing_client(&self) -> Option<Arc<dyn MusicApi>> {
        self.client_of(self.primary_shelf())
    }

    /// The scope that owns the saved play queue, including an offline run.
    pub(crate) fn playback_slug(&self) -> Option<&'static str> {
        match self.local_only {
            true => Some(self.local_slug()),
            false => self.provider_slug(),
        }
    }

    /// Starts an offline run that plays only the local music folders. Stored streaming
    /// accounts are kept for a later switch back.
    pub fn use_local(&mut self, cx: &mut Context<Self>) {
        self.release(cx);
        self.active = None;
        self.local_only = true;
        self.login_page = false;
        let slug = self.local_slug();
        self.settings
            .update(cx, |settings, cx| settings.set_provider(slug, cx));
        if self.local_client.is_none() {
            self.restore_local(cx);
        }
        self.state = SessionState::SignedIn(self.local_profile());
        cx.notify();
        cx.emit(SessionEvent::SignedIn);
    }

    /// The profile an offline run is signed in as.
    fn local_profile(&self) -> UserProfile {
        UserProfile {
            id: self.local_slug().to_owned(),
            display_name: t!("login-offline-title").to_string(),
            avatar: None,
        }
    }

    /// The sign-in method the open page started with, which its method tabs read.
    pub fn login_method(&self) -> Option<&SignIn> {
        self.login_method.as_ref()
    }

    /// The host to try when checking whether the network is back. It is the active provider's
    /// own, so the check never touches a service the app is not already using.
    pub fn reach(&self) -> Option<String> {
        if self.local_only {
            return None;
        }
        self.providers.get(self.active?)?.reach()
    }

    /// What `link` points at, with the slug of the provider whose link it is. Every provider is
    /// asked, not only the current one.
    pub fn read_link(&self, link: &str) -> Option<(&'static str, Linked)> {
        self.providers
            .iter()
            .find_map(|provider| Some((provider.slug(), provider.link(link)?)))
    }

    /// The name of the provider `slug` and whether it has an account stored.
    pub fn account(&self, slug: &str) -> Option<(&'static str, bool)> {
        self.providers
            .iter()
            .find(|provider| provider.slug() == slug)
            .map(|provider| (provider.name(), provider.stored()))
    }

    pub fn local_slug(&self) -> &'static str {
        self.local_provider.slug()
    }

    pub fn active_slugs(&self) -> Vec<&'static str> {
        let mut slugs = Vec::new();
        if self.client.is_some() {
            slugs.extend(self.provider_slug());
        }
        if self.local_client.is_some() {
            slugs.push(self.local_slug());
        }
        slugs
    }

    pub fn slug_for(&self, id: &str) -> Option<&'static str> {
        self.provider_for(id).map(|provider| provider.slug())
    }

    /// The provider an id belongs to, whichever shelf it sits on.
    pub fn provider_for(&self, id: &str) -> Option<&dyn MusicProvider> {
        match music::is_local_id(id) {
            true => Some(self.local_provider.as_ref()),
            false => self.active.map(|index| self.providers[index].as_ref()),
        }
    }

    pub fn authenticated(&self) -> bool {
        self.authenticated
    }

    /// Whether the active session is an anonymous guest session rather than an authenticated account.
    pub fn guest(&self) -> bool {
        self.client.is_some() && !self.authenticated
    }

    /// What the live streaming provider can do beyond listing and playing. Nothing is offered
    /// while signed out, which is what the empty set means.
    /// What the signed-in provider can do. The stream levels are the playback factory's when it
    /// names any, since a provider with a second engine serves other levels through it.
    pub fn capabilities(&self) -> Capabilities {
        let qualities = self
            .playback
            .as_ref()
            .and_then(|playback| playback.qualities())
            .unwrap_or(self.capabilities.qualities);
        Capabilities {
            qualities,
            ..self.capabilities
        }
    }

    /// The same, for whichever shelf a thing belongs to. Anything that routes by id asks this
    /// one, since the two shelves can differ: local files keep favorites but seed no station.
    pub fn capabilities_of(&self, shelf: Shelf) -> Capabilities {
        match shelf {
            Shelf::Streaming => self.capabilities(),
            Shelf::Local => self.local_capabilities,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(
            self.state,
            SessionState::Restoring | SessionState::Authorizing(_)
        )
    }

    /// Tears the live session down and opens the stored sign-in again, after a change to what
    /// the stored account reads. The library, search,
    /// genres and home all reload on the sign-in that follows, and nothing the old client
    /// answered can arrive afterwards. A restore still under way is dropped and started over, so
    /// it cannot install a client built for the previous choice.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, SessionState::Authorizing(_)) || self.active.is_none() {
            return;
        }
        if let Some(active) = self.active {
            self.sessions.remove(self.providers[active].slug());
        }
        self.release(cx);
        self.restore(cx);
    }

    /// The provider whose stored account can be limited to one library of its server. A guest
    /// session has no account to limit.
    fn library_provider(&self) -> Option<&Arc<dyn MusicProvider>> {
        self.providers.iter().find(|provider| {
            provider.has_libraries() && provider.stored() && !provider.stored_guest()
        })
    }

    /// Whether a stored account can be limited to one library of its server.
    pub fn offers_libraries(&self) -> bool {
        self.library_provider().is_some()
    }

    /// The library the stored account reads, or `None` for the whole server. It reads what is
    /// stored and asks the server nothing.
    pub fn library(&self) -> Option<Library> {
        self.library_provider()?.library()
    }

    /// The libraries the last read found, or why it failed. `None` while the read is under way.
    pub fn libraries(&self) -> Option<&Result<Vec<Library>, String>> {
        self.libraries.as_ref()
    }

    /// Asks the stored account's server for its libraries, dropping any answer already held.
    pub fn load_libraries(&mut self, cx: &mut Context<Self>) {
        self.libraries = None;
        self.libraries_task = None;
        let Some(provider) = self.library_provider().cloned() else {
            cx.notify();
            return;
        };
        let io = self.io.clone();
        self.libraries_task = Some(cx.spawn(async move |this, cx| {
            let read = join(io.spawn(async move { provider.libraries().await })).await;
            this.update(cx, |this, cx| {
                this.libraries = Some(read.map_err(|error| format!("{error:#}")));
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Drops the libraries read for the picker and stops a read under way.
    pub fn forget_libraries(&mut self, cx: &mut Context<Self>) {
        self.libraries = None;
        self.libraries_task = None;
        cx.notify();
    }

    /// Keeps the stored account's library choice, or the whole server with `None`, and rebuilds
    /// the session around it when that account is the one in use. A stored account that is not
    /// in use picks the choice up when it next signs in. Fails when the choice cannot be kept.
    pub fn choose_library(
        &mut self,
        library: Option<Library>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let Some(provider) = self.library_provider().cloned() else {
            anyhow::bail!("no stored account can be limited to a library");
        };
        provider.choose_library(library)?;
        if self.provider_slug() == Some(provider.slug()) {
            self.reload(cx);
        }
        Ok(())
    }

    /// Opens the active provider's stored sign-in. One that cannot be opened hands over to the
    /// next provider with an account stored and then to a guest session, so the sign-in page
    /// comes up only when there is nothing at all to carry on with.
    pub fn restore(&mut self, cx: &mut Context<Self>) {
        if self.is_pending() {
            return;
        }
        if self.local_only {
            self.state = SessionState::SignedIn(self.local_profile());
            self.login_page = false;
            cx.notify();
            cx.emit(SessionEvent::SignedIn);
            return;
        }
        self.passed.clear();
        let Some(active) = self.active else {
            self.state = SessionState::SignedOut;
            cx.notify();
            cx.emit(SessionEvent::SignedOut);
            return;
        };
        self.restore_at(active, cx);
    }

    fn restore_at(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active = Some(index);
        self.state = SessionState::Restoring;
        cx.notify();

        let provider = self.providers[index].clone();
        log::debug!("session: restoring {}", provider.name());
        let io = self.io.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let restored = join(io.spawn(async move { provider.restore().await })).await;

            this.update(cx, |this, cx| match restored {
                Ok(Some(session)) => this.signed_in(session, index, cx),
                Ok(None) => this.fall_back(index, None, cx),
                // An account that is only out of reach is kept, and the app carries on offline.
                Err(error) if Failure::new(&error).offline() && this.stored_active() => {
                    this.failed(&error, cx)
                }
                Err(error) => this.fall_back(index, Some(&error), cx),
            })
            .ok();
        }));
    }

    /// Moves past a provider whose stored sign-in did not open, to the next provider with an
    /// account stored, or failing that to a guest session. The provider keeps its error for the
    /// settings page, and a toast says it could not sign back in, or that the provider invalidated
    /// the session when it refused the stored sign-in outright. With nowhere to go the
    /// session ends the way it always did, on the sign-in page.
    fn fall_back(&mut self, failed: usize, error: Option<&Error>, cx: &mut Context<Self>) {
        let provider = self.providers[failed].clone();
        self.passed.push(failed);
        self.sessions.remove(provider.slug());
        let next = self
            .providers
            .iter()
            .enumerate()
            .find(|(index, provider)| provider.stored() && !self.passed.contains(index))
            .map(|(index, _)| index);
        let guest = self
            .providers
            .iter()
            .enumerate()
            .find(|(index, provider)| {
                !self.passed.contains(index)
                    && provider
                        .sign_in_options()
                        .iter()
                        .any(|option| matches!(option, SignIn::Anonymous))
            })
            .map(|(_, provider)| provider.slug());

        if next.is_none() && guest.is_none() {
            match error {
                Some(error) => self.failed(error, cx),
                None => {
                    self.state = SessionState::SignedOut;
                    cx.notify();
                    cx.emit(SessionEvent::SignedOut);
                }
            }
            return;
        }

        match error {
            Some(error) => {
                log::warn!(
                    "session: cannot restore {}, falling back: {error:#}",
                    provider.name()
                );
                self.error = Some((failed, Failure::new(error)));
                Toasts::with_detail(
                    Outcome::Failed,
                    "toast-restore-failed",
                    Some(provider.name().into()),
                    format!("{error:#}"),
                    cx,
                );
            }
            None if provider.stored() => {
                log::warn!(
                    "session: {} refused its stored sign-in, falling back",
                    provider.name()
                );
                let toast = Toast::new(Outcome::Failed, "toast-session-invalidated")
                    .name(provider.name())
                    .open("toast-sign-in", Target::ProviderSettings)
                    .lasting();
                Toasts::post(toast, cx);
            }
            None => log::debug!(
                "session: {} has nothing stored, falling back",
                provider.name()
            ),
        }
        // The failed provider's library goes with it, as it would on the sign-in page.
        self.client = None;
        self.catalog = None;
        self.playback = None;
        cx.emit(SessionEvent::SignedOut);

        if let Some(index) = next {
            return self.restore_at(index, cx);
        }
        if let Some(slug) = guest {
            // A sign-in only starts from a settled state, and nothing renders in between.
            log::info!("session: starting the guest session for {slug}");
            self.state = SessionState::SignedOut;
            self.start_sign_in(slug, SignIn::Anonymous, SecretInput::Browser, cx);
        }
    }

    pub fn sign_in(&mut self, slug: &str, method: SignIn, cx: &mut Context<Self>) {
        self.start_sign_in(slug, method, SecretInput::Browser, cx);
    }

    /// Switches the open sign-in page to another method without closing it. A failure already
    /// settled needs no cancel; an attempt in flight is abandoned first.
    pub fn switch_sign_in(&mut self, slug: &str, method: SignIn, cx: &mut Context<Self>) {
        if matches!(self.state, SessionState::Authorizing(_)) {
            self.cancel_sign_in(cx);
        }
        self.start_sign_in(slug, method, SecretInput::Browser, cx);
    }

    pub fn sign_in_with_cookies(&mut self, slug: &str, cx: &mut Context<Self>) {
        self.start_sign_in(slug, SignIn::Secret, SecretInput::Manual, cx);
    }

    fn start_sign_in(
        &mut self,
        slug: &str,
        method: SignIn,
        secret_input: SecretInput,
        cx: &mut Context<Self>,
    ) {
        if self.is_pending() {
            return;
        }
        let Some(index) = self
            .providers
            .iter()
            .position(|provider| provider.slug() == slug)
        else {
            return;
        };
        self.local_only = false;
        self.resume = match &self.state {
            SessionState::SignedIn(profile) => self.active.map(|active| (active, profile.clone())),
            _ => None,
        };
        self.error = None;
        self.login_page = true;
        self.login = Some(self.providers[index].slug());
        self.login_method = Some(method.clone());
        self.awaiting = Some(index);
        self.state = SessionState::Authorizing(None);
        cx.notify();

        let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        self.input = Some(input_tx);
        let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<SignInPrompt>();
        self.prompt_task = Some(cx.spawn(async move |this, cx| {
            while let Some(prompt) = prompt_rx.recv().await {
                this.update(cx, |this, cx| {
                    if matches!(this.state, SessionState::Authorizing(_)) {
                        let secret = matches!(prompt, SignInPrompt::Secret);
                        this.state = SessionState::Authorizing(Some(prompt));
                        cx.notify();
                        if secret && matches!(secret_input, SecretInput::Browser) {
                            this.open_window(cx);
                        }
                    }
                })
                .ok();
            }
        }));
        let prompt: PromptSink = Arc::new(move |prompt| {
            prompt_tx.send(prompt).ok();
        });

        let provider = self.providers[index].clone();
        log::debug!("session: restoring {}", provider.name());
        let io = self.io.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let authorized =
                join(io.spawn(async move { provider.sign_in(method, prompt, input_rx).await }))
                    .await;

            this.update(cx, |this, cx| {
                this.prompt_task = None;
                this.input = None;
                this.window = None;
                match authorized {
                    Ok(session) => this.signed_in(session, index, cx),
                    Err(error) => this.failed(&error, cx),
                }
            })
            .ok();
        }));
    }

    /// Drops the attempt and the sign-in page, returning to whatever the app showed before.
    pub fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        self.end_sign_in(false, cx);
    }

    /// Drops the prompt for the current attempt but leaves the sign-in page up, so another
    /// method or provider can be picked without starting over.
    pub fn dismiss_sign_in(&mut self, cx: &mut Context<Self>) {
        self.end_sign_in(true, cx);
    }

    /// Leaves a provider's card for the provider list, keeping the sign-in overlay up.
    pub fn back_to_login(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, SessionState::Authorizing(_)) {
            self.end_sign_in(true, cx);
        }
        self.login = None;
        self.login_method = None;
        self.login_page = true;
        cx.notify();
    }

    /// Closes the whole sign-in overlay, from the provider list or a settled card.
    pub fn close_login(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, SessionState::Authorizing(_)) {
            self.end_sign_in(false, cx);
            return;
        }
        self.login = None;
        self.login_method = None;
        self.awaiting = None;
        self.login_page = false;
        cx.notify();
    }

    fn end_sign_in(&mut self, keep_page: bool, cx: &mut Context<Self>) {
        if !matches!(self.state, SessionState::Authorizing(_)) {
            return;
        }
        let login = self.login;
        let method = self.login_method.clone();
        if let Some(index) = self.awaiting {
            let provider = self.providers[index].clone();
            self.io.spawn(async move { provider.abandon() });
        }
        self.task = None;
        self.prompt_task = None;
        self.input = None;
        self.window = None;
        self.window_task = None;
        self.awaiting = None;
        self.error = None;
        self.login = None;
        self.login_method = None;
        match self.resume.take() {
            Some((index, profile)) => {
                self.active = Some(index);
                self.state = SessionState::SignedIn(profile);
            }
            None => {
                self.state = SessionState::SignedOut;
                cx.emit(SessionEvent::SignedOut);
            }
        }
        self.login_page = keep_page;
        if keep_page {
            self.login = login;
            self.login_method = method;
        }
        cx.notify();
    }

    /// Opens the browser window for the secret prompt now showing. The window answers the prompt
    /// itself once the user is through; closing it cancels the sign-in.
    fn open_window(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.state,
            SessionState::Authorizing(Some(SignInPrompt::Secret))
        ) || self.window.is_some()
        {
            return;
        }
        let Some(index) = self.awaiting else {
            return;
        };
        let provider = &self.providers[index];
        let Some(sign_in) = provider.web_sign_in() else {
            return;
        };
        let target = webview::Target {
            url: sign_in.url.to_string(),
            landing: sign_in.landing.to_string(),
            domain: sign_in.domain.to_string(),
            proof: sign_in.proof.iter().map(ToString::to_string).collect(),
            title: t!("login-window-title", provider = provider.name()).to_string(),
            agent: sign_in.agent.map(str::to_owned),
            script: None,
        };
        match webview::Page::open(target) {
            Ok(login) => self.window = Some(login),
            Err(error) => {
                log::warn!("session: cannot open the sign-in window: {error:#}");
                // Dropping the provider's task closes its prompt channel, which ends the prompt
                // task on its own; this runs inside that task, so it must not drop it here.
                self.task = None;
                self.input = None;
                return self.failed(&error, cx);
            }
        }
        self.window_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WINDOW_POLL).await;
                let open = this.update(cx, |this, cx| {
                    let Some(login) = this.window.as_mut() else {
                        return false;
                    };
                    match login.poll() {
                        webview::Poll::Pending => true,
                        webview::Poll::Closed => {
                            this.window = None;
                            this.cancel_sign_in(cx);
                            false
                        }
                        webview::Poll::Cookies(header) => {
                            this.window = None;
                            this.submit_input(header, cx);
                            false
                        }
                    }
                });
                if !open.unwrap_or(false) {
                    break;
                }
            }
        }));
    }

    pub fn submit_input(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(input) = &self.input {
            self.window = None;
            input.send(text).ok();
            if let SessionState::Authorizing(Some(
                SignInPrompt::Secret | SignInPrompt::Accounts(_) | SignInPrompt::Verify { .. },
            )) = &self.state
            {
                self.state = SessionState::Authorizing(None);
                cx.notify();
            }
        }
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        if self.local_only {
            self.settings
                .update(cx, |settings, cx| settings.set_provider("", cx));
            self.local_only = false;
            self.release(cx);
            return;
        }
        if let Some(active) = self.active {
            let slug = self.providers[active].slug();
            self.providers[active].sign_out();
            self.sessions.remove(slug);
        }
        self.release(cx);
        if let Some(index) = self.remaining() {
            self.active = Some(index);
            self.restore(cx);
        }
    }

    /// Whether the active provider has an account stored, which is what tells a failed restore
    /// from a user who signed out.
    fn stored_active(&self) -> bool {
        self.active
            .is_some_and(|index| self.providers[index].stored())
    }

    /// Tries the stored account again once the network is back, for a run that started without
    /// one. Anything but a run held up by the network is left alone.
    pub fn restore_if_offline(&mut self, cx: &mut Context<Self>) {
        if self.local_only {
            return;
        }
        if matches!(self.state, SessionState::Offline(_)) {
            self.restore(cx);
        }
    }

    fn remaining(&self) -> Option<usize> {
        self.active
            .filter(|index| self.providers[*index].stored())
            .or_else(|| self.providers.iter().position(|provider| provider.stored()))
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.watch = None;
        self.reconnect = None;
        self.reconnecting = false;
        self.attempt = 0;
        self.prompt_task = None;
        self.input = None;
        self.window = None;
        self.window_task = None;
        self.awaiting = None;
        self.login = None;
        self.login_method = None;
        self.login_page = false;
        self.resume = None;
        self.client = None;
        self.catalog = None;
        self.playback = None;
        self.shape = Shape::Saved;
        self.authenticated = false;
        self.capabilities = Capabilities::NONE;
        self.state = SessionState::SignedOut;
        cx.notify();
        cx.emit(SessionEvent::SignedOut);
    }

    fn signed_in(&mut self, session: ProviderSession, index: usize, cx: &mut Context<Self>) {
        self.local_only = false;
        let replaced = self
            .resume
            .take()
            .is_some_and(|(held, profile)| held != index || profile.id != session.profile.id);
        if replaced {
            self.drop_previous_session(cx);
        }
        self.active = Some(index);
        self.awaiting = None;
        self.login = None;
        self.login_method = None;
        self.login_page = false;
        // A provider passed over on the way here keeps its error until it signs in again.
        self.error = self.error.take().filter(|(failed, _)| *failed != index);
        self.passed.clear();
        let slug = self.providers[index].slug();
        self.settings.update(cx, |settings, cx| {
            settings.set_provider(slug, cx);
        });
        self.sessions.insert(
            slug.to_owned(),
            Cached {
                api: session.api.clone(),
                playback: session.playback.clone(),
                shape: session.shape,
                authenticated: session.authenticated,
                capabilities: session.capabilities,
                profile: session.profile.clone(),
            },
        );
        self.catalog = Some(Arc::new(CatalogSource::new(session.api.clone())));
        self.client = Some(session.api);
        self.playback = Some(session.playback);
        self.shape = session.shape;
        self.authenticated = session.authenticated;
        self.capabilities = session.capabilities;
        self.state = SessionState::SignedIn(session.profile);
        log::debug!(
            "session: signed in {} (guest={})",
            slug,
            !session.authenticated
        );
        self.attempt = 0;
        self.start_heartbeat(cx);
        self.auto_checkin(cx);
        cx.notify();
        cx.emit(SessionEvent::SignedIn);
        self.warm_others(cx);
    }

    /// Restores the other providers that have a stored account in the background, so the first
    /// switch to one of them does not pay the wasm start-up and profile round trip.
    fn warm_others(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        let wanted: Vec<Arc<dyn MusicProvider>> = self
            .providers
            .iter()
            .enumerate()
            .filter(|(index, provider)| {
                Some(*index) != active
                    && !self.sessions.contains_key(provider.slug())
                    && !self.warming.contains(provider.slug())
                    && (provider.stored() || provider.stored_guest())
            })
            .map(|(_, provider)| provider.clone())
            .collect();
        for provider in wanted {
            let slug = provider.slug();
            let io = self.io.clone();
            self.warming.insert(slug.to_owned());
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let restored = join(io.spawn(async move { provider.restore().await })).await;
                this.update(cx, |this, cx| {
                    this.warming.remove(slug);
                    let Ok(Some(session)) = restored else {
                        return;
                    };
                    if this.sessions.contains_key(slug) {
                        return;
                    }
                    this.sessions.insert(
                        slug.to_owned(),
                        Cached {
                            api: session.api,
                            playback: session.playback,
                            shape: session.shape,
                            authenticated: session.authenticated,
                            capabilities: session.capabilities,
                            profile: session.profile,
                        },
                    );
                    log::debug!("session: kept {slug} warm for a switch");
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn drop_previous_session(&mut self, cx: &mut Context<Self>) {
        if let Some(slug) = self.active.and_then(|active| self.providers.get(active)) {
            self.sessions.remove(slug.slug());
        }
        self.client = None;
        self.catalog = None;
        self.playback = None;
        self.shape = Shape::Saved;
        self.authenticated = false;
        self.capabilities = Capabilities::NONE;
        self.watch = None;
        self.reconnect = None;
        self.reconnecting = false;
        self.attempt = 0;
        cx.emit(SessionEvent::SignedOut);
    }

    fn start_heartbeat(&mut self, cx: &mut Context<Self>) {
        self.watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(HEARTBEAT).await;
                if this
                    .update(cx, |this, cx| {
                        this.reconnect_if_stale(cx);
                        this.auto_checkin(cx);
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    pub fn reconnect_if_stale(&mut self, cx: &mut Context<Self>) -> bool {
        if self.reconnecting {
            return true;
        }
        if !matches!(self.state, SessionState::SignedIn(_)) {
            return false;
        }
        let Some(client) = &self.client else {
            return false;
        };
        if client.alive() {
            self.attempt = 0;
            return false;
        }
        let Some(active) = self.active else {
            return false;
        };
        let wait = BACKOFF[self.attempt.min(BACKOFF.len() - 1)];
        self.attempt += 1;
        self.reconnecting = true;
        log::warn!(
            "session: the {} session went stale, reconnecting in {}s",
            self.providers[active].name(),
            wait.as_secs()
        );
        let provider = self.providers[active].clone();
        let io = self.io.clone();
        self.reconnect = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let restored = join(io.spawn(async move { provider.restore().await })).await;
            this.update(cx, |this, cx| {
                this.reconnecting = false;
                match restored {
                    Ok(Some(session)) => this.reconnected(session, cx),
                    Ok(None) => log::warn!("session: nothing stored to reconnect with"),
                    Err(error) => log::warn!("session: cannot reconnect: {error:#}"),
                }
            })
            .ok();
        }));
        true
    }

    fn reconnected(&mut self, session: ProviderSession, cx: &mut Context<Self>) {
        self.attempt = 0;
        self.catalog = Some(Arc::new(CatalogSource::new(session.api.clone())));
        if let Some(slug) = self
            .active
            .and_then(|active| self.providers.get(active))
            .map(|p| p.slug())
        {
            self.sessions.insert(
                slug.to_owned(),
                Cached {
                    api: session.api.clone(),
                    playback: session.playback.clone(),
                    shape: session.shape,
                    authenticated: session.authenticated,
                    capabilities: session.capabilities,
                    profile: session.profile.clone(),
                },
            );
        }
        self.client = Some(session.api);
        self.playback = Some(session.playback);
        self.shape = session.shape;
        self.authenticated = session.authenticated;
        self.capabilities = session.capabilities;
        log::debug!("session: reconnected");
        cx.notify();
        cx.emit(SessionEvent::Reconnected);
    }

    /// Keeps the stored account without reaching it, so the app runs on what the library kept,
    /// which still plays through an engine built from the stored sign-in alone.
    fn carry_on_offline(&mut self, failure: Failure, cx: &mut Context<Self>) {
        // A provider may need the runtime to build its offline engine.
        let _runtime = self.io.handle().enter();
        self.playback = self
            .active
            .and_then(|active| self.providers[active].offline_playback());
        self.state = SessionState::Offline(failure);
        cx.notify();
    }

    fn failed(&mut self, error: &Error, cx: &mut Context<Self>) {
        let failure = Failure::new(error);
        Network::failed(&format!("{error:#}"), cx);
        if let Some(failed) = self.awaiting.or(self.active) {
            self.error = Some((failed, failure.clone()));
        }
        // Only a restore may end up offline. A sign-in the user is watching says what went
        // wrong on the page they started it from.
        let restoring = self.awaiting.is_none();
        self.awaiting = None;
        if let Some((index, profile)) = self.resume.take() {
            self.active = Some(index);
            self.state = SessionState::SignedIn(profile);
            cx.notify();
            return;
        }
        if restoring && failure.offline() && self.stored_active() {
            log::warn!("session: the account could not be reached, carrying on offline");
            return self.carry_on_offline(failure, cx);
        }
        self.client = None;
        self.catalog = None;
        self.playback = None;
        self.state = SessionState::Failed(failure);
        cx.notify();
        cx.emit(SessionEvent::SignedOut);
    }

    fn restore_local(&mut self, cx: &mut Context<Self>) {
        if self.local_folders.is_empty() {
            return;
        }
        self.rescan_local(false, cx);
    }

    /// Adds a folder to the local library, then rescans every configured folder together so
    /// artists and albums that span more than one root merge into one, seamlessly.
    pub fn add_local_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.add_local_folders(vec![path], cx);
    }

    /// Same as [`Session::add_local_folder`], for a batch picked in one native dialog.
    pub fn add_local_folders(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut folders = self.local_folders.clone();
        for path in paths {
            if folders.iter().any(|existing| overlaps(existing, &path)) {
                log::warn!(
                    "session: {} overlaps an already-added local folder",
                    path.display()
                );
                continue;
            }
            folders.push(path);
        }
        if folders.len() == self.local_folders.len() {
            return;
        }
        self.set_local_folders(folders, true, cx);
    }

    /// Drops a folder from the local library and offers to put it back.
    pub fn remove_local_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(at) = self
            .local_folders
            .iter()
            .position(|existing| existing == path)
        else {
            return;
        };
        let mut folders = self.local_folders.clone();
        let removed = folders.remove(at);
        self.set_local_folders(folders, true, cx);

        let session = cx.entity().downgrade();
        let toast = Toast::new(Outcome::Plain, "toast-folder-removed")
            .name(folder_name(&removed))
            .undo(move |cx| {
                let removed = removed.clone();
                session
                    .update(cx, |session, cx| {
                        session.restore_local_folder(removed, at, cx)
                    })
                    .ok();
            });
        Toasts::post(toast, cx);
    }

    /// Puts a folder the user just removed back where it was in the list. A folder gone from
    /// disk is reported instead, and one that overlaps a folder added since is left alone.
    fn restore_local_folder(&mut self, path: PathBuf, at: usize, cx: &mut Context<Self>) {
        if !path.is_dir() {
            Toasts::about(
                Outcome::Warning,
                "toast-folder-missing",
                folder_name(&path),
                cx,
            );
            return;
        }
        if self
            .local_folders
            .iter()
            .any(|existing| overlaps(existing, &path))
        {
            return;
        }
        let mut folders = self.local_folders.clone();
        folders.insert(at.min(folders.len()), path);
        self.set_local_folders(folders, true, cx);
    }

    /// Rescans every configured local folder without changing the list, e.g. after files
    /// changed on disk or a tag was edited. A `thorough` rescan is the one the user asked for:
    /// it forgets what the last scan recorded, so every folder is listed and every file stat'd
    /// again, which is the only way an edit made behind Wisp's back is noticed.
    pub fn rescan_local(&mut self, thorough: bool, cx: &mut Context<Self>) {
        if thorough {
            self.local_provider.forget_scan();
        }
        self.set_local_folders(self.local_folders.clone(), thorough, cx);
    }

    /// Points the local library at `folders` and scans them. A scan already under way is
    /// cancelled first: its folders may be the ones just removed, and two scans would only
    /// fight over the same disk. An `asked` scan is one the user started, and its end or
    /// failure is reported in a toast.
    fn set_local_folders(&mut self, folders: Vec<PathBuf>, asked: bool, cx: &mut Context<Self>) {
        music::progress::cancel();
        self.local_revision = self.local_revision.wrapping_add(1);
        let revision = self.local_revision;
        self.scan_asked |= asked;
        if folders.is_empty() {
            self.scanning = false;
            self.scan_asked = false;
            self.local_provider.sign_out();
            self.local_folders = Vec::new();
            self.settings.update(cx, |settings, cx| {
                settings.set_local_folders(Vec::new(), cx)
            });
            self.local_client = None;
            self.local_tracks.clear();
            self.local_moves.clear();
            self.local_issues.clear();
            self.local_catalog = None;
            self.local_capabilities = Capabilities::NONE;
            self.local_task = None;
            cx.notify();
            cx.emit(SessionEvent::LocalChanged);
            return;
        }

        let provider = self.local_provider.clone();
        let chosen = folders.clone();
        let io = self.io.clone();
        let counting = self.scan_asked;
        self.scanning = true;
        cx.notify();
        self.local_task = Some(cx.spawn(async move |this, cx| {
            let prompt: PromptSink = Arc::new(|_| {});
            let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let signed_in = join(io.spawn(async move {
                let session = provider.sign_in(SignIn::Path(folders), prompt, rx).await?;
                let songs = match counting {
                    true => session
                        .api
                        .all_tracks()
                        .await
                        .map_or(0, |tracks| tracks.len()),
                    false => 0,
                };
                anyhow::Ok((session, songs))
            }))
            .await;

            this.update(cx, |this, cx| {
                if this.local_revision != revision {
                    return;
                }
                this.scanning = false;
                let asked = std::mem::take(&mut this.scan_asked);
                match signed_in {
                    Ok((session, songs)) => {
                        this.local_folders = chosen.clone();
                        this.settings
                            .update(cx, |settings, cx| settings.set_local_folders(chosen, cx));
                        this.local_signed_in(session, cx);
                        if asked && this.local_issues.is_empty() {
                            Scan::global(cx).update(cx, |scan, cx| scan.finished(songs, cx));
                        }
                    }
                    Err(error) => {
                        log::warn!("session: cannot update local music folders: {error:#}");
                        if asked {
                            Toasts::with_detail(
                                Outcome::Failed,
                                "toast-scan-failed",
                                None,
                                format!("{error:#}"),
                                cx,
                            );
                        }
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }

    /// Whether a local scan is under way right now.
    pub fn scanning(&self) -> bool {
        self.scanning
    }

    fn local_signed_in(&mut self, session: ProviderSession, cx: &mut Context<Self>) {
        self.local_tracks = session
            .api
            .local_tracks()
            .into_iter()
            .filter_map(|track| Some((track.id.clone()?, track)))
            .collect();
        self.local_moves = session.api.local_moves();
        self.local_issues = session.api.local_issues();
        if !self.local_issues.is_empty() {
            Toasts::post(
                Toast::new(Outcome::Warning, "toast-scan-issues")
                    .count(self.local_issues.len())
                    .detail(self.local_issues.join("\n"))
                    .open("settings-page-music-folders", Target::MusicFolders),
                cx,
            );
        }
        self.local_catalog = Some(Arc::new(CatalogSource::new(session.api.clone())));
        self.local_client = Some(session.api);
        if self.local_playback.is_none() {
            self.local_playback = Some(session.playback);
        }
        self.local_capabilities = session.capabilities;
        cx.notify();
        cx.emit(SessionEvent::LocalChanged);
    }
}

/// The folder's own name for a toast, or the whole path when it has none, such as `/`.
fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether `a` and `b` are the same directory, or one contains the other — either way, scanning
/// both would double-count the tracks they share.
fn overlaps(a: &Path, b: &Path) -> bool {
    let a = std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let b = std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    a == b || a.starts_with(&b) || b.starts_with(&a)
}
