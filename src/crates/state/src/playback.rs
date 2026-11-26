use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, SharedString, Task};
use logic::station::{
    self, FM_AHEAD, RADIO_LOOKAHEAD, Station, dry_after, ran_dry, runs_low, unheard,
};
use music::Repeat;
use music::speed::SpeedControl;
use music::vault::Vault;
use music::{
    Analyzer, Meter, MusicApi, PlaybackConfig, PlaybackEvent as BackendEvent, PlaybackEvents,
    PlaybackFactory, Player, Quality, RadioSeed, Spectrum, Track, TrackSignal, Transcode,
};
use ui::{Pin, PinKind};

/// Why the provider will play nothing more this session. Every later load fails the same way
/// without asking the engine again.
#[derive(Clone, Copy, PartialEq)]
enum Refusal {
    /// The provider refused the account and nothing plays this session.
    Keys,
    /// The provider streams only to a signed-in listener.
    SignIn,
}

/// How a load came about, which decides its debounce and whether the engine may keep its queue.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    /// The user chose a track.
    Pick,
    /// The user skipped once.
    Skip,
    /// The user is skipping repeatedly; the load waits for the skipping to stop.
    Burst,
    /// The queue moved on by itself, so a gapless engine keeps the tail of the last track.
    Segue,
    /// The provider turned the last load down for now and the same track is asked for again.
    Retry,
}

impl Start {
    fn debounce(self) -> Duration {
        match self {
            Self::Burst => SKIP_DEBOUNCE,
            Self::Pick | Self::Skip | Self::Segue | Self::Retry => Duration::ZERO,
        }
    }
}

/// What the user asked for last. It moves the moment they act, while `PlaybackState` waits for
/// the engine to confirm, so a control can follow the intent without waiting on the network.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Intent {
    Play,
    Pause,
}

use crate::queue::{Queue, QueuePlacement};
use crate::QueuePosition;
use serde::{Deserialize, Serialize};

use crate::{
    AppSettings, Equalizer, Io, Network, Outcome, Session, SessionEvent, SessionState, Target,
    Toast, Toasts,
};

const POSITION_INTERVAL: Duration = Duration::from_millis(500);
const CLOCK_SETTLE: Duration = Duration::from_secs(1);
/// How far a position report may be from the clock and still be eased in. A report further off
/// means the clock was wrong rather than drifting, as when an engine started somewhere other
/// than where it said, so the clock jumps to it.
const CLOCK_JUMP: Duration = Duration::from_secs(2);
/// Position reports that can still arrive from before a seek: one already in the channel and
/// one for a packet decoded before the engine read the command. A successful seek keeps the
/// engine busy, so more than this while one is in flight means it never left the old position.
const STALE_POSITIONS: u8 = 2;
/// How far before a seek target the engine may land and still be shown at the target. A decoder
/// settles on the packet holding the target, a few tens of milliseconds early at most; showing
/// the target keeps a lyric clicked at its first word on that line, not the one before.
const SEEK_SNAP: Duration = Duration::from_millis(100);
const PRELOAD_BEFORE_END: Duration = Duration::from_secs(10);
const SKIP_DEBOUNCE: Duration = Duration::from_millis(250);
const KEY_COOLDOWN: Duration = Duration::from_secs(1);
/// How long after a play or pause press an engine event saying the opposite is taken for the
/// echo of an earlier press rather than news. Past it the engine is believed again, so a pause
/// the engine makes on its own is not lost for good.
const ECHO_WINDOW: Duration = Duration::from_secs(2);
/// How many loads in a row may fail before the queue stops moving on by itself and waits for
/// play.
const FAILURE_LIMIT: u8 = 3;
/// The wait before a track the provider turned down for now is asked for again. Each refusal in
/// a row doubles it, up to `THROTTLE_CAP`.
const THROTTLE_WAIT: Duration = Duration::from_secs(2);
const THROTTLE_CAP: Duration = Duration::from_secs(30);
/// How many refusals in a row a track is retried through before it is held paused until play.
const THROTTLE_LIMIT: u8 = 6;
const RESUME_STEP: Duration = Duration::from_secs(5);
/// How far one volume notch moves the level, as a share of full volume. The volume wheel and the
/// volume keys both take it, so the two agree.
pub const VOLUME_STEP: f32 = 0.05;
/// How far the seek keys move through the track.
const SEEK_STEP: Duration = Duration::from_secs(5);
/// How long a pause may last before play reloads the track instead of resuming the engine. What
/// an engine holds goes stale while paused, and resuming it plays out the queued audio and then
/// reports the track as ended. Apple's license asks for renewal 13 minutes after it is issued,
/// and that clock starts at the load rather than the pause, so this leaves room for a long track.
const STALE_PAUSE: Duration = Duration::from_secs(5 * 60);
/// A level this low counts as muted, matching the player's volume button.
const MUTE_THRESHOLD: f32 = 0.001;
/// The level a mute button restores when the volume was already zero.
const DEFAULT_UNMUTE_VOLUME: f32 = 0.7;
const SIMILAR_LIMIT: usize = 20;

/// The remainder FM reports to KuGou. It treats a remainder of five or more as a full client
/// buffer and answers with nothing, so the station is asked with the 0–4 figure EchoMusic uses
/// for its FM buffer whatever the queue holds.
const FM_REMAIN: usize = 5;

/// How many unreadable files a toast names before it cuts the list short.
const LISTED_FILES: usize = 5;
/// How many tracks' signal reports are kept: the one playing, the one queued behind it, and
/// room for a skip that has not been heard yet.
const SIGNALS: usize = 4;
/// The FAQ entry on why a provider may refuse an account the keys to its audio.
/// How long the pointer rests on a play button before its collection is fetched, so a pointer
/// crossing a row of covers asks for none of them.
const WARM_DWELL: Duration = Duration::from_millis(40);
/// How long a fetched collection may start a press without being asked for again.
const WARM_TTL: Duration = Duration::from_secs(120);
/// Where a listener reads what to do about a provider that refuses this account its keys.
const KEYS_FAQ: &str = "https://github.com/9Vcc/WispMusic#readme";

/// A collection fetched for a play button, so a press on it need not wait for its tracks.
struct Warm {
    origin: Origin,
    /// The tracks, the radio continuation and when they arrived.
    loaded: Option<(Vec<Track>, Option<String>, Instant)>,
    /// The fetch while it runs unclaimed. A press moves it to the playback's `fetch`.
    fetch: Option<Task<()>>,
}

impl Warm {
    /// Whether a press on `origin` can use this, which takes a fetch still running or tracks
    /// that arrived recently.
    fn serves(&self, origin: &Origin) -> bool {
        let recent = self
            .loaded
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() < WARM_TTL);
        self.origin == *origin && (self.fetch.is_some() || recent)
    }
}

/// A track the engine was asked to fetch ahead, and whether it was also lined up to follow the
/// current one gaplessly.
struct Preloaded {
    id: String,
    segue: bool,
}

/// The position shown between the engine's reports. It runs on wall time from `reset` and is
/// nudged toward each report by `correct`, spread over a moment so the progress bar and the
/// lyrics glide instead of stepping. Parked, it holds `base`.
struct LiveClock {
    base: Duration,
    since: Option<Instant>,
    /// Seconds still to fold in from the last report, negative when the clock was ahead.
    correction: f64,
    /// Seconds over which the correction is folded in.
    settle: f64,
    /// How many seconds of the track one wall second carries, so the lyrics and the bar move
    /// with the voice at a changed speed instead of staying at 1x between reports.
    speed: f32,
}

impl LiveClock {
    fn new(speed: f32) -> Self {
        Self {
            base: Duration::ZERO,
            since: None,
            correction: 0.,
            settle: CLOCK_SETTLE.as_secs_f64(),
            speed,
        }
    }

    /// Rebases the clock on a new tempo, keeping the position it shows.
    fn retempo(&mut self, speed: f32) {
        if (self.speed - speed).abs() < f32::EPSILON {
            return;
        }
        let now = Instant::now();
        let at = self.at(now);
        self.base = at;
        self.since = self.since.map(|_| now);
        self.correction = 0.;
        self.settle = CLOCK_SETTLE.as_secs_f64();
        self.speed = speed;
    }

    /// Puts the clock at `at`, running from now or parked there.
    fn reset(&mut self, at: Duration, running: bool) {
        self.base = at;
        self.since = running.then(Instant::now);
        self.correction = 0.;
        self.settle = CLOCK_SETTLE.as_secs_f64();
    }

    /// Folds a reported position in without a jump.
    fn correct(&mut self, toward: Duration) {
        self.correct_at(toward, Instant::now());
    }

    fn correct_at(&mut self, toward: Duration, now: Instant) {
        let shown = self.at(now);
        self.since = Some(now);
        let gap = signed_gap(toward, shown);
        // Eased in, a gap of a minute would hold the clock nearly still for that minute.
        if gap.abs() > CLOCK_JUMP.as_secs_f64() {
            self.base = toward;
            self.correction = 0.;
            self.settle = CLOCK_SETTLE.as_secs_f64();
            return;
        }
        self.base = shown;
        self.correction = gap;
        // A negative correction is spread over longer than the discrepancy itself, which keeps
        // the clock moving forward while it converges instead of ever snapping back.
        self.settle = CLOCK_SETTLE.as_secs_f64() + self.correction.abs();
    }

    fn now(&self) -> Duration {
        self.at(Instant::now())
    }

    fn at(&self, now: Instant) -> Duration {
        let Some(since) = self.since else {
            return self.base;
        };
        let elapsed = now.saturating_duration_since(since).as_secs_f64();
        let blend = (elapsed / self.settle).clamp(0., 1.);
        shifted(
            self.base,
            elapsed * f64::from(self.speed) + self.correction * blend,
        )
    }
}

/// Seconds from `from` to `to`, negative when `to` is earlier.
fn signed_gap(to: Duration, from: Duration) -> f64 {
    match to >= from {
        true => (to - from).as_secs_f64(),
        false => -(from - to).as_secs_f64(),
    }
}

/// `base` moved by `seconds`, never below zero.
fn shifted(base: Duration, seconds: f64) -> Duration {
    match seconds >= 0. {
        true => base.saturating_add(Duration::from_secs_f64(seconds)),
        false => base.saturating_sub(Duration::from_secs_f64(-seconds)),
    }
}

/// [wisp] 音量增益与原版一致：线性。滑到多少就播多响——原版的 dB taper
/// 会把 15% 压到约 0.7% 的增益，几乎听不见。
fn gain(level: f32) -> f32 {
    level.clamp(0., 1.)
}

/// [wisp] dB 转线性因子，供前置放大与音量相乘。
fn preamp_factor(db: f32) -> f32 {
    10f32.powf(db / 20.)
}

/// What the engine has confirmed. `Playing` means audio is reaching the output; a track being
/// fetched, or a seek still buffering, reads `Loading`. What the user asked for lives in
/// `Intent` and can run ahead of this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,
    Playing,
    Paused,
    Loading,
    /// Nothing plays and this is why, in developer English.
    Failed(String),
}

/// Why a streaming track cannot play while the network is gone. Each reason is the Fluent key
/// of the toast that says so, and a row that cannot play shows the same words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unreachable {
    /// The network is gone.
    Offline,
}

impl Unreachable {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Offline => "toast-offline",
        }
    }

    fn toast(&self, cx: &mut App) {
        Toasts::show(Outcome::Failed, self.key(), cx);
    }
}

/// What other entities hear from `Playback`: history records a start, the sheet closes on an
/// end, and scrobbling tells the provider about every pause and seek.
pub enum PlaybackEvent {
    StartedPlayback,
    EndedPlayback,
    Paused,
    /// The engine landed a seek, playing or paused.
    Seeked,
}

/// Remembers the level silenced by a temporary mute, without saving it as a volume preference.
#[derive(Default)]
struct MuteMemory {
    restore: Option<f32>,
}

impl MuteMemory {
    /// Returns a temporary volume change for a mute request, preserving the first level silenced.
    fn change(&mut self, muted: bool, level: f32) -> Option<f32> {
        if muted {
            return match level > MUTE_THRESHOLD {
                true => {
                    self.restore = Some(level);
                    Some(0.)
                }
                false => None,
            };
        }
        match level > MUTE_THRESHOLD {
            true => {
                self.forget();
                None
            }
            false => {
                let restore = self.restore.take().unwrap_or(DEFAULT_UNMUTE_VOLUME);
                Some(restore)
            }
        }
    }

    /// Forgets the saved level when a volume change comes from another control.
    fn volume_changed(&mut self) {
        self.forget();
    }

    fn forget(&mut self) {
        self.restore = None;
    }
}

/// A one-shot request to pause after wall-clock time or when the current track ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sleep {
    After(Duration),
    EndOfTrack,
}

/// [wisp] The four choices of the player bar's play mode, cycled by its one button as the old
/// app did: play in order, loop the list, play at random, or repeat one song.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayMode {
    #[default]
    Sequential,
    List,
    Random,
    Single,
}

impl PlayMode {
    /// The tooltip's Fluent key.
    pub fn key(self) -> &'static str {
        match self {
            Self::Sequential => "player-sequential",
            Self::List => "player-repeat-all",
            Self::Random => "player-shuffle",
            Self::Single => "player-repeat-one",
        }
    }
}

/// What kind of collection a queued track came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Whence {
    Album,
    Playlist,
    Artist,
    /// The radio of a track, whose id the origin carries.
    Radio,
    /// The radio of a whole album, playlist or artist, whose id the origin carries.
    #[serde(rename = "collection-radio")]
    CollectionRadio(RadioSeed),
    Saved,
    Local,
}

/// The collection a queued track came from. While one of its tracks plays, its page shows a pause
/// button and the queue names it. Two origins are the same collection when kind and id match, and
/// the name is only for display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Origin {
    pub whence: Whence,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<SharedString>,
}

impl PartialEq for Origin {
    fn eq(&self, other: &Self) -> bool {
        self.whence == other.whence && self.id == other.id
    }
}

impl Eq for Origin {}

impl Origin {
    /// Whether `load` can fetch this collection from its provider. Local files are listed only
    /// by the library, which starts them from its own table.
    pub fn fetched(&self) -> bool {
        !matches!(self.whence, Whence::Local)
    }

    pub fn album(id: impl Into<String>) -> Self {
        Self::of(Whence::Album, id)
    }

    pub fn playlist(id: impl Into<String>) -> Self {
        Self::of(Whence::Playlist, id)
    }

    pub fn artist(id: impl Into<String>) -> Self {
        Self::of(Whence::Artist, id)
    }

    pub fn radio(id: impl Into<String>) -> Self {
        Self::of(Whence::Radio, id)
    }

    pub fn collection_radio(seed: RadioSeed, id: impl Into<String>) -> Self {
        Self::of(Whence::CollectionRadio(seed), id)
    }

    pub fn saved() -> Self {
        Self::of(Whence::Saved, String::new())
    }

    pub fn local() -> Self {
        Self::of(Whence::Local, String::new())
    }

    pub fn named(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Loads the collection once for playback or queue placement, retaining radio continuation.
    /// When `stations` is false the provider cannot seed a radio, so a song collection is just
    /// the track itself.
    pub(crate) async fn load(
        &self,
        client: &dyn MusicApi,
        stations: bool,
    ) -> Result<(Vec<Track>, Option<String>)> {
        let tracks = match self.whence {
            Whence::Album => client.album_tracks(&self.id).await?,
            Whence::Playlist => client.playlist_tracks(&self.id).await?,
            Whence::Artist => client.artist_tracks(&self.id).await?,
            Whence::Saved => client.saved_tracks().await?,
            Whence::Radio => {
                if !stations {
                    return Ok((vec![client.track(&self.id).await?], None));
                }
                let (mut tracks, next) = client.track_radio(&self.id, None).await?;
                tracks.retain(|track| track.playable);
                return Ok((tracks, next));
            }
            Whence::CollectionRadio(seed) => {
                if !stations {
                    anyhow::bail!("the provider seeds no radio")
                }
                let (mut tracks, next) =
                    station::collection_radio(client, seed, &self.id, None).await?;
                tracks.retain(|track| track.playable);
                return Ok((tracks, next));
            }
            Whence::Local => anyhow::bail!("local collections are supplied by the library"),
        };
        Ok((tracks, None))
    }

    /// The stretch of the station this origin names, from its start or from the continuation
    /// `from`. Only a radio is a station, and a collection's radio is blended from the track
    /// radios of its songs.
    pub(crate) async fn station(
        &self,
        client: &dyn MusicApi,
        stations: bool,
        from: Option<&str>,
    ) -> Result<(Vec<Track>, Option<String>)> {
        match self.whence {
            Whence::CollectionRadio(seed) => {
                if !stations {
                    anyhow::bail!("the provider seeds no radio")
                }
                station::collection_radio(client, seed, &self.id, from).await
            }
            _ => client.track_radio(&self.id, from).await,
        }
    }

    fn of(whence: Whence, id: impl Into<String>) -> Self {
        Self {
            whence,
            id: id.into(),
            name: None,
        }
    }
}

impl From<&Pin> for Origin {
    fn from(pin: &Pin) -> Self {
        let origin = match pin.kind {
            PinKind::Album => Origin::album(pin.id.clone()),
            PinKind::Playlist => Origin::playlist(pin.id.clone()),
            PinKind::Artist => Origin::artist(pin.id.clone()),
            PinKind::Song => Origin::radio(pin.id.clone()),
            PinKind::Liked => Origin::saved(),
        };
        origin.named(pin.title.clone())
    }
}

/// The last FM track that left the current slot, with the feedback KuGou's recommender reads.
#[derive(Clone, Debug)]
struct FmSeed {
    id: String,
    playtime: u32,
    overplay: bool,
}

/// Where files dropped on the queue go: `gap` upcoming tracks in, before the track whose queue
/// uid is `before`. The uid wins once the files are read, since the queue may have moved on.
struct DropAt {
    gap: usize,
    before: Option<String>,
}

impl DropAt {
    /// The gap to use now. Files dropped before a track that has since started or played go to
    /// the front of what is upcoming, and the recorded gap stands when the track is gone.
    fn resolve(&self, queue: &Queue) -> usize {
        let Some(before) = &self.before else {
            return self.gap;
        };
        match queue.position(before) {
            Some(crate::QueuePosition::Upcoming(at)) => at,
            Some(crate::QueuePosition::Current | crate::QueuePosition::Past(_)) => 0,
            None => self.gap,
        }
    }
}

/// A file handed in from the OS that could not be read as a track.
struct Unread {
    name: String,
    /// What the system said when the file was looked up, or `None` when the file is there but
    /// is not audio Wisp can read.
    error: Option<String>,
}

impl Unread {
    /// Names the file at `path` and keeps the error if it could not be reached at all.
    fn of(path: &Path) -> Self {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let error = std::fs::metadata(path).err().map(|error| error.to_string());
        Self { name, error }
    }
}

/// Notified on every position report, in place of `Playback`. It holds nothing; an observer
/// reads the time from `Playback`.
pub struct PositionTick;

/// Everything the UI knows about what is playing, and the only thing that drives an engine.
/// Holds one engine for the streaming provider and one for local files, a `Queue` for what
/// comes next, and a clock for the position between the engine's reports. Views act through
/// its methods; the engine answers through `on_backend_event`; nothing else touches a `Player`.
pub struct Playback {
    state: PlaybackState,
    /// The last position the engine reported, or the one asked of it.
    position: Duration,
    clock: LiveClock,
    /// Notified on each position report, which notifies nothing else.
    ticks: Entity<PositionTick>,
    track: Option<Track>,
    /// The provider whose factory built the streaming engine, so a switch to another provider
    /// knows the engine belongs to someone else.
    stream_slug: Option<String>,
    engine: Option<Box<dyn Player>>,
    local_engine: Option<Box<dyn Player>>,
    session: Entity<Session>,
    queue: Entity<Queue>,
    settings: Entity<AppSettings>,
    level: f32,
    /// [wisp] 前置放大（dB），叠加在音量之外的整体增益，改动即时生效。
    preamp_db: f32,
    mute: MuteMemory,
    /// The level the volume slider is being dragged to, heard and shown but not yet saved.
    sliding: Option<f32>,
    normalisation: bool,
    gapless: bool,
    transcode: Transcode,
    quality: Quality,
    /// 播放速度（Wisp 变速控件）
    speed: f32,
    /// 与音频线程共享的无重启速度控制
    speed_control: Arc<SpeedControl>,
    /// Whether tracks that played are cached in `vault`, so playing one again downloads nothing.
    keep_tracks: bool,
    /// How many gigabytes the cache in `vault` may take.
    cache_gb: u32,
    /// Where tracks are kept, shared with every engine.
    vault: Arc<Vault>,
    /// Reads the vault's master key from the keychain. The vault keeps nothing until it ends.
    _unlock: Task<()>,
    /// Evicts what no longer fits after the cache limit came down.
    resize: Option<Task<()>>,
    /// What the cache takes on disk, in bytes, as last measured.
    usage: Option<u64>,
    /// Measures the vault, or clears its cache and then measures it.
    measuring: Option<Task<()>>,
    /// The `state::Equalizer`'s reader, shared with every engine started here, so a change
    /// reaches the output without a restart.
    equalizer: music::Equalizer,
    /// Shared with every engine like the equalizer's reader, and read only while watched.
    meter: Meter,
    /// Shared with every engine like the meter, so the equalizer screen can hear the music ahead
    /// of the equalizer.
    analyzer: Analyzer,
    /// What the engines reported about the sound of the last few tracks they opened, newest
    /// last, for the signal path.
    signals: VecDeque<(String, TrackSignal)>,
    repeat: Repeat,
    radio: bool,
    /// [wisp] 本次播放是否来自私人 FM：随播放源切换而置位/清除，不写入设置。
    fm_session: bool,
    /// The FM mode and song pool last used by this session.
    fm_mode: String,
    fm_song_pool: u32,
    fm_seed: Option<FmSeed>,
    /// The shuffle preference an FM session has to put back when it ends.
    fm_shuffle: Option<bool>,
    /// A FM skip pressed while the queue had nothing playable, carried out once the refill lands.
    fm_pending_skip: bool,
    /// The last empty FM batch, so a dry station is retried at a human pace.
    station_empty_at: Option<Instant>,
    fm_retry: Option<Task<()>>,
    /// [wisp] 私人 FM 的单曲循环开关；只在 FM 会话里生效，与普通循环模式互不影响。
    fm_repeat: bool,
    /// The track the current similar-tracks suggestions were drawn from.
    seeded: Option<String>,
    /// The station the queue is playing, while the provider has more of it.
    station: Option<Station<Origin>>,
    /// The fetch of the station's next stretch; done once it lands, so one runs at a time.
    topping: Option<Task<()>>,
    /// Continuations in a row that brought no track the queue had not already heard.
    dry: u8,
    /// The streaming engine's event pump; dropping it stops listening.
    task: Option<Task<()>>,
    local_task: Option<Task<()>>,
    /// The pending load, held through its debounce; a newer load cancels it.
    load: Option<Task<()>>,
    /// The collection being fetched to start or extend the queue.
    fetch: Option<Task<()>>,
    /// The collection a press is waiting on and whether it asked for shuffle. It means
    /// something only while `fetch` holds the load.
    sought: Option<(Origin, bool)>,
    /// The collection last fetched for a play button, before or after its press.
    warm: Option<Warm>,
    enqueue: Option<Task<()>>,
    /// [wisp] Reads a track's full metadata before it starts, for a play request that only
    /// carries an id and a title (a pinned song, a link).
    resolve: Option<Task<()>>,
    /// Set while a resolved track is being loaded, so an answer that is still bare does not ask
    /// the provider in a loop.
    no_resolve: bool,
    /// [wisp] 正在取第一批私人漫游的曲子。
    roaming: Option<Task<()>>,
    suggest: Option<Task<()>>,
    /// The track the engine was asked to fetch ahead, so it is asked once.
    preloaded: Option<Preloaded>,
    /// When the user last skipped, for telling a burst of skips from one.
    skipped: Option<Instant>,
    /// No load goes out before this after a track failed, so a bad key cannot be hammered.
    blocked_until: Option<Instant>,
    /// Loads that failed since audio last played or the user last chose a track.
    failures: u8,
    /// Loads the provider turned down for now since audio last played or the user last picked a
    /// track. It sets how long the next retry waits.
    throttles: u8,
    /// When the engine last reported a pause. It is wall-clock time so a suspend counts toward
    /// `STALE_PAUSE`, and a clock set back counts as stale.
    paused_at: Option<SystemTime>,
    refused: Option<Refusal>,
    /// Where the restored track resumes. Set until the engine has it ready or the user plays.
    resume_at: Option<Duration>,
    /// The seek the engine is carrying out while playing. One is in flight at a time: the clock
    /// waits at its target until the engine reports audio from there, and position reports
    /// from before it are ignored.
    seek_in_flight: Option<Duration>,
    /// The latest seek asked for while one was in flight. It goes out when that one lands, so
    /// a burst of seeks costs the engine two rather than one per click.
    seek_next: Option<Duration>,
    /// Position reports seen while a seek is in flight.
    stale_positions: u8,
    /// Where the last seek asked to go, until the engine reports where it landed.
    seek_target: Option<Duration>,
    intent: Intent,
    /// When the user last pressed play or pause, until the engine reports the state they asked
    /// for. Engines answer each press in order, so a quick pause and play gets a late `Paused`
    /// after the play, which must not overwrite `intent`.
    pressed: Option<Instant>,
    /// Whether the engine holds the restored track paused at `resume_at`, so play is instant.
    resume_ready: bool,
    /// Whether playback stopped on a stale session and continues once it reconnects.
    awaiting_reconnect: bool,
    /// A track the listener asked to play while the provider session was still coming up, so
    /// the click is not lost.
    awaiting_start: Option<String>,
    /// The position last written to settings for resuming.
    stored: Duration,
    sleep: Option<Sleep>,
    sleep_task: Option<Task<()>>,
    /// The task still reading files, which the next batch waits for.
    pending_open: Option<Task<()>>,
    /// Set while the files of an open have not yet started anything, so the first readable
    /// batch of the burst plays even when it arrives as a tail.
    opening: bool,
}

impl EventEmitter<PlaybackEvent> for Playback {}

impl Playback {
    pub fn new(
        session: Entity<Session>,
        queue: Entity<Queue>,
        settings: Entity<AppSettings>,
        equalizer: &Entity<Equalizer>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Letting go of the engine stops it, and the app waits for that before it is gone.
        cx.on_app_quit(|this, cx| {
            this.task = None;
            this.engine = None;
            cx.background_executor().timer(Duration::ZERO)
        })
        .detach();
        cx.subscribe(&session, |this, session, event, cx| match event {
            SessionEvent::SignedIn => {
                let (playback, slug) = {
                    let session = session.read(cx);
                    (
                        session.playback(),
                        session.provider_slug().map(str::to_owned),
                    )
                };
                this.queue.update(cx, |queue, cx| queue.sync_provider(cx));
                let switched = this.stream_slug.is_some() && this.stream_slug != slug;
                if switched {
                    this.teardown(cx);
                }
                let Some(playback) = playback else {
                    this.refresh_local_metadata(cx);
                    return;
                };
                match this.engine.is_some() {
                    true => this.rebind(playback, cx),
                    false => this.start_engine(playback, cx),
                }
                this.adopt(cx);
                this.refresh_local_metadata(cx);
                if let Some(id) = this.awaiting_start.take() {
                    let track = this
                        .queue
                        .read(cx)
                        .current()
                        .cloned()
                        .filter(|track| track.id.as_deref() == Some(id.as_str()));
                    if let Some(track) = track {
                        this.load_from(&track, Duration::ZERO, Start::Pick, cx);
                    }
                }
            }
            SessionEvent::Reconnected => {
                let Some(playback) = session.read(cx).playback() else {
                    return;
                };
                this.rebind(playback, cx);
            }
            SessionEvent::SignedOut => this.teardown(cx),
            // The online engine gives way to the offline one, picking the current track up where it
            // was. A provider with no offline engine stops streaming altogether.
            SessionEvent::WentOffline => match session.read(cx).playback() {
                Some(playback) if this.local_active() => this.start_engine(playback, cx),
                Some(playback) => this.rebind(playback, cx),
                None => this.teardown(cx),
            },
            SessionEvent::LocalChanged => {
                if this.local_engine.is_none()
                    && let Some(playback) = session.read(cx).local_playback()
                {
                    this.start_local_engine(playback, cx);
                }
                this.refresh_local_metadata(cx);
            }
        })
        .detach();
        // An account out of reach still plays through an engine the session builds from the stored
        // sign-in alone.
        cx.observe(&session, |this, session, cx| {
            let session = session.read(cx);
            if this.engine.is_none()
                && matches!(session.state(), SessionState::Offline(_))
                && let Some(playback) = session.playback()
            {
                this.start_engine(playback, cx);
            }
        })
        .detach();
        cx.observe(&queue, |this, _, cx| {
            // FM tops up when a track starts and from the fetch it already asked for. Its
            // queue changes constantly as covers stack, so observing every change would ask
            // the same station over and over. A song radio still tops up on queue changes.
            if !this.fm_session {
                this.continue_station(cx);
            }
            this.suggest_similar(false, cx);
        })
        .detach();

        let level = settings.read(cx).volume();
        let preamp_db = settings.read(cx).preamp_db();
        let normalisation = settings.read(cx).normalisation();
        let gapless = settings.read(cx).gapless();
        // [wisp] 转码原本是某类自建服务端的能力，provider 裁剪后恒为默认（无转码）。
        let transcode = music::Transcode::default();
        let quality = settings.read(cx).quality();
        let speed = settings.read(cx).playback_speed();
        let speed_control = Arc::new(SpeedControl::default());
        let keep_tracks = settings.read(cx).keep_tracks();
        let cache_gb = settings.read(cx).cache_gb();
        // [wisp] 缓存目录可按设置自定义。
        let cache_dir = settings.read(cx).cache_dir().map(PathBuf::from);
        let vault = Arc::new(Vault::with_cache(cache_dir, crate::vault::bytes(cache_gb)));
        vault.set_caching(keep_tracks);
        let unlock = crate::vault::unlock(vault.clone(), cx);
        let (equalizer, meter) = (equalizer.read(cx).reader(), equalizer.read(cx).meter());
        let repeat = settings.read(cx).repeat();
        let radio = settings.read(cx).radio();

        let mut playback = Self {
            state: PlaybackState::Idle,
            position: Duration::ZERO,
            clock: LiveClock::new(speed),
            ticks: cx.new(|_| PositionTick),
            track: None,
            stream_slug: None,
            engine: None,
            local_engine: None,
            session,
            queue,
            settings,
            level,
            preamp_db,
            mute: MuteMemory::default(),
            sliding: None,
            normalisation,
            gapless,
            transcode,
            quality,
            speed,
            speed_control,
            keep_tracks,
            cache_gb,
            vault,
            _unlock: unlock,
            resize: None,
            usage: None,
            measuring: None,
            equalizer,
            meter,
            analyzer: Analyzer::default(),
            signals: VecDeque::new(),
            repeat,
            radio,
            fm_session: false,
            fm_mode: "normal".to_owned(),
            fm_song_pool: 0,
            fm_seed: None,
            fm_shuffle: None,
            fm_pending_skip: false,
            station_empty_at: None,
            fm_retry: None,
            fm_repeat: false,
            seeded: None,
            station: None,
            topping: None,
            dry: 0,
            task: None,
            local_task: None,
            load: None,
            fetch: None,
            sought: None,
            warm: None,
            enqueue: None,
            resolve: None,
            no_resolve: false,
            roaming: None,
            suggest: None,
            preloaded: None,
            skipped: None,
            blocked_until: None,
            failures: 0,
            throttles: 0,
            paused_at: None,
            refused: None,
            resume_at: None,
            seek_in_flight: None,
            seek_next: None,
            stale_positions: 0,
            seek_target: None,
            intent: Intent::Pause,
            pressed: None,
            resume_ready: false,
            awaiting_reconnect: false,
            awaiting_start: None,
            stored: Duration::ZERO,
            sleep: None,
            sleep_task: None,
            pending_open: None,
            opening: false,
        };
        // Start the local engine at startup so files can play before the first scan.
        if let Some(factory) = playback.session.read(cx).local_playback() {
            playback.start_local_engine(factory, cx);
        }
        playback
    }

    /// The active queue for every consumer.
    pub fn shown_queue(&self, cx: &App) -> crate::QueueView {
        crate::QueueView::new(
            self.queue.clone(),
            matches!(self.state(), PlaybackState::Playing),
            cx,
        )
    }

    /// The track the player shows.
    pub fn shown_track(&self, _cx: &App) -> Option<Track> {
        self.track.clone()
    }

    /// How far into the shown track playback is, and how long that track is.
    pub fn shown_time(&self, _cx: &App) -> (Duration, Duration) {
        let position = match self.state {
            PlaybackState::Playing => self.live_position(),
            _ => self.position,
        };
        (
            position,
            self.track
                .as_ref()
                .map_or(Duration::ZERO, |track| track.duration),
        )
    }

    /// The volume the controls show and change.
    pub fn shown_volume(&self, _cx: &App) -> f32 {
        self.sliding.unwrap_or(self.level)
    }

    /// Plays a track the user picked, from its start.
    pub fn play(&mut self, track: &Track, cx: &mut Context<Self>) {
        self.fm_session = false;
        self.restore_fm_shuffle(cx);
        self.load_after(track, Start::Pick, cx);
    }

    /// The engine a track id belongs to: local files have their own.
    fn engine_for(&self, id: &str) -> Option<&dyn Player> {
        match music::is_local_id(id) {
            true => self.local_engine.as_deref(),
            false => self.engine.as_deref(),
        }
    }

    fn active_engine(&self) -> Option<&dyn Player> {
        let id = self.track.as_ref()?.id.as_deref()?;
        self.engine_for(id)
    }

    /// The spectrum of the music on its way into the equalizer, on every output.
    pub fn analyzer(&self) -> &Analyzer {
        &self.analyzer
    }

    /// The playing engine's spectrum, set to hear the track before or after the volume as the
    /// visualizer setting asks.
    pub fn spectrum(&self, cx: &App) -> Option<Spectrum> {
        let spectrum = self.active_engine()?.spectrum()?;
        spectrum.set_absolute(self.settings.read(cx).visualizer_absolute());
        Some(spectrum)
    }

    /// For the signal path, `None` while no engine is up for the track loaded now. Otherwise
    /// what its engine reported about the track's sound, `None` inside until it opened it.
    pub fn track_signal(&self) -> Option<Option<&TrackSignal>> {
        let id = self.track.as_ref()?.id.as_deref()?;
        self.engine_for(id)?;
        Some(
            self.signals
                .iter()
                .rev()
                .find(|(known, _)| known == id)
                .map(|(_, track)| track),
        )
    }

    /// Keeps what an engine reported about the sound of track `id`, in place of any older
    /// report for it.
    fn learn_signal(&mut self, id: String, track: TrackSignal, cx: &mut Context<Self>) {
        self.signals.retain(|(known, _)| *known != id);
        if self.signals.len() == SIGNALS {
            self.signals.pop_front();
        }
        self.signals.push_back((id, track));
        cx.notify();
    }

    /// Pauses the engine the new track does not belong to, so the two never sound at once.
    fn silence_other(&self, id: &str) {
        let other = match music::is_local_id(id) {
            true => self.engine.as_deref(),
            false => self.local_engine.as_deref(),
        };
        if let Some(engine) = other {
            engine.pause();
        }
    }

    /// Fetches a track ahead, as when the pointer rests on its row, so playing it starts at
    /// once. Asked once per track; the current track is never preloaded.
    pub fn preload(&mut self, track: &Track) {
        self.preload_internal(track, false);
    }

    fn preload_internal(&mut self, track: &Track, segue: bool) {
        let Some(id) = track.id.as_deref() else {
            return;
        };
        if self.engine_for(id).is_none() {
            return;
        }
        if !track.playable || self.track.as_ref().and_then(|track| track.id.as_deref()) == Some(id)
        {
            return;
        }
        // A gapless join only ever happens inside one engine. Lining a track up in the engine
        // the current track does not play through would join it to whatever that engine last
        // held, and the load that follows would resume that track instead of this one.
        let segue = segue && music::is_local_id(id) == self.local_active();
        if self
            .preloaded
            .as_ref()
            .is_some_and(|held| held.id == id && (held.segue || !segue))
        {
            return;
        }
        self.preloaded = Some(Preloaded {
            id: id.to_owned(),
            segue,
        });
        let Some(engine) = self.engine_for(id) else {
            return;
        };
        if let Err(error) = engine.preload(id, segue) {
            self.preloaded = None;
            log::warn!("playback: cannot preload {}: {error:#}", track.name);
        }
    }

    fn load_after(&mut self, track: &Track, start: Start, cx: &mut Context<Self>) {
        self.load_from(track, Duration::ZERO, start, cx);
    }

    /// [wisp] Plays a track the caller may only have an id and a title for (a pinned song, a
    /// link). The provider's copy fills in the artists, duration and cover before anything is
    /// heard, so the player bar, the total time and the lyrics know what is playing. A track
    /// that already carries metadata, one the provider cannot be asked about, or one it cannot
    /// answer for falls back to playing what was handed in.
    /// [wisp] 开始私人漫游：取一批私人 FM 入队从第一首播，并确保电台续歌开着——
    /// 播完这一批队尾会自动补下一批（首页与 FM 页共用的入口）。
    pub fn start_roaming(&mut self, cx: &mut Context<Self>) {
        // 私人 FM 入口：这批推荐算一次新的 FM 会话。
        let Some(client) = self.session.read(cx).client() else {
            return;
        };
        let mode = self.settings.read(cx).fm_mode().to_owned();
        let pool = self.settings.read(cx).fm_song_pool();
        let request_mode = mode.clone();
        self.roaming = Some(Io::global(cx).load(
            async move {
                let (tracks, next) = client
                    .track_radio_with("", None, Some(&request_mode), Some(pool))
                    .await?;
                anyhow::Ok((tracks, next))
            },
            move |this, loaded, cx| {
                this.roaming = None;
                match loaded {
                    Ok((tracks, next)) if !tracks.is_empty() => {
                        this.start_fm(tracks, 0, next, mode, pool, cx);
                    }
                    Ok(_) => log::warn!("playback: radio returned no tracks"),
                    Err(error) => log::warn!("playback: cannot start roaming: {error:#}"),
                }
            },
            cx,
        ));
    }

    pub fn play_resolved(&mut self, track: &Track, cx: &mut Context<Self>) {
        self.fm_session = false;
        self.restore_fm_shuffle(cx);
        let Some(id) = track.id.clone() else {
            return self.failed(format!("{} has no track id", track.name), cx);
        };
        let bare = track.artists.is_empty() && track.duration.is_zero();
        let Some(client) = self.session.read(cx).client().filter(|_| bare) else {
            return self.load_after(track, Start::Pick, cx);
        };
        let fallback = track.clone();
        let wanted = id.clone();
        self.resolve = Some(Io::global(cx).load(
            async move { client.track(&wanted).await },
            move |this, found, cx| {
                this.resolve = None;
                let track = match found {
                    Ok(track) if track.id.as_deref() == Some(id.as_str()) => track,
                    Ok(_) => {
                        log::warn!("playback: the provider answered with another track for {id}");
                        fallback
                    }
                    Err(error) => {
                        log::warn!("playback: cannot read the track {id}: {error:#}");
                        fallback
                    }
                };
                this.no_resolve = true;
                this.load_after(&track, Start::Pick, cx);
                this.no_resolve = false;
            },
            cx,
        ));
    }

    /// Loads a track to play from `at`, after the debounce `start` asks for. The state reads
    /// Loading from here until the engine reports audio; a refusal or an unplayable track fails
    /// without reaching the engine.
    fn load_from(&mut self, track: &Track, at: Duration, start: Start, cx: &mut Context<Self>) {
        let Some(id) = track.id.clone() else {
            return self.failed(format!("{} has no track id", track.name), cx);
        };
        let is_local = music::is_local_id(&id);
        if !is_local {
            match self.refused {
                Some(Refusal::Keys) => return self.refuse(cx),
                Some(Refusal::SignIn) => return self.gate(cx),
                None => {}
            }
        }
        if !track.playable {
            return self.failed(format!("{} is not available to stream", track.name), cx);
        }
        if let Some(reason) = self.reason(&id, cx) {
            return Self::unreachable(reason, start, cx);
        }
        let bare = track.artists.is_empty() && track.duration.is_zero();
        if bare
            && !is_local
            && !self.no_resolve
            && self.resolve.is_none()
            && let Some(client) = self.session.read(cx).client()
        {
            let fallback = track.clone();
            let wanted = id.clone();
            self.resolve = Some(Io::global(cx).load(
                async move { client.track(&wanted).await },
                move |this, found, cx| {
                    this.resolve = None;
                    let track = match found {
                        Ok(track) if track.id.as_deref() == Some(id.as_str()) => track,
                        Ok(_) => {
                            log::warn!(
                                "playback: the provider answered with another track for {id}"
                            );
                            fallback
                        }
                        Err(error) => {
                            log::warn!("playback: cannot read the track {id}: {error:#}");
                            fallback
                        }
                    };
                    this.queue
                        .update(cx, |queue, cx| queue.replace_current(&track, cx));
                    this.no_resolve = true;
                    this.load_from(&track, at, start, cx);
                    this.no_resolve = false;
                },
                cx,
            ));
            return;
        }
        if self.engine_for(&id).is_none() {
            if start == Start::Pick {
                self.awaiting_start = Some(id);
            }
            return;
        }
        if self.awaiting_start.as_deref() == Some(id.as_str()) {
            self.awaiting_start = None;
        }
        self.silence_other(&id);
        if !matches!(start, Start::Segue | Start::Retry) {
            self.failures = 0;
        }
        if start == Start::Pick {
            self.throttles = 0;
        }

        self.track = Some(track.clone());
        music::builtin::remember(track);
        self.state = PlaybackState::Loading;
        self.position = at;
        self.clock.reset(at, false);
        self.preloaded = None;
        self.resume_at = None;
        self.seek_in_flight = None;
        self.seek_next = None;
        self.seek_target = None;
        self.intent = Intent::Play;
        self.resume_ready = false;
        cx.notify();

        let wait = self
            .blocked_until
            .and_then(|until| until.checked_duration_since(Instant::now()))
            .unwrap_or_default()
            .max(start.debounce());

        self.load = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                if this.intent == Intent::Pause {
                    return this.hold(at, cx);
                }
                let Some(engine) = this.engine_for(&id) else {
                    return;
                };
                if let Err(error) = engine.load(&id, at, start == Start::Segue) {
                    let reason = crate::blamed(&error, cx);
                    this.failed(reason, cx);
                }
            })
            .ok();
        }));
        if self.fm_session {
            // One look when a track starts; the fetch then keeps topping itself up to the
            // FM window, rather than reacting to every queue mutation along the way.
            self.continue_station(cx);
        }
    }

    /// Replaces the queue with `tracks` and plays the one at `index`.
    pub fn start(
        &mut self,
        tracks: Vec<Track>,
        index: usize,
        origin: Option<Origin>,
        cx: &mut Context<Self>,
    ) {
        self.fm_session = false;
        self.restore_fm_shuffle(cx);
        self.fetch = None;
        self.begin(tracks, index, origin, cx);
    }

    /// [wisp] 从私人 FM 开始一批推荐：这次播放属于 FM 会话，循环键显示 ∞。
    /// `next` continues the station from the last track the provider answered, so the queue
    /// refills instead of stopping at the end of the first batch.
    pub fn start_fm(
        &mut self,
        tracks: Vec<Track>,
        index: usize,
        next: Option<String>,
        mode: String,
        song_pool: u32,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "playback: fm start with {} tracks, continuation={:?}, mode={mode}, pool={song_pool}",
            tracks.len(),
            next
        );
        self.start(tracks, index, None, cx);
        self.enter_fm_shuffle(cx);
        self.fm_session = true;
        self.fm_mode = mode;
        self.fm_song_pool = song_pool;
        self.fm_seed = None;
        self.station_empty_at = None;
        self.fm_retry = None;
        self.station = next.map(|next| Station {
            origin: Origin::radio(String::new()),
            next,
        });
        self.continue_station(cx);
        cx.notify();
    }

    /// Replaces the queue with `tracks` and plays the first playable one, or a random one when
    /// shuffle is on.
    pub fn start_any(
        &mut self,
        tracks: Vec<Track>,
        origin: Option<Origin>,
        cx: &mut Context<Self>,
    ) {
        let index = self.opener(&tracks, cx).unwrap_or_default();
        self.start(tracks, index, origin, cx);
    }

    /// Turn shuffle on and start the tracks from a random playable one.
    pub fn shuffle_any(
        &mut self,
        tracks: Vec<Track>,
        origin: Option<Origin>,
        cx: &mut Context<Self>,
    ) {
        self.queue
            .update(cx, |queue, cx| queue.set_shuffle(true, cx));
        self.start_any(tracks, origin, cx);
    }

    /// Which of `tracks` a collection opens with.
    fn opener(&self, tracks: &[Track], cx: &Context<Self>) -> Option<usize> {
        let playable = tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| track.playable && self.unreachable_reason(track, cx).is_none())
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match self.queue.read(cx).shuffle() {
            true => fastrand::choice(&playable).copied(),
            false => playable.first().copied(),
        }
    }

    /// Plays `seed` now and fills the queue behind it with its radio once that arrives.
    pub fn play_radio(&mut self, seed: &Track, cx: &mut Context<Self>) {
        self.fm_session = false;
        self.restore_fm_shuffle(cx);
        let Some(id) = seed.id.clone() else {
            return self.failed(format!("{} has no track id", seed.name), cx);
        };
        if !seed.playable {
            return self.failed(format!("{} is not available to stream", seed.name), cx);
        }

        let origin = Origin::radio(id.clone()).named(seed.name.clone());
        let Some(client) = self.client_for(&id, cx) else {
            return;
        };

        self.fetch = None;
        if !self.begin(vec![seed.clone()], 0, Some(origin.clone()), cx) {
            return;
        }

        let seed_id = seed.id.clone();
        let title = seed.name.clone();
        self.sought = None;
        self.fetch = Some(Io::global(cx).load(
            async move {
                let (mut tracks, next) = client.track_radio(&id, None).await?;
                tracks.retain(|track| track.id != seed_id && track.playable);
                Ok((tracks, next))
            },
            move |this, loaded, cx| match loaded {
                Ok((tracks, next)) if this.origin(cx) == Some(&origin) => {
                    this.station = next.map(|next| Station {
                        origin: origin.clone(),
                        next,
                    });
                    this.queue
                        .update(cx, |queue, cx| queue.extend_context(tracks, origin, cx));
                }
                Ok(_) => {}
                Err(error) => {
                    log::error!("playback: cannot load radio queue: {error:#}");
                    crate::noted(&error, cx);
                    if this.origin(cx) == Some(&origin) {
                        Toasts::about(Outcome::Failed, "toast-radio-failed", title, cx);
                    }
                }
            },
            cx,
        ));
    }

    /// Appends a track, or starts playing it when nothing is queued.
    pub fn enqueue(&mut self, track: Track, cx: &mut Context<Self>) {
        self.enqueue_tracks(vec![track], None, QueuePlacement::End, cx);
    }

    /// Whether the active player can honour a local queue placement other than appending.
    pub fn can_order_queue(&self) -> bool {
        self.active_engine().is_none_or(Player::can_order_queue)
    }

    pub fn play_next(&mut self, track: Track, cx: &mut Context<Self>) {
        self.enqueue_tracks(vec![track], None, QueuePlacement::Next, cx);
    }

    /// Queues a track after everything already lined up, or starts playing it when nothing is.
    pub fn play_last(&mut self, track: Track, cx: &mut Context<Self>) {
        self.enqueue_tracks(vec![track], None, QueuePlacement::Last, cx);
    }

    /// Places tracks the user picked and says where they went, naming one track and counting
    /// several. Nothing is said when they start playback instead, since playing is the answer.
    pub fn enqueue_tracks(
        &mut self,
        tracks: Vec<Track>,
        origin: Option<Origin>,
        placement: QueuePlacement,
        cx: &mut Context<Self>,
    ) {
        let count = tracks.len();
        let named = match tracks.as_slice() {
            [only] => Some((only.name.clone(), song_target(only))),
            _ => None,
        };
        if count == 0 {
            return;
        }
        let Some(placed) = self.place(tracks, origin, placement, cx) else {
            return;
        };
        match named {
            Some((name, target)) => {
                if let Some(key) = placed.toast("track") {
                    Toasts::linked(Outcome::Done, key, name, target, cx);
                }
            }
            None => {
                if let Some(key) = placed.toast("tracks") {
                    Toasts::post(Toast::new(Outcome::Done, key).count(count), cx);
                }
            }
        }
    }

    /// Puts tracks into the queue at `placement` without a word, or starts playing them when
    /// nothing is queued. While another device has playback they go to the end of its queue
    /// instead. Returns where they went, for the toast, or `None` when they started playback or
    /// went nowhere.
    fn place(
        &mut self,
        tracks: Vec<Track>,
        origin: Option<Origin>,
        placement: QueuePlacement,
        cx: &mut Context<Self>,
    ) -> Option<QueuePlacement> {
        let playing = self.queue.read(cx).current().is_some();
        if tracks.is_empty() {
            return playing.then_some(placement);
        }
        if !playing {
            self.begin(tracks, 0, origin, cx);
            return None;
        }
        let tracks = tracks.into_iter().map(|mut track| {
            track.added_by = None;
            track
        });
        self.queue
            .update(cx, |queue, cx| queue.insert(tracks, origin, placement, cx));
        Some(placement)
    }

    /// Opens paths handed in from the OS (a file-association launch or hand-off). They join the
    /// end of the manual queue in the order they were given. The first batch of a burst starts
    /// playing at once and skips what was queued by hand before it. A `tail` batch only joins,
    /// unless no batch of the burst has started anything yet or nothing is playing.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, tail: bool, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.resolve_paths(paths, None, tail, cx);
    }

    /// Queues paths dropped on the queue at `gap` upcoming tracks in, or at the end of the queue.
    pub fn enqueue_paths(
        &mut self,
        paths: Vec<PathBuf>,
        gap: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let gap = gap.unwrap_or(usize::MAX);
        let before = self
            .queue
            .read(cx)
            .uid(crate::QueuePosition::Upcoming(gap))
            .map(str::to_owned);
        self.resolve_paths(paths, Some(DropAt { gap, before }), false, cx);
    }

    /// Reads the files on the tokio runtime and plays what it could read. A later batch waits for
    /// the one before it, so tracks reach the queue in the order they came. Files it could not read
    /// are reported on a toast, as a failure when none could be read and as a warning otherwise.
    fn resolve_paths(
        &mut self,
        paths: Vec<PathBuf>,
        drop: Option<DropAt>,
        tail: bool,
        cx: &mut Context<Self>,
    ) {
        let provider = self.session.read(cx).local_provider();
        let previous_open = self.pending_open.take();
        self.pending_open = Some(Io::global(cx).load(
            async move {
                if let Some(previous) = previous_open {
                    previous.await;
                }
                let mut tracks = Vec::new();
                let mut unread = Vec::new();
                for path in paths {
                    match provider.track_from_path(&path) {
                        Some(track) => tracks.push(track),
                        None => {
                            log::warn!("local: cannot open {}", path.display());
                            unread.push(Unread::of(&path));
                        }
                    }
                }
                anyhow::Ok((tracks, unread))
            },
            move |this, loaded, cx| match loaded {
                Ok((tracks, unread)) => {
                    if drop.is_none() && !tail {
                        this.opening = true;
                    }
                    report_unread(&unread, tracks.is_empty(), cx);
                    if tracks.is_empty() {
                        return;
                    }
                    match drop {
                        Some(drop) => {
                            let gap = drop.resolve(this.queue.read(cx));
                            this.insert_all(tracks, gap, None, cx);
                        }
                        None => {
                            let play = this.opening || this.track.is_none();
                            let at = this.queue.read(cx).manual_end();
                            let first = tracks.iter().position(|track| track.playable);
                            this.queue.update(cx, |queue, cx| {
                                queue.insert(tracks, None, QueuePlacement::End, cx)
                            });
                            if play && let Some(first) = first {
                                this.opening = false;
                                this.fetch = None;
                                let track = this.queue.update(cx, |queue, cx| {
                                    queue.select(crate::QueuePosition::Upcoming(at + first), cx)
                                });
                                if let Some(track) = track {
                                    this.load_after(&track, Start::Pick, cx);
                                }
                            }
                        }
                    }
                }
                Err(error) => log::warn!("playback: cannot open files: {error:#}"),
            },
            cx,
        ));
    }

    /// Inserts tracks `gap` places into the upcoming list without a word, or starts playing
    /// them when nothing is queued.
    pub fn insert_all(
        &mut self,
        tracks: Vec<Track>,
        gap: usize,
        origin: Option<Origin>,
        cx: &mut Context<Self>,
    ) {
        self.place(tracks, origin, QueuePlacement::Gap(gap), cx);
    }

    /// Fetches a pinned collection and inserts it `gap` tracks ahead, or at the end. A pinned
    /// song goes in on its own, without an origin.
    pub fn enqueue_pin(&mut self, pin: &Pin, gap: Option<usize>, cx: &mut Context<Self>) {
        let origin = (pin.kind != PinKind::Song).then(|| Origin::from(pin));
        self.enqueue_from(
            &pin.id,
            origin,
            QueuePlacement::Gap(gap.unwrap_or(usize::MAX)),
            cx,
        );
    }

    /// Fetches any collection through the same queue placement path.
    pub fn enqueue_collection(
        &mut self,
        origin: Origin,
        placement: QueuePlacement,
        cx: &mut Context<Self>,
    ) {
        let id = origin.id.clone();
        self.enqueue_from(&id, Some(origin), placement, cx);
    }

    fn client_for(&self, id: &str, cx: &Context<Self>) -> Option<Arc<dyn MusicApi>> {
        let session = self.session.read(cx);
        match music::is_local_id(id) {
            true => session.local_client(),
            false => session.client(),
        }
    }

    /// Whether the provider that owns `id` can seed a station. Without one there is nothing to
    /// keep a queue going, so the radio paths leave it alone rather than asking and getting
    /// nothing back.
    fn stations(&self, id: &str, cx: &Context<Self>) -> bool {
        self.session
            .read(cx)
            .capabilities_of(crate::Shelf::of(id))
            .radio
    }

    /// Fetches tracks for the queue on the tokio runtime and places them on arrival, with a
    /// toast once they land or the fetch fails. One fetch runs at a time, and a second request
    /// while one runs is dropped before it shows anything. The tracks keep `origin` as the
    /// collection they were queued from.
    fn enqueue_from(
        &mut self,
        id: &str,
        origin: Option<Origin>,
        placement: QueuePlacement,
        cx: &mut Context<Self>,
    ) {
        if self.enqueue.is_some() {
            return;
        }
        let Some(client) = self.client_for(id, cx) else {
            return;
        };
        let source = match origin.as_ref().map(|origin| origin.whence) {
            Some(Whence::Album) => "album",
            Some(Whence::Playlist) => "playlist",
            Some(Whence::Artist) => "artist",
            _ => "track",
        };
        let id = id.to_owned();
        let from = origin.clone();
        let stations = self.stations(&id, cx);
        self.enqueue = Some(Io::global(cx).load(
            async move {
                match from {
                    Some(origin) => origin
                        .load(&*client, stations)
                        .await
                        .map(|(tracks, _)| tracks),
                    None => client.track(&id).await.map(|track| vec![track]),
                }
            },
            move |this, loaded, cx| {
                this.enqueue = None;
                match loaded {
                    Ok(tracks) => {
                        let placed = this.place(tracks, origin, placement, cx);
                        if let Some(key) = placed.and_then(|placed| placed.toast(source)) {
                            Toasts::show(Outcome::Done, key, cx);
                        }
                    }
                    Err(error) => {
                        log::error!("playback: cannot enqueue {source}: {error:#}");
                        Toasts::show(Outcome::Failed, "toast-queue-failed", cx);
                    }
                }
            },
            cx,
        ));
    }

    /// The collection the current track was queued from. `None` when the user added it on its
    /// own.
    pub fn origin<'a>(&self, cx: &'a App) -> Option<&'a Origin> {
        self.queue.read(cx).origin()
    }

    /// What a play button for `origin` shows, read like `control`. `None` when the current track
    /// came from somewhere else, so pressing it starts `origin` afresh.
    pub fn playing_from(&self, origin: &Origin, cx: &App) -> Option<bool> {
        match self.origin(cx) == Some(origin) {
            true => self.control(),
            false => None,
        }
    }

    /// Whether a play button for `origin` shows pause: its tracks play, or a press on it is
    /// still fetching them. The second half is what answers the press in the frame it lands.
    pub fn engaged(&self, origin: &Origin, cx: &App) -> bool {
        self.playing_from(origin, cx) == Some(true) || self.awaits(origin)
    }

    /// Whether a press on `origin` is waiting for its tracks.
    fn awaits(&self, origin: &Origin) -> bool {
        self.fetch.is_some()
            && self
                .sought
                .as_ref()
                .is_some_and(|(sought, _)| sought == origin)
    }

    /// Fetches `origin` ahead of a press, as when the pointer rests on its play button, so the
    /// press starts it at once. One collection is kept at a time. Asking again for the one held
    /// or the one playing does nothing.
    pub fn warm_origin(&mut self, origin: &Origin, cx: &mut Context<Self>) {
        if !origin.fetched() {
            return;
        }
        if self.origin(cx) == Some(origin) || self.awaits(origin) {
            return;
        }
        if self.warm.as_ref().is_some_and(|warm| warm.serves(origin)) {
            return;
        }
        let Some(client) = self.client_for(&origin.id, cx) else {
            return;
        };
        self.warm = Some(Warm {
            origin: origin.clone(),
            loaded: None,
            fetch: Some(self.collect(origin.clone(), client, WARM_DWELL, cx)),
        });
    }

    /// Fetches the tracks of `origin` after `dwell` and hands them to `collected`.
    fn collect(
        &self,
        origin: Origin,
        client: Arc<dyn MusicApi>,
        dwell: Duration,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let io = Io::global(cx);
        let stations = self.stations(&origin.id, cx);
        cx.spawn(async move |this, cx| {
            if !dwell.is_zero() {
                cx.background_executor().timer(dwell).await;
            }
            let from = origin.clone();
            let loaded =
                crate::join(io.spawn(async move { from.load(&*client, stations).await })).await;
            this.update(cx, |this, cx| this.collected(origin, loaded, cx))
                .ok();
        })
    }

    /// Takes the tracks of `origin` once they arrive. A press that was waiting on them starts
    /// the queue. With no press they are kept for one, and the track a press would open is
    /// fetched ahead too.
    fn collected(
        &mut self,
        origin: Origin,
        loaded: Result<(Vec<Track>, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        let pressed = match self.awaits(&origin) {
            true => self.sought.take().map(|(_, shuffled)| shuffled),
            false => None,
        };
        let held = self.warm.as_ref().is_some_and(|warm| warm.origin == origin);
        match loaded {
            Ok((tracks, next)) => {
                let kept = Some((tracks.clone(), next.clone(), Instant::now()));
                match &mut self.warm {
                    Some(warm) if held => {
                        warm.loaded = kept;
                        warm.fetch = None;
                    }
                    // another collection is being fetched for the pointer, so this one is
                    // not kept
                    Some(warm) if warm.fetch.is_some() => {}
                    _ => {
                        self.warm = Some(Warm {
                            origin: origin.clone(),
                            loaded: kept,
                            fetch: None,
                        })
                    }
                }
                match pressed {
                    Some(shuffled) => self.arrive(origin, tracks, next, shuffled, cx),
                    None if !self.queue.read(cx).shuffle() => {
                        if let Some(index) = self.opener(&tracks, cx) {
                            self.preload(&tracks[index]);
                        }
                    }
                    None => {}
                }
            }
            Err(error) => {
                if held {
                    self.warm = None;
                }
                match pressed {
                    None => log::debug!("playback: cannot fetch a collection ahead: {error:#}"),
                    Some(_) if self.has_active_playback() => {
                        log::error!("playback: cannot load context: {error:#}");
                        cx.notify();
                    }
                    Some(_) => {
                        let reason = crate::blamed(&error, cx);
                        self.failed(reason, cx);
                    }
                }
            }
        }
    }

    /// Starts the queue from the fetched tracks of `origin`, shuffled when the press asked.
    fn arrive(
        &mut self,
        origin: Origin,
        tracks: Vec<Track>,
        next: Option<String>,
        shuffled: bool,
        cx: &mut Context<Self>,
    ) {
        if shuffled && !tracks.is_empty() {
            self.queue
                .update(cx, |queue, cx| queue.set_shuffle(true, cx));
        }
        let index = self.opener(&tracks, cx).unwrap_or_default();
        let station = next.map(|next| Station {
            origin: origin.clone(),
            next,
        });
        if self.begin(tracks, index, Some(origin), cx) {
            self.station = station;
        }
    }

    /// Drops the station the queue was playing, continuation and fetch alike, once the queue
    /// is something else.
    fn leave_station(&mut self, cx: &mut Context<Self>) {
        self.restore_fm_shuffle(cx);
        self.station = None;
        self.topping = None;
        self.dry = 0;
        self.fm_pending_skip = false;
        self.station_empty_at = None;
        self.fm_retry = None;
    }

    /// FM always plays its covers in order; the shuffle of the queue it replaced is put back
    /// once FM is left.
    fn enter_fm_shuffle(&mut self, cx: &mut Context<Self>) {
        let before = self.queue.read(cx).shuffle();
        self.fm_shuffle = Some(before);
        if before {
            log::info!("playback: fm plays in cover order, leaving shuffle for after the session");
            self.queue
                .update(cx, |queue, cx| queue.force_shuffle(false, cx));
        }
    }

    fn restore_fm_shuffle(&mut self, cx: &mut Context<Self>) {
        if let Some(shuffle) = self.fm_shuffle.take() {
            self.queue
                .update(cx, |queue, cx| queue.force_shuffle(shuffle, cx));
        }
    }

    /// Hands a fetched collection to the queue, every track tagged with `origin`, and plays the
    /// chosen one. A chosen track that cannot play here while the network is gone leaves the
    /// queue alone too, and says why.
    fn begin(
        &mut self,
        tracks: Vec<Track>,
        index: usize,
        origin: Option<Origin>,
        cx: &mut Context<Self>,
    ) -> bool {
        // A track that cannot play here leaves the queue and whatever plays as they were.
        if let Some(reason) = tracks
            .get(index)
            .and_then(|track| self.unreachable_reason(track, cx))
        {
            Self::unreachable(reason, Start::Pick, cx);
            return false;
        }
        let Some(track) = self
            .queue
            .update(cx, |queue, cx| queue.start(tracks, index, origin, cx))
        else {
            return true;
        };
        self.leave_station(cx);
        self.play(&track, cx);
        true
    }

    /// Starts the queue from a collection, keeping the continuation if it is a station. Tracks
    /// fetched ahead by `warm_origin` start it at once, and a fetch still running is waited on
    /// rather than repeated. Shows Loading only when nothing plays yet, so a failure
    /// mid-playback is logged rather than shown.
    fn gather(&mut self, origin: Origin, shuffled: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client_for(&origin.id, cx) else {
            return;
        };
        if self.awaits(&origin) {
            return;
        }
        let warm = self.warm.as_mut().filter(|warm| warm.serves(&origin));
        if let Some((tracks, next, _)) = warm.as_ref().and_then(|warm| warm.loaded.clone()) {
            self.fetch = None;
            self.sought = None;
            return self.arrive(origin, tracks, next, shuffled, cx);
        }
        let running = warm.and_then(|warm| warm.fetch.take());
        if !self.has_active_playback() {
            self.state = PlaybackState::Loading;
        }
        self.fetch = Some(match running {
            Some(running) => running,
            None => self.collect(origin.clone(), client, Duration::ZERO, cx),
        });
        self.sought = Some((origin, shuffled));
        cx.notify();
    }

    /// Skips to the next playable track.
    pub fn next(&mut self, cx: &mut Context<Self>) {
        // 私人 FM 的单曲循环：手动下一首也重播当前曲，和旧版行为一致。
        if self.fm_session && self.fm_repeat {
            self.seek(Duration::ZERO, cx);
            self.resume(cx);
            return;
        }
        if self.fm_session {
            self.remember_fm_seed(false);
        }
        if self.fm_session && (!self.queue.read(cx).has_next() || !self.fm_has_playable_ahead(cx)) {
            // An empty or all-unplayable FM queue is not the end of playback: ask the
            // recommender for the next stretch and keep the session alive.
            log::info!(
                "playback: fm next with no playable upcoming, topping={}",
                self.topping.is_some()
            );
            self.fm_pending_skip = true;
            self.fetch = None;
            if self.topping.is_none() {
                self.continue_station(cx);
            }
            return;
        }
        self.fetch = None;
        let start = self.burst();
        // [wisp] Next on the last song of a looping queue wraps like the end of the song does;
        // random play loops too, as it never has a last song, and so does repeat-one's manual
        // next, as the old app wrapped it back to the front.
        if (matches!(self.repeat, Repeat::All | Repeat::One) || self.queue.read(cx).shuffle())
            && self.queue.read(cx).ahead() == 0
        {
            let track = match self.queue.read(cx).shuffle() {
                true => self.queue.update(cx, |queue, cx| queue.rewind_ordered(cx)),
                false => self.queue.update(cx, |queue, cx| queue.rewind(cx)),
            };
            if let Some(track) = track {
                self.follow_after(track, start, cx);
            }
            return;
        }
        if !self.queue.read(cx).has_next() && self.topping.is_none() {
            // [wisp] 顺序播完停下后，再按下一首从这一轮的第一首重新开始；还在播时先停下。
            if self.wants_playing() {
                return self.pause(cx);
            }
            if let Some(track) = self.queue.update(cx, |queue, cx| queue.rewind(cx)) {
                return self.follow_after(track, start, cx);
            }
            return self.pause(cx);
        }
        self.follow_queue(start, cx);
    }

    fn remember_fm_seed(&mut self, overplay: bool) {
        let Some(id) = self.track.as_ref().and_then(|track| track.id.clone()) else {
            return;
        };
        self.fm_seed = Some(FmSeed {
            id,
            playtime: self.position.as_secs().min(u64::from(u32::MAX)) as u32,
            overplay,
        });
    }

    /// Removing tracks from queue
    pub fn remove_from_queue(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let current_removed = self
            .queue
            .update(cx, |queue, cx| queue.remove_tracks(ids, cx));
        if !current_removed {
            return;
        }
        self.pause(cx);
        self.track = None;
        self.next(cx);
    }

    /// Notes a skip and says whether it is part of a burst, which loads only once the skipping
    /// stops.
    fn burst(&mut self) -> Start {
        let now = Instant::now();
        let rapid = self
            .skipped
            .replace(now)
            .is_some_and(|last| now.duration_since(last) < SKIP_DEBOUNCE);
        match rapid {
            true => Start::Burst,
            false => Start::Skip,
        }
    }

    /// Lines the next track up behind the current one once it is near its end, so a gapless
    /// engine can join them. An engine that fetched it already only queues it here.
    fn preload_next(&mut self, position: Duration, cx: &Context<Self>) {
        let Some(duration) = self.track.as_ref().map(|track| track.duration) else {
            return;
        };
        if duration.is_zero()
            || self.state != PlaybackState::Playing
            || duration.saturating_sub(position) > PRELOAD_BEFORE_END
        {
            return;
        }
        self.preload_upcoming(true, cx);
    }

    /// Asks the engine to fetch the track that plays after this one. With `segue` it is also
    /// lined up to follow gaplessly, which waits for the end because the queue can still change.
    fn preload_upcoming(&mut self, segue: bool, cx: &Context<Self>) {
        // Random play has no predictable next track, so nothing is lined up behind this one.
        if self.queue.read(cx).shuffle() {
            self.preloaded = None;
            return;
        }
        let queue = self.queue.read(cx);
        let next = match self.repeat {
            // nothing plays after the track a sleep request ends on
            _ if self.sleep == Some(Sleep::EndOfTrack) => None,
            Repeat::One => None,
            // the end of the queue wraps to its first song, which `advance` rewinds to
            Repeat::All if queue.ahead() == 0 => queue
                .past()
                .chain(queue.current())
                .find(|track| track.playable)
                .cloned(),
            _ => queue.upcoming().next().cloned(),
        };
        let Some(next) = next else {
            self.preloaded = None;
            return;
        };

        self.preload_internal(&next, segue);
    }

    pub fn radio(&self) -> bool {
        self.radio
    }

    /// [wisp] 本次播放是否来自私人 FM：循环键据此显示 ∞。
    pub fn fm_session(&self) -> bool {
        self.fm_session
    }

    /// [wisp] 私人 FM 是否在单曲循环：只在 FM 会话里有意义。
    pub fn fm_repeat(&self) -> bool {
        self.fm_repeat
    }

    /// [wisp] 切换私人 FM 的单曲循环；∞ ↔ 单曲循环。
    pub fn toggle_fm_repeat(&mut self, cx: &mut Context<Self>) {
        if !self.fm_session {
            return;
        }
        self.fm_repeat = !self.fm_repeat;
        cx.notify();
    }

    pub fn toggle_radio(&mut self, cx: &mut Context<Self>) {
        self.radio = !self.radio;
        let radio = self.radio;
        self.settings
            .update(cx, |settings, cx| settings.set_radio(radio, cx));
        match radio {
            true => self.suggest_similar(true, cx),
            false => {
                // 续播关掉后就不再是无限 FM，会话随之结束。
                self.fm_session = false;
                self.restore_fm_shuffle(cx);
                self.forget_similar(cx);
            }
        }
    }

    fn forget_similar(&mut self, cx: &mut Context<Self>) {
        self.seeded = None;
        self.suggest = None;
        self.queue.update(cx, |queue, cx| queue.clear_similar(cx));
        cx.notify();
    }

    /// The track the suggestions are drawn from, which is whatever is playing. It is the track
    /// the listener chose and it is playable by definition, so a station is never seeded from
    /// something at the end of the queue that will be skipped for being unavailable.
    fn seed(&self, cx: &Context<Self>) -> Option<Track> {
        self.queue.read(cx).current().cloned()
    }

    fn fm_has_playable_ahead(&self, cx: &App) -> bool {
        self.queue.read(cx).upcoming().any(|track| track.playable)
    }

    fn schedule_fm_retry(&mut self, cx: &mut Context<Self>) {
        if self.fm_retry.is_some() {
            return;
        }
        self.fm_retry = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            this.update(cx, |this, cx| {
                this.fm_retry = None;
                this.station_empty_at = None;
                if this.fm_session && this.topping.is_none() && !this.fm_has_playable_ahead(cx) {
                    this.continue_station(cx);
                }
            })
            .ok();
        }));
    }

    /// Fetches the station's next stretch once fewer than `RADIO_LOOKAHEAD` tracks are left to
    /// play, so the queue never reaches its end while the provider has more. FM keeps its own
    /// shallower window because KuGou reports a bigger remainder as a full buffer. What the
    /// queue has already heard is left out. The station is the queue itself, so it goes on
    /// whether or not radio suggestions are on; a fetch that fails drops the continuation and
    /// hands the end of the queue back to `advance` through `stranded`.
    fn continue_station(&mut self, cx: &mut Context<Self>) {
        // [wisp] 随机播放只在现有曲目里取歌：不向队列补位，取完回绕。FM is its own
        // infinite source and must keep refilling regardless of the shuffle setting.
        let fm = self.fm_session;
        if !fm && self.queue.read(cx).shuffle() {
            return;
        }
        let ahead = match fm {
            true => self
                .queue
                .read(cx)
                .upcoming()
                .filter(|track| track.playable)
                .count(),
            false => self.queue.read(cx).ahead(),
        };
        let lookahead = if fm { FM_AHEAD } else { RADIO_LOOKAHEAD };
        if self.topping.is_some() || !runs_low(ahead, lookahead) {
            return;
        }
        if fm
            && (self
                .station_empty_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(5))
                || self.fm_retry.is_some())
        {
            return;
        }
        let station = self.station.clone();
        if !fm && station.is_none() {
            return;
        }
        let origin = station
            .as_ref()
            .map(|station| station.origin.clone())
            .unwrap_or_else(|| Origin::radio(String::new()));
        let stations = self.stations(&origin.id, cx);
        let station_origin = origin.clone();
        let Some(client) = self.client_for(&origin.id, cx) else {
            return;
        };

        // FM only guards the current window: the whole-session history makes KuGou's naturally
        // repetitive recommendation pool dry out. A song radio keeps the full history.
        let heard = match fm {
            true => self.queue.read(cx).ahead_ids(),
            false => self.queue.read(cx).ids(),
        };
        let token = station.as_ref().map(|station| station.next.clone());
        let remain = self.queue.read(cx).ahead().min(FM_REMAIN - 1) as u32;
        let fm_seed = self.fm_seed.clone();
        let current = self.track.as_ref().and_then(|track| {
            track
                .id
                .clone()
                .map(|id| format!("{id}|{}|0|{remain}", self.position.as_secs()))
        });
        let mode = self.fm_mode.clone();
        let pool = self.fm_song_pool;
        if fm {
            let seed = fm_seed
                .as_ref()
                .map(|seed| {
                    format!(
                        "{}|{}|{}",
                        seed.id.split('_').next().unwrap_or(""),
                        seed.playtime,
                        seed.overplay
                    )
                })
                .unwrap_or_else(|| "current".to_owned());
            log::info!(
                "playback: fm continue ahead={ahead} remain={remain} seed={seed} mode={mode} pool={pool}"
            );
        }
        self.topping = Some(Io::global(cx).load(
            async move {
                let (mut tracks, next) = match fm {
                    true => {
                        // Feed the recommender the track that just left, the time it played and
                        // whether it ended naturally. A token frozen at the first batch made
                        // every refill look like the same request.
                        let from = fm_seed
                            .map(|seed| {
                                format!(
                                    "{}|{}|{}|{remain}",
                                    seed.id,
                                    seed.playtime,
                                    u8::from(seed.overplay)
                                )
                            })
                            .or(current)
                            .or(token);
                        client
                            .track_radio_with("", from.as_deref(), Some(&mode), Some(pool))
                            .await?
                    }
                    false => {
                        let token = token.unwrap_or_default();
                        station_origin
                            .station(&*client, stations, Some(&token))
                            .await?
                    }
                };
                unheard(&mut tracks, &heard);
                anyhow::Ok((tracks, next))
            },
            move |this, loaded, cx| {
                this.topping = None;
                if this.fm_session != fm || (!fm && this.station.as_ref() != station.as_ref()) {
                    return;
                }
                match loaded {
                    Ok((tracks, next)) => {
                        this.dry = dry_after(this.dry, tracks.len());
                        this.station_empty_at = tracks.is_empty().then(Instant::now);
                        this.station = next.filter(|_| !ran_dry(this.dry)).map(|next| Station {
                            origin: origin.clone(),
                            next,
                        });
                        let idle = this.track.is_none() && !this.queue.read(cx).has_next();
                        if tracks.is_empty() {
                            // Never feed an empty batch back through the queue: it would
                            // notify the observer and immediately start another fetch.
                            if fm && this.topping.is_none() {
                                this.schedule_fm_retry(cx);
                            }
                        } else {
                            this.fm_retry = None;
                            this.queue.update(cx, |queue, cx| {
                                queue.extend_context(tracks, origin.clone(), cx)
                            });
                            if fm && this.fm_pending_skip {
                                this.fm_pending_skip = false;
                                this.follow_queue(Start::Segue, cx);
                            } else if idle {
                                this.follow_queue(Start::Segue, cx);
                            }
                            if fm && this.topping.is_none() {
                                // Keep stacking until the FM window is full, then wait for
                                // the track-start trigger.
                                this.continue_station(cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.station = None;
                        log::warn!("playback: cannot continue the station: {error:#}");
                        if fm {
                            this.station_empty_at = Some(Instant::now());
                            this.schedule_fm_retry(cx);
                        }
                    }
                }
                if !fm && this.station.is_none() {
                    this.stranded(cx);
                }
            },
            cx,
        ));
    }

    /// Picks playback back up when the station that was covering the end of the queue stops
    /// answering, by putting the track that ended through `advance` again, which is where radio
    /// takes over. Does nothing while anything is still playing or waiting to.
    fn stranded(&mut self, cx: &mut Context<Self>) {
        if self.track.is_some() || self.queue.read(cx).has_next() {
            return;
        }
        let ended = self.seed(cx);
        self.advance(ended, cx);
    }

    /// Fills the suggestions from the current track's radio when radio is on and there are
    /// none, and tops them up once fewer than `RADIO_LOOKAHEAD` tracks are left to play, so the
    /// next batch has arrived long before the queue reaches it. What is already queued is left
    /// out, and the provider's best `SIMILAR_LIMIT` of the rest are kept. A failure is shown
    /// only when `asked`, that is when the user just turned radio on, since a top-up failing
    /// on its own is retried at the next change.
    fn suggest_similar(&mut self, asked: bool, cx: &mut Context<Self>) {
        // FM is its own infinite stream. Adding a single-track radio beside it narrows the
        // pool and keeps the queue replaying the same suggestions.
        if !self.radio || self.fm_session {
            return;
        }
        let held = self.queue.read(cx).similar().count();
        if held > 0 && !runs_low(self.queue.read(cx).upcoming().len(), RADIO_LOOKAHEAD) {
            return;
        }
        let Some(seed) = self.seed(cx) else {
            return self.forget_similar(cx);
        };
        let Some(id) = seed.id.clone() else {
            return self.forget_similar(cx);
        };
        if !self.stations(&id, cx) {
            return self.forget_similar(cx);
        }
        if self.seeded.as_deref() == Some(id.as_str()) {
            return;
        }
        let Some(client) = self.client_for(&id, cx) else {
            return self.forget_similar(cx);
        };

        let queued = self.queue.read(cx).ids();
        self.seeded = Some(id.clone());
        let title = seed.name.clone();
        let origin = Origin::radio(id.clone()).named(seed.name);
        self.suggest = Some(Io::global(cx).load(
            async move {
                let (mut tracks, _) = client.track_radio(&id, None).await?;
                unheard(&mut tracks, &queued);
                tracks.truncate(SIMILAR_LIMIT);
                anyhow::Ok(tracks)
            },
            move |this, loaded, cx| match loaded {
                Ok(_) if !this.radio => {}
                Ok(tracks) => this.queue.update(cx, |queue, cx| match held {
                    0 => queue.suggest(tracks, origin, cx),
                    _ => queue.extend_similar(tracks, origin, cx),
                }),
                Err(error) => {
                    this.seeded = None;
                    log::warn!("playback: cannot load similar tracks: {error:#}");
                    crate::noted(&error, cx);
                    if asked && this.radio {
                        Toasts::about(Outcome::Failed, "toast-similar-failed", title, cx);
                    }
                }
            },
            cx,
        ));
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    /// Switches the repeat mode and remembers it in settings.
    pub fn set_repeat(&mut self, repeat: Repeat, cx: &mut Context<Self>) {
        if self.repeat == repeat {
            return;
        }
        self.repeat = repeat;
        self.unline();
        self.settings
            .update(cx, |settings, cx| settings.set_repeat(repeat, cx));
        cx.notify();
    }

    /// Takes back the track lined up behind the current one, for a change that alters what
    /// follows it.
    fn unline(&mut self) {
        self.preloaded = None;
        if let Some(engine) = self.active_engine() {
            engine.unline();
        }
    }

    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        let repeat = match self.shown_repeat(cx) {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
        self.set_repeat(repeat, cx);
    }

    /// A binary on/off flip for surfaces (tray, dock menu) that do not fit the three-way
    /// cycle the player bar's button drives; `One` counts as on and flips straight to `Off`.
    pub fn toggle_repeat(&mut self, cx: &mut Context<Self>) {
        let repeat = match self.shown_repeat(cx) {
            Repeat::Off => Repeat::All,
            Repeat::All | Repeat::One => Repeat::Off,
        };
        self.set_repeat(repeat, cx);
    }

    pub fn shown_repeat(&self, _cx: &App) -> Repeat {
        self.repeat
    }

    /// Turns shuffle on or off.
    pub fn set_shuffle(&mut self, on: bool, cx: &mut Context<Self>) {
        self.queue.update(cx, |queue, cx| queue.set_shuffle(on, cx));
    }

    /// Flips shuffle from what the player shows.
    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.set_shuffle(!self.shown_shuffle(cx), cx);
    }

    pub fn shown_shuffle(&self, cx: &App) -> bool {
        self.queue.read(cx).shuffle()
    }

    /// Which of the four play modes is on, as the one mode button shows it.
    pub fn play_mode(&self, cx: &App) -> PlayMode {
        if self.shown_shuffle(cx) {
            return PlayMode::Random;
        }
        match self.shown_repeat(cx) {
            Repeat::Off => PlayMode::Sequential,
            Repeat::All => PlayMode::List,
            Repeat::One => PlayMode::Single,
        }
    }

    /// Steps the mode button through 顺序 → 列表循环 → 随机 → 单曲循环, as the old app did.
    pub fn cycle_play_mode(&mut self, cx: &mut Context<Self>) {
        match self.play_mode(cx) {
            PlayMode::Sequential => self.set_repeat(Repeat::All, cx),
            PlayMode::List => self.set_shuffle(true, cx),
            PlayMode::Random => {
                self.set_shuffle(false, cx);
                self.set_repeat(Repeat::One, cx);
            }
            PlayMode::Single => self.set_repeat(Repeat::Off, cx),
        }
    }

    /// Decides what follows a track that ended: the same one on repeat-one, the queue's start
    /// on repeat-all, more radio when it is on and the queue ran out, else the next in line.
    fn advance(&mut self, ended: Option<Track>, cx: &mut Context<Self>) {
        if self.fm_session
            && let Some(track) = ended.as_ref()
            && let Some(id) = track.id.clone()
        {
            self.fm_seed = Some(FmSeed {
                id,
                playtime: track.duration.as_secs().min(u64::from(u32::MAX)) as u32,
                overplay: true,
            });
        }
        // 私人 FM 的单曲循环优先于普通循环：一首放完就再放这首。
        if self.fm_session && self.fm_repeat {
            return match ended {
                Some(track) => self.load_after(&track, Start::Segue, cx),
                None => self.segue_queue(cx),
            };
        }
        if self.fm_session && (!self.queue.read(cx).has_next() || !self.fm_has_playable_ahead(cx)) {
            log::info!(
                "playback: fm advance with no playable upcoming, topping={}",
                self.topping.is_some()
            );
            self.fetch = None;
            if self.topping.is_none() {
                self.continue_station(cx);
            }
            return;
        }
        // [wisp] 随机播放：从队列剩下的歌里随机取下一首，取完回绕重来，不向队列补歌。
        if self.queue.read(cx).shuffle() {
            if self.queue.read(cx).ahead() == 0 {
                self.fetch = None;
                if let Some(track) = self.queue.update(cx, |queue, cx| queue.rewind_ordered(cx)) {
                    self.follow_after(track, Start::Segue, cx);
                }
            } else {
                self.segue_queue(cx);
            }
            return;
        }
        match self.repeat {
            Repeat::One => match ended {
                Some(track) => self.load_after(&track, Start::Segue, cx),
                None => self.segue_queue(cx),
            },
            Repeat::All if self.queue.read(cx).ahead() == 0 => {
                self.fetch = None;
                if let Some(track) = self.queue.update(cx, |queue, cx| queue.rewind(cx)) {
                    self.follow_after(track, Start::Segue, cx);
                }
            }
            // The station's next stretch is on its way; playing resumes when it lands.
            _ if self.topping.is_some() && !self.queue.read(cx).has_next() => {
                self.fetch = None;
            }
            _ if self.radio && !self.queue.read(cx).has_next() => {
                let seed = ended
                    .or_else(|| self.track.clone())
                    .filter(|seed| seed.id.as_deref().is_some_and(|id| self.stations(id, cx)));
                match seed {
                    Some(seed) => self.extend_radio(&seed, cx),
                    None => self.segue_queue(cx),
                }
            }
            _ => self.segue_queue(cx),
        }
    }

    fn segue_queue(&mut self, cx: &mut Context<Self>) {
        self.fetch = None;
        self.follow_queue(Start::Segue, cx);
    }

    /// Appends the seed's radio to the queue and moves on, or just moves on if it fails.
    fn extend_radio(&mut self, seed: &Track, cx: &mut Context<Self>) {
        let Some(id) = seed.id.clone() else {
            return self.next(cx);
        };
        let Some(client) = self.client_for(&id, cx) else {
            return self.next(cx);
        };

        let origin = Origin::radio(id.clone()).named(seed.name.clone());
        let heard = self.queue.read(cx).ids();
        self.sought = None;
        self.fetch = Some(Io::global(cx).load(
            async move {
                let (mut tracks, _) = client.track_radio(&id, None).await?;
                unheard(&mut tracks, &heard);
                anyhow::Ok(tracks)
            },
            move |this, loaded, cx| match loaded {
                Ok(tracks) if !tracks.is_empty() => {
                    this.queue.update(cx, |queue, cx| {
                        queue.insert(tracks, Some(origin), QueuePlacement::End, cx)
                    });
                    this.leave_station(cx);
                    this.follow_queue(Start::Segue, cx);
                }
                Ok(_) => {
                    log::warn!("playback: radio returned no tracks");
                    this.finish_queue(cx);
                }
                Err(error) => {
                    log::warn!("playback: cannot extend radio: {error:#}");
                    this.finish_queue(cx);
                }
            },
            cx,
        ));
    }

    fn follow_queue(&mut self, start: Start, cx: &mut Context<Self>) {
        let Some(track) = self.playable_next(cx) else {
            return self.finish_queue(cx);
        };
        self.load_after(&track, start, cx);
    }

    /// Ends a queue that has run out with repeat off, moving its last song to history so Play
    /// can start it over. Does nothing while a track is loaded or more is queued.
    fn finish_queue(&mut self, cx: &mut Context<Self>) {
        if self.repeat == Repeat::Off
            && !self.fm_session
            && self.track.is_none()
            && !self.queue.read(cx).has_next()
        {
            self.fetch = None;
            self.queue.update(cx, |queue, cx| queue.finish(cx));
            cx.notify();
        }
    }

    /// The next playable track the queue has, dropping the ones that are not. Reaching the
    /// suggestions means radio has taken over from the station the queue was playing.
    fn playable_next(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        loop {
            let (track, suggested) = self.queue.update(cx, |queue, cx| queue.next(cx))?;
            if suggested {
                self.leave_station(cx);
            }
            if track.playable {
                return Some(track);
            }
        }
    }

    fn follow_after(&mut self, track: Track, start: Start, cx: &mut Context<Self>) {
        let track = match track.playable {
            true => Some(track),
            false => self.playable_next(cx),
        };
        let Some(track) = track else {
            return;
        };
        self.load_after(&track, start, cx);
    }

    /// [wisp] Stops playback and empties the queue, history included.
    pub fn clear_queue(&mut self, cx: &mut Context<Self>) {
        self.pause(cx);
        self.fetch = None;
        self.topping = None;
        self.station = None;
        self.fm_session = false;
        self.restore_fm_shuffle(cx);
        self.fm_pending_skip = false;
        self.track = None;
        self.position = Duration::ZERO;
        self.clock.reset(Duration::ZERO, false);
        self.state = PlaybackState::Idle;
        self.queue.update(cx, |queue, cx| queue.clear_all(cx));
        self.remember(true, cx);
        cx.notify();
    }

    pub fn has_previous(&self, cx: &App) -> bool {
        self.track.is_some() || self.queue.read(cx).has_previous()
    }

    /// The occurrence id of a queue row, for callers that address edits by id.
    pub fn queue_uid(&self, position: QueuePosition, cx: &App) -> Option<String> {
        self.queue.read(cx).uid(position).map(str::to_owned)
    }

    /// Removes the upcoming queue row with `uid`, saying whether it was there. The current and
    /// past rows cannot be removed.
    pub fn remove_queued(&mut self, uid: &str, cx: &mut Context<Self>) -> bool {
        let Some(index) = self
            .queue
            .read(cx)
            .position(uid)
            .and_then(QueuePosition::upcoming)
        else {
            return false;
        };
        self.queue
            .update(cx, |queue, cx| queue.remove(QueuePosition::Upcoming(index), cx));
        true
    }

    /// Moves the upcoming row with `uid` to `gap`, saying whether it could be moved.
    pub fn move_queued(&mut self, uid: &str, gap: usize, cx: &mut Context<Self>) -> bool {
        let Some(from) = self
            .queue
            .read(cx)
            .position(uid)
            .and_then(QueuePosition::upcoming)
        else {
            return false;
        };
        self.queue
            .update(cx, |queue, cx| queue.move_upcoming_to_gap(from, gap, cx));
        true
    }

    /// Drops everything queued after the current track, the same as the queue's Clear button.
    pub fn clear_queued(&mut self, cx: &mut Context<Self>) {
        self.queue.update(cx, |queue, cx| queue.clear_upcoming(cx));
    }

    /// Whether a play press with no track loaded starts the finished queue over instead of
    /// doing nothing.
    pub fn can_restart_queue(&self, cx: &App) -> bool {
        self.fetch.is_none()
            && self.track.is_none()
            && self.queue.read(cx).current().is_none()
            && self.queue.read(cx).has_previous()
    }

    pub fn previous(&mut self, cx: &mut Context<Self>) {
        // [wisp] 随机播放的上一首也是随机取一首，第一首时也能往回切，与旧版一致。
        if self.queue.read(cx).shuffle() {
            if self.track.is_none() {
                return;
            }
            self.fetch = None;
            let start = self.burst();
            return match self.queue.update(cx, |queue, cx| queue.previous_random(cx)) {
                Some(track) => self.load_after(&track, start, cx),
                None => self.seek(Duration::ZERO, cx),
            };
        }
        // [wisp] 上一首总是切歌：历史里有就回退，没有就按循环逻辑跳到列表最后一首。
        self.fetch = None;
        let start = self.burst();
        match self.queue.update(cx, |queue, cx| queue.previous(cx)) {
            Some(track) => self.load_after(&track, start, cx),
            None => self.seek(Duration::ZERO, cx),
        }
    }

    /// Selects a stable local occurrence; selecting current toggles playback.
    pub fn play_queue(&mut self, uid: &str, cx: &mut Context<Self>) {
        use crate::QueuePosition;
        let Some(position) = self.queue.read(cx).position(uid) else {
            return;
        };
        if position == QueuePosition::Current {
            return self.toggle_play(cx);
        }
        if self.fm_session {
            self.remember_fm_seed(false);
        }
        self.fetch = None;
        let suggested = self.queue.read(cx).kind(position) == Some(crate::QueueKind::Suggested);
        let track = self
            .queue
            .update(cx, |queue, cx| queue.select(position, cx));
        let Some(track) = track else { return };
        if suggested {
            self.leave_station(cx);
        }
        self.load_after(&track, Start::Pick, cx);
    }

    /// Plays on. A restored track the engine does not hold yet, or one paused for longer than
    /// `STALE_PAUSE`, is loaded afresh at its position.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        self.intent = Intent::Play;
        self.pressed = Some(Instant::now());
        if let Some(at) = self.resume_at {
            if !self.resume_ready {
                return self.reload_and_seek(at, cx);
            }
            self.resume_at = None;
            self.resume_ready = false;
        }
        if self
            .paused_at
            .take()
            .is_some_and(|since| since.elapsed().unwrap_or(STALE_PAUSE) >= STALE_PAUSE)
        {
            log::info!("playback: paused too long to trust the engine, reloading the track");
            return self.reload_and_seek(self.position, cx);
        }
        if let Some(engine) = self.active_engine() {
            engine.play();
            cx.notify();
        }
    }

    fn reload_and_seek(&mut self, at: Duration, cx: &mut Context<Self>) {
        let Some(track) = self.track.clone() else {
            return;
        };
        if self.active_engine().is_none() {
            return;
        }
        self.load_from(&track, at, Start::Pick, cx);
    }

    /// Applies rescanned tags to the track on show. A file that moved reopens at the position
    /// it had, without restarting the queue around it.
    fn refresh_local_metadata(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.track.clone() else {
            return;
        };
        let Some(track) = previous
            .id
            .as_deref()
            .and_then(|id| self.session.read(cx).refreshed_local_track(id))
            .cloned()
        else {
            return;
        };
        if previous == track {
            return;
        }
        let moved = previous.id != track.id;
        let at = self.position;
        let playing = self.intent == Intent::Play
            && matches!(self.state, PlaybackState::Playing | PlaybackState::Loading);
        self.preloaded = None;
        if moved && track.playable {
            self.track = Some(track.clone());
            if playing {
                self.load_from(&track, at, Start::Pick, cx);
                return;
            }
            if let Some(engine) = self.active_engine() {
                engine.pause();
            }
            self.hold(at, cx);
            self.remember(true, cx);
            return;
        }
        self.track = Some(track);
        self.remember(true, cx);
        cx.notify();
    }

    /// Has the engine fetch the restored track and hold it paused at `resume_at`, so the first
    /// play is instant. Until it says so, play loads the track instead.
    fn prepare_resume(&mut self, cx: &mut Context<Self>) {
        self.resume_ready = false;
        let Some(at) = self.resume_at else {
            return;
        };
        let Some(track) = self.track.clone() else {
            return;
        };
        music::builtin::remember(&track);
        let Some(id) = track.id.as_deref().filter(|_| track.playable) else {
            return;
        };
        let Some(engine) = self.engine_for(id) else {
            return;
        };
        match engine.load_paused_at(id, at) {
            Ok(()) => {
                self.resume_ready = true;
                self.state = PlaybackState::Paused;
                self.position = at;
                self.clock.reset(at, false);
            }
            Err(error) => log::warn!(
                "playback: cannot prepare {} for resume: {error:#}",
                track.name
            ),
        }
        cx.notify();
    }

    /// Restores the last session's track and queue from settings, paused where it stopped.
    /// Only when nothing is playing or queued, and only for the provider that saved it.
    fn adopt(&mut self, cx: &mut Context<Self>) {
        if self.track.is_some() {
            return;
        }
        if let Some(track) = self.queue.read(cx).current().cloned() {
            self.track = Some(track);
            self.state = PlaybackState::Paused;
            self.position = Duration::ZERO;
            self.clock.reset(Duration::ZERO, false);
            self.stored = Duration::ZERO;
            self.resume_at = Some(Duration::ZERO);
            self.prepare_resume(cx);
            cx.notify();
            return;
        }
        if !self.queue.read(cx).is_empty() {
            return;
        }
        let Some(slug) = self.session.read(cx).provider_slug() else {
            return;
        };
        let Some(resume) = self
            .settings
            .read(cx)
            .resume()
            .filter(|resume| resume.provider == slug)
            .cloned()
        else {
            return;
        };

        let at = Duration::from_secs_f32(resume.position.max(0.));
        let Some(track) = self.queue.update(cx, |queue, cx| queue.revive(resume, cx)) else {
            return;
        };
        self.track = Some(track);
        self.state = PlaybackState::Paused;
        self.position = at;
        self.clock.reset(at, false);
        self.stored = at;
        self.resume_at = Some(at);
        self.prepare_resume(cx);
    }

    /// Writes the position to settings for resuming, every few seconds or when `force`d.
    fn remember(&mut self, force: bool, cx: &mut Context<Self>) {
        let position = self.position;
        if !force && position.abs_diff(self.stored) < RESUME_STEP {
            return;
        }
        self.stored = position;
        let seconds = position.as_secs_f32();
        self.settings
            .update(cx, |settings, cx| settings.set_resume_position(seconds, cx));
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        self.intent = Intent::Pause;
        self.pressed = Some(Instant::now());
        if let Some(engine) = self.active_engine() {
            engine.pause();
            cx.notify();
        }
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        match self.wants_playing() {
            true => self.pause(cx),
            false => {
                if self.can_restart_queue(cx) {
                    if let Some(track) = self.queue.update(cx, |queue, cx| queue.rewind(cx)) {
                        self.follow_after(track, Start::Pick, cx);
                    }
                    return;
                }
                self.resume(cx);
            }
        }
    }

    pub fn sleep(&self) -> Option<Sleep> {
        self.sleep
    }

    /// Replaces the active sleep request, dropping its timer task when cancelled or superseded.
    /// A timer that runs out while music plays pauses it and says so on a toast.
    pub fn set_sleep(&mut self, sleep: Option<Sleep>, cx: &mut Context<Self>) {
        if sleep == Some(Sleep::EndOfTrack) {
            self.unline();
        }
        self.sleep = sleep;
        self.sleep_task = match sleep {
            Some(Sleep::After(after)) => Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(after).await;
                this.update(cx, |this, cx| {
                    this.sleep = None;
                    if this.wants_playing() {
                        Toasts::show(Outcome::Info, "toast-sleep-ended", cx);
                    }
                    this.pause(cx);
                    cx.notify();
                })
                .ok();
            })),
            _ => None,
        };
        cx.notify();
    }

    /// Ends an end-of-track sleep request once its track has ended, holding the next track
    /// paused, and says so on a toast.
    fn doze(&mut self, cx: &mut Context<Self>) {
        Toasts::show(Outcome::Info, "toast-sleep-ended", cx);
        self.sleep = None;
        self.sleep_task = None;
        self.fetch = None;
        let Some(track) = self.playable_next(cx) else {
            return;
        };
        self.track = Some(track);
        self.resume_at = Some(Duration::ZERO);
        self.remember(true, cx);
        self.prepare_resume(cx);
    }

    /// Moves to the next playable track and holds it paused without fetching it, so play loads
    /// it then. Leaves nothing loaded when the queue has run out.
    fn hold_next(&mut self, cx: &mut Context<Self>) {
        self.fetch = None;
        self.track = self.playable_next(cx);
        if self.track.is_some() {
            self.hold(Duration::ZERO, cx);
            self.remember(true, cx);
        }
    }

    /// Keeps the current track paused at `at` without asking the engine for it, so play loads
    /// it from there.
    fn hold(&mut self, at: Duration, cx: &mut Context<Self>) {
        self.intent = Intent::Pause;
        self.state = PlaybackState::Paused;
        self.position = at;
        self.clock.reset(at, false);
        self.resume_at = Some(at);
        self.resume_ready = false;
        cx.notify();
    }

    /// Whether the user wants the current track playing, whatever the engine has managed so
    /// far. A transport control reads this; anything that follows the sound reads `state`.
    /// Whether the engine reporting `reported` answers a press the user has since reversed.
    fn overtaken(&self, reported: Intent) -> bool {
        reported != self.intent
            && self
                .pressed
                .is_some_and(|pressed| pressed.elapsed() < ECHO_WINDOW)
    }

    pub fn wants_playing(&self) -> bool {
        self.intent == Intent::Play && self.track.is_some()
    }

    /// What a play button for the current track shows. `Some(true)` is pause and `Some(false)`
    /// is play, and pressing either goes to `toggle_play`. `None` means nothing is loaded that a
    /// press could resume, so the button starts its tracks instead.
    pub fn control(&self) -> Option<bool> {
        self.has_active_playback().then(|| self.wants_playing())
    }

    pub fn play_origin(&mut self, origin: Origin, cx: &mut Context<Self>) {
        self.fm_session = false;
        self.restore_fm_shuffle(cx);
        if origin.fetched() {
            self.gather(origin, false, cx);
        }
    }

    /// Turns shuffle on and plays `origin` from a random track once its tracks are loaded.
    pub fn shuffle_origin(&mut self, origin: Origin, cx: &mut Context<Self>) {
        self.fm_session = false;
        self.restore_fm_shuffle(cx);
        if origin.fetched() {
            self.gather(origin, true, cx);
        }
    }

    /// The play button of a collection page: pauses or resumes when the queue came from it,
    /// starts it otherwise.
    pub fn toggle_origin(&mut self, origin: &Origin, cx: &mut Context<Self>) {
        match self.playing_from(origin, cx) {
            Some(_) => self.toggle_play(cx),
            None => self.play_origin(origin.clone(), cx),
        }
    }

    /// Moves to `position`. While playing, the clock parks there until the engine reports
    /// audio from it, and a second seek meanwhile waits for the first to land. A restored track
    /// not yet held by the engine only moves its resume point.
    pub fn seek(&mut self, position: Duration, cx: &mut Context<Self>) {
        if self.resume_at.is_some() {
            self.resume_at = Some(position);
            if self.resume_ready
                && let Some(engine) = self.active_engine()
            {
                engine.seek(position);
            }
            self.position = position;
            self.clock.reset(position, false);
            self.remember(true, cx);
            cx.notify();
            return;
        }
        if self.active_engine().is_none() {
            return;
        }
        // The clock parks at the target; the engine's Seeked starts it again once audio from
        // there is playing. A loading or paused engine takes the seek at once and reports the
        // position with its Playing, so only a playing one needs the seek tracked.
        self.position = position;
        self.clock.reset(position, false);
        self.seek_target = Some(position);
        match (self.state == PlaybackState::Playing, self.seek_in_flight) {
            (true, Some(_)) => self.seek_next = Some(position),
            (true, None) => {
                self.seek_in_flight = Some(position);
                self.stale_positions = 0;
                self.send_seek(position);
            }
            (false, _) => self.send_seek(position),
        }
        cx.notify();
    }

    fn send_seek(&self, position: Duration) {
        if let Some(engine) = self.active_engine() {
            engine.seek(position);
        }
    }

    /// The position to show for where the engine reports it landed: the seek target itself
    /// when the engine stopped just short of it on a packet boundary.
    fn landed(&mut self, at: Duration) -> Duration {
        match self.seek_target.take() {
            Some(target) if at < target && target - at < SEEK_SNAP => target,
            _ => at,
        }
    }

    /// Sends the seek that queued up behind the one that just landed.
    fn follow_up_seek(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.seek_next.take() {
            self.seek(next, cx);
        }
    }

    /// Seeks to a share of the track, as the progress bar asks.
    pub fn seek_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let Some(total) = self
            .track
            .as_ref()
            .map(|track| track.duration)
            .filter(|total| !total.is_zero())
        else {
            return;
        };

        let position = Duration::from_secs_f32(total.as_secs_f32() * fraction.clamp(0., 1.));
        self.seek(position, cx);
    }

    pub fn state(&self) -> &PlaybackState {
        &self.state
    }

    /// The entity notified on every position report, which `Playback` itself is not. Observe it
    /// as well as `Playback` to follow the position as it moves.
    pub fn ticks(&self) -> &Entity<PositionTick> {
        &self.ticks
    }

    /// Tells the observers of `ticks` that the position moved.
    fn tick(&self, cx: &mut App) {
        self.ticks.update(cx, |_, cx| cx.notify());
    }

    /// The position as last reported or asked for. It only moves on events.
    pub fn position(&self) -> Duration {
        self.position
    }

    /// The position right now: the clock while playing, else `position`. Never past the end.
    pub fn live_position(&self) -> Duration {
        let live = match self.state == PlaybackState::Playing {
            true => self.clock.now(),
            false => self.position,
        };
        match self.track.as_ref().map(|track| track.duration) {
            Some(total) if !total.is_zero() => live.min(total),
            _ => live,
        }
    }

    pub fn track(&self) -> Option<&Track> {
        self.track.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, PlaybackState::Loading)
    }

    /// Whether a track is loaded, whatever it is doing.
    fn has_active_playback(&self) -> bool {
        self.track.is_some()
            && matches!(
                self.state,
                PlaybackState::Playing | PlaybackState::Paused | PlaybackState::Loading
            )
    }

    pub fn volume(&self) -> f32 {
        self.level
    }

    /// Whether the shown playback device is silent.
    pub fn muted(&self, cx: &App) -> bool {
        self.shown_volume(cx) <= MUTE_THRESHOLD
    }

    /// Toggles mute using the volume of the current playback device.
    pub fn toggle_muted(&mut self, cx: &mut Context<Self>) {
        self.set_muted(!self.muted(cx), cx);
    }

    /// Mutes or restores the output without saving the temporary level.
    pub fn set_muted(&mut self, muted: bool, cx: &mut Context<Self>) {
        let current = self.shown_volume(cx);
        let Some(level) = self.mute.change(muted, current) else {
            return;
        };
        self.apply_volume(level, false, cx);
    }

    /// The gain the volume applies, 1 at full volume.
    pub fn volume_gain(&self) -> f32 {
        gain(self.level)
    }

    pub fn set_volume(&mut self, level: f32, cx: &mut Context<Self>) {
        self.mute.volume_changed();
        self.apply_volume(level, true, cx);
    }

    /// Changes the volume and optionally remembers it as the preference.
    fn apply_volume(&mut self, level: f32, remember: bool, cx: &mut Context<Self>) {
        self.sliding = None;
        self.level = level.clamp(0., 1.);
        if remember {
            self.settings
                .update(cx, |settings, cx| settings.set_volume(self.level, cx));
        }
        self.apply_gain();
        cx.notify();
    }

    /// Follows the volume slider while it is dragged: the level is heard at once, but nothing is
    /// saved and no observer wakes, since nearly every view and the media controls observe
    /// playback. The caller repaints itself, and calls `end_volume_slide` when the slider is let go.
    pub fn slide_volume(&mut self, level: f32, _cx: &App) {
        let level = level.clamp(0., 1.);
        self.mute.volume_changed();
        self.sliding = Some(level);
        self.level = level;
        self.apply_gain();
    }

    /// Saves the level the slider was let go at and tells every observer, once per drag.
    pub fn end_volume_slide(&mut self, cx: &mut Context<Self>) {
        let Some(level) = self.sliding else {
            return;
        };
        self.set_volume(level, cx);
    }

    /// Turns the volume by `steps` notches of `VOLUME_STEP` from the level shown.
    pub fn nudge_volume(&mut self, steps: i8, cx: &mut Context<Self>) {
        let level = volume_after(self.shown_volume(cx), steps);
        self.set_volume(level, cx);
    }

    /// Moves `SEEK_STEP` forward or back through the shown track, stopping at its start and end.
    /// Does nothing without a track.
    pub fn skip(&mut self, forward: bool, cx: &mut Context<Self>) {
        let (_, total) = self.shown_time(cx);
        if total.is_zero() {
            return;
        }
        let at = self.seek_target.unwrap_or_else(|| self.live_position());
        self.seek(seek_target(at, total, forward), cx);
    }

    /// Sends the gain for the current level to both engines. [wisp] 前置放大叠在这里，
    /// 与音量同走 Volume 节点，改动即时生效，不需要重开引擎。
    fn apply_gain(&self) {
        let level = gain(self.level) * preamp_factor(self.preamp_db);
        if let Some(engine) = self.engine.as_ref() {
            engine.set_gain(level);
        }
        if let Some(engine) = self.local_engine.as_ref() {
            engine.set_gain(level);
        }
    }

    /// [wisp] 当前前置放大，dB。
    pub fn preamp_db(&self) -> f32 {
        self.preamp_db
    }

    /// [wisp] 改前置放大：存设置并立即重发增益。
    pub fn set_preamp_db(&mut self, db: f32, cx: &mut Context<Self>) {
        let db = db.clamp(crate::settings::PREAMP_MIN, crate::settings::PREAMP_MAX);
        if self.preamp_db == db {
            return;
        }
        self.preamp_db = db;
        self.settings
            .update(cx, |settings, cx| settings.set_preamp_db(db, cx));
        self.apply_gain();
        cx.notify();
    }

    pub fn normalisation(&self) -> bool {
        self.normalisation
    }

    pub fn gapless(&self) -> bool {
        self.gapless
    }

    pub fn transcode(&self) -> Transcode {
        self.transcode
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn keep_tracks(&self) -> bool {
        self.keep_tracks
    }

    pub fn cache_gb(&self) -> u32 {
        self.cache_gb
    }

    /// What the cached tracks take on disk, in bytes, as last measured.
    pub fn usage(&self) -> Option<u64> {
        self.usage
    }

    /// Measures what the cache takes on disk, off the main thread.
    pub fn measure_storage(&mut self, cx: &mut Context<Self>) {
        let vault = self.vault.clone();
        self.measured(cx, move || vault.usage());
    }

    /// Removes every cached track and measures the vault again after.
    pub fn clear_cache(&mut self, cx: &mut Context<Self>) {
        let vault = self.vault.clone();
        self.measured(cx, move || {
            vault.forget();
            vault.usage()
        });
    }

    /// [wisp] 换缓存目录：存设置、用同一把 master key 重建 vault、重启引擎并重新测量占用。
    /// 返回旧缓存目录，供设置页提示用户自行清理；目录没变时返回 `None`。
    pub fn set_cache_dir(
        &mut self,
        dir: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let text = dir.as_ref().map(|dir| dir.to_string_lossy().into_owned());
        let current = self.settings.read(cx).cache_dir().map(str::to_owned);
        if current == text {
            return None;
        }
        let previous = self.vault.cache_root().to_path_buf();
        self.settings.update(cx, |settings, cx| {
            settings.set_cache_dir(text.as_deref(), cx)
        });
        let vault = Arc::new(Vault::with_cache(dir, crate::vault::bytes(self.cache_gb)));
        vault.set_caching(self.keep_tracks);
        self._unlock = crate::vault::unlock(vault.clone(), cx);
        self.vault = vault;
        self.restart_engine(cx);
        self.measure_storage(cx);
        cx.notify();
        Some(previous)
    }

    /// Runs `work` off the main thread and keeps the usage it answers.
    fn measured(&mut self, cx: &mut Context<Self>, work: impl FnOnce() -> u64 + Send + 'static) {
        self.measuring = Some(cx.spawn(async move |this, cx| {
            let usage = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| {
                if this.usage != Some(usage) {
                    this.usage = Some(usage);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Changes how many gigabytes the cache of played tracks may take. A lower limit evicts
    /// the least recently played tracks right away.
    pub fn set_cache_gb(&mut self, gb: u32, cx: &mut Context<Self>) {
        let (low, high) = crate::vault::CACHE_GB_RANGE;
        let gb = gb.clamp(low, high);
        if self.cache_gb == gb {
            return;
        }
        self.cache_gb = gb;
        self.settings
            .update(cx, |settings, cx| settings.set_cache_gb(gb, cx));
        let vault = self.vault.clone();
        let bytes = crate::vault::bytes(gb);
        self.resize = Some(cx.background_spawn(async move { vault.set_limit(bytes) }));
        cx.notify();
    }

    /// Turns caching played tracks on or off. Turning it off also removes every cached track.
    pub fn set_keep_tracks(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.keep_tracks == on {
            return;
        }
        self.keep_tracks = on;
        self.vault.set_caching(on);
        self.settings
            .update(cx, |settings, cx| settings.set_keep_tracks(on, cx));
        if !on {
            self.clear_cache(cx);
        }
        cx.notify();
    }

    pub fn set_gapless(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.gapless == on {
            return;
        }
        self.gapless = on;
        self.settings
            .update(cx, |settings, cx| settings.set_gapless(on, cx));
        self.restart_engine(cx);
    }

    /// Changes the stream level asked of a streaming service. The streaming engine restarts, so
    /// the current track reloads at the new level where it was.
    pub fn set_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        if self.quality == quality {
            return;
        }
        self.quality = quality;
        self.settings
            .update(cx, |settings, cx| settings.set_quality(quality, cx));
        self.restart_engine(cx);
    }

    /// 改变播放速度：引擎重启并在原位置重开当前曲目，立即听到新速度。
    pub fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        let speed = speed.clamp(0.5, 2.0);
        if (self.speed - speed).abs() < 0.001 {
            return;
        }
        // The output queue holds a couple of seconds of audio already produced at the old
        // tempo; reopening at the heard position is what makes the new speed audible now.
        let at = self.live_position();
        self.speed = speed;
        self.clock.retempo(speed);
        self.settings
            .update(cx, |settings, cx| settings.set_playback_speed(speed, cx));
        // 无缝：写共享控制，音频线程下一缓冲周期就地切速，不重开曲目
        if let Some(engine) = self.engine.as_ref() {
            engine.set_speed(speed);
        }
        if let Some(engine) = self.local_engine.as_ref() {
            engine.set_speed(speed);
        }
        if self.state == PlaybackState::Playing
            && !at.is_zero()
            && self.resume_at.is_none()
            && self.active_engine().is_some()
        {
            self.seek(at, cx);
        }
        self.tick(cx);
        cx.notify();
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    pub fn set_normalisation(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.normalisation == on {
            return;
        }
        self.normalisation = on;
        self.settings
            .update(cx, |settings, cx| settings.set_normalisation(on, cx));
        self.restart_engine(cx);
    }

    /// Whether the current track plays through the local engine.
    fn local_active(&self) -> bool {
        self.track
            .as_ref()
            .and_then(|track| track.id.as_deref())
            .is_some_and(music::is_local_id)
    }

    /// Rebuilds the streaming engine after a setting it was configured with changed, resuming
    /// where it was unless a local track is playing.
    fn restart_engine(&mut self, cx: &mut Context<Self>) {
        let playback = match self.engine.is_some() {
            true => self.session.read(cx).playback(),
            false => None,
        };
        let Some(playback) = playback else {
            return cx.notify();
        };

        match self.local_active() {
            true => self.start_engine(playback, cx),
            false => self.rebind(playback, cx),
        }
    }

    /// Rebuilds every running engine on the audio device it should play on now, as when the
    /// device is chosen in settings.
    pub(crate) fn restart_outputs(&mut self, cx: &mut Context<Self>) {
        self.restart_output(false, None, cx);
        self.restart_output(true, None, cx);
    }

    /// Rebuilds the local or the streaming engine, if it is running, on the audio device it
    /// should play on now. The engine of the current track reloads it where the clock stood,
    /// playing if it was and held paused there if it was not; the other one, or one with nothing
    /// loaded, comes back empty, so its stream still reopens at the device's new rate. A failed
    /// track is left as it is.
    fn restart_output(
        &mut self,
        local: bool,
        heard: Option<(String, Duration)>,
        cx: &mut Context<Self>,
    ) {
        let running = match local {
            true => self.local_engine.is_some(),
            false => self.engine.is_some(),
        };
        if !running {
            return;
        }
        let playback = match local {
            true => self.session.read(cx).local_playback(),
            false => self.session.read(cx).playback(),
        };
        let Some(playback) = playback else {
            return;
        };
        let current = self
            .track
            .clone()
            .filter(|track| track.id.is_some() && local == self.local_active());
        // what the output had played beats the clock, which kept running while the device
        // switched and would put the reload past the start of a track that just began
        let at = heard
            .filter(|(id, _)| {
                current.as_ref().and_then(|track| track.id.as_deref()) == Some(id.as_str())
            })
            .map(|(_, at)| at)
            .unwrap_or_else(|| self.live_position());
        let playing = match self.state {
            PlaybackState::Playing => Some(true),
            PlaybackState::Loading => Some(self.intent == Intent::Play),
            PlaybackState::Paused | PlaybackState::Idle => Some(false),
            PlaybackState::Failed(_) => None,
        };
        // the engine holds a paused track where it stood as it starts, as it does a restored one
        if current.is_some() && playing == Some(false) {
            self.resume_at = Some(at);
        }

        log::info!("playback: restarting after the audio output changed");
        match local {
            true => {
                self.local_task = None;
                retire(self.local_engine.take());
                self.start_local_engine(playback, cx);
            }
            false => {
                self.task = None;
                retire(self.engine.take());
                self.start_engine(playback, cx);
            }
        }
        if let Some(track) = current
            && playing == Some(true)
        {
            self.load_from(&track, at, Start::Pick, cx);
        }
    }

    /// Whether a track's failure is the session having gone stale, in which case the session
    /// reconnects and playback continues from `rebind`.
    fn ask_for_reconnect(&mut self, cx: &mut Context<Self>) -> bool {
        if self.track.is_none() {
            return false;
        }
        self.awaiting_reconnect = self
            .session
            .update(cx, |session, cx| session.reconnect_if_stale(cx));
        self.awaiting_reconnect
    }

    /// Moves playback onto a fresh engine, as after a reconnect, and resumes where it was if it
    /// was playing, loading to play, or waiting to.
    fn rebind(&mut self, playback: Arc<dyn PlaybackFactory>, cx: &mut Context<Self>) {
        let resume = match self.state {
            PlaybackState::Playing => true,
            PlaybackState::Loading => self.intent == Intent::Play,
            _ => false,
        } || self.awaiting_reconnect;
        self.awaiting_reconnect = false;
        let at = self.live_position();
        self.task = None;
        self.engine = None;
        self.preloaded = None;
        self.blocked_until = None;
        if self.track.is_some() {
            self.resume_at = Some(at);
        }
        self.start_engine(playback, cx);
        if resume {
            self.resume(cx);
        }
    }

    /// Builds the streaming engine and starts pumping its events.
    fn start_engine(&mut self, playback: Arc<dyn PlaybackFactory>, cx: &mut Context<Self>) {
        // A level chosen under another provider may not exist here, so the engine asks for the
        // nearest one this provider serves. The listener's own choice stays in settings for the
        // next provider that does serve it.
        let quality = self
            .quality
            .within(self.session.read(cx).capabilities().qualities);
        let config = PlaybackConfig {
            normalisation: self.normalisation,
            gapless: self.gapless,
            position_interval: POSITION_INTERVAL,
            gain: gain(self.level) * preamp_factor(self.preamp_db),
            transcode: self.transcode,
            quality,
            speed: self.speed,
            speed_control: self.speed_control.clone(),
            equalizer: self.equalizer.clone(),
            meter: self.meter.clone(),
            analyzer: self.analyzer.clone(),
            vault: Some(self.vault.clone()),
        };
        let (engine, events) = playback.start(config);

        self.stream_slug = self.session.read(cx).provider_slug().map(str::to_owned);
        self.listen(events, false, cx);
        self.engine = Some(engine);
        self.refused = None;
        if !self.local_active() {
            self.state = PlaybackState::Idle;
            self.position = Duration::ZERO;
            self.clock.reset(Duration::ZERO, false);
        }
        self.prepare_resume(cx);
        cx.notify();
    }

    fn start_local_engine(&mut self, playback: Arc<dyn PlaybackFactory>, cx: &mut Context<Self>) {
        let config = PlaybackConfig {
            normalisation: self.normalisation,
            gapless: self.gapless,
            position_interval: POSITION_INTERVAL,
            gain: gain(self.level) * preamp_factor(self.preamp_db),
            transcode: self.transcode,
            quality: self.quality,
            speed: self.speed,
            speed_control: self.speed_control.clone(),
            equalizer: self.equalizer.clone(),
            meter: self.meter.clone(),
            analyzer: self.analyzer.clone(),
            vault: Some(self.vault.clone()),
        };
        let (engine, events) = playback.start(config);

        self.listen(events, true, cx);
        self.local_engine = Some(engine);
        self.prepare_resume(cx);
    }

    /// Pumps an engine's events into `on_backend_event` until the engine is dropped.
    fn listen(&mut self, mut events: Box<dyn PlaybackEvents>, local: bool, cx: &mut Context<Self>) {
        let task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this
                    .update(cx, |this, cx| this.on_backend_event(event, local, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        match local {
            true => self.local_task = task,
            false => self.task = task,
        }
    }

    /// Applies what an engine reports. Events from the engine not playing the current track,
    /// or about another track, are dropped, so a late event from a previous load cannot move
    /// the clock.
    fn on_backend_event(&mut self, event: BackendEvent, local: bool, cx: &mut Context<Self>) {
        // an engine whose output has to reopen is rebuilt whether it plays the current track
        // or sits idle, since a dead engine would take the next track and play nothing
        if let BackendEvent::OutputChanged { heard } = event {
            self.restart_output(local, heard, cx);
            return;
        }
        // kept for whichever track it names, since a queued track reports before it plays
        if let BackendEvent::Signal { id, track } = event {
            self.learn_signal(id, track, cx);
            return;
        }
        if local != self.local_active() {
            return;
        }
        let current_id = self.track.as_ref().and_then(|track| track.id.as_deref());
        if let Some(event_id) = event.id()
            && current_id != Some(event_id)
        {
            return;
        }
        match event {
            // A Loading with the current id is the engine restarting the load at a seek
            // target, so a seek queued behind it must survive.
            BackendEvent::Position { .. }
            | BackendEvent::Length { .. }
            | BackendEvent::Downloaded { .. }
            | BackendEvent::Loading { .. } => {}
            BackendEvent::Playing { .. }
            | BackendEvent::Paused { .. }
            | BackendEvent::Seeked { .. } => {
                self.seek_in_flight = None;
            }
            _ => {
                self.seek_in_flight = None;
                self.seek_next = None;
            }
        }
        match event {
            // rebuilt or kept before the filters above
            BackendEvent::OutputChanged { .. } | BackendEvent::Signal { .. } => {}
            BackendEvent::Unavailable { .. }
            | BackendEvent::Throttled { .. }
            | BackendEvent::Refused
            | BackendEvent::Blocked
                if self.resume_ready =>
            {
                self.resume_ready = false;
                self.state = PlaybackState::Paused;
                log::warn!("playback: cannot hold the restored track, waiting for play");
            }
            BackendEvent::Playing { .. } if self.overtaken(Intent::Play) => {
                log::debug!("playback: ignoring a play the user has since paused");
            }
            BackendEvent::Paused { .. } if self.overtaken(Intent::Pause) => {
                log::debug!("playback: ignoring a pause the user has since undone");
            }
            BackendEvent::Loading { at, .. } => {
                self.state = PlaybackState::Loading;
                self.position = at;
                self.clock.reset(at, false);
            }
            BackendEvent::Playing { at, .. } => {
                let at = self.landed(at);
                let started = self.state != PlaybackState::Playing;
                self.intent = Intent::Play;
                self.pressed = None;
                self.state = PlaybackState::Playing;
                self.paused_at = None;
                self.failures = 0;
                self.throttles = 0;
                self.position = at;
                self.clock.reset(at, true);
                if started {
                    cx.emit(PlaybackEvent::StartedPlayback);
                }
                self.follow_up_seek(cx);
            }
            BackendEvent::Paused { at, .. } => {
                self.intent = Intent::Pause;
                self.pressed = None;
                self.state = PlaybackState::Paused;
                self.paused_at = Some(SystemTime::now());
                self.position = at;
                self.clock.reset(at, false);
                self.remember(true, cx);
                cx.emit(PlaybackEvent::Paused);
                self.follow_up_seek(cx);
            }
            BackendEvent::Seeked { at, .. } => {
                let at = self.landed(at);
                self.position = at;
                self.clock.reset(at, self.state == PlaybackState::Playing);
                self.remember(true, cx);
                cx.emit(PlaybackEvent::Seeked);
                self.follow_up_seek(cx);
            }
            BackendEvent::Position { at, .. } => {
                let mut failed = false;
                if self.seek_in_flight.is_some() {
                    self.stale_positions += 1;
                    if self.stale_positions <= STALE_POSITIONS {
                        return;
                    }
                    log::warn!("playback: the seek did not land, carrying on from {at:?}");
                    self.seek_in_flight = None;
                    failed = true;
                }
                self.position = at;
                match (self.state == PlaybackState::Playing, failed) {
                    (true, true) => self.clock.reset(at, true),
                    (true, false) => self.clock.correct(at),
                    (false, _) => self.clock.reset(at, false),
                }
                self.remember(false, cx);
                self.preload_next(at, cx);
                if self.fm_session {
                    // Keep the next FM track resolved while the current one plays, so a skip
                    // does not pay the full stream lookup again.
                    self.preload_upcoming(false, cx);
                }
                self.tick(cx);
                // a report that only moves the clock reaches the observers of `ticks` alone
                if !failed {
                    return;
                }
                self.follow_up_seek(cx);
            }
            BackendEvent::Downloaded { .. } => self.preload_upcoming(false, cx),
            BackendEvent::Length { duration, .. } => {
                if let Some(track) = self.track.as_mut()
                    && !duration.is_zero()
                    && track.duration != duration
                {
                    track.duration = duration;
                }
            }
            BackendEvent::Ended { .. } => {
                let ended = self.track.take();
                self.state = PlaybackState::Idle;
                self.position = Duration::ZERO;
                self.clock.reset(Duration::ZERO, false);
                cx.emit(PlaybackEvent::EndedPlayback);
                match self.sleep == Some(Sleep::EndOfTrack) {
                    true => self.doze(cx),
                    false => self.advance(ended, cx),
                }
            }
            BackendEvent::Throttled { .. } => self.throttled(cx),
            BackendEvent::Unavailable { .. } if self.ask_for_reconnect(cx) => {
                self.state = PlaybackState::Loading;
                log::warn!("playback: the provider went stale, waiting for a reconnect");
            }
            BackendEvent::Unavailable { .. } => {
                let failed = self.track.take();
                let target = failed.as_ref().and_then(song_target);
                let name = failed
                    .as_ref()
                    .map_or_else(|| "?".to_owned(), |track| track.name.clone());
                log::warn!(
                    "playback: {name} failed to load, backing off {}s",
                    KEY_COOLDOWN.as_secs()
                );
                self.blocked_until = Some(Instant::now() + KEY_COOLDOWN);
                self.failures = self.failures.saturating_add(1);
                self.state = PlaybackState::Idle;
                self.position = Duration::ZERO;
                self.clock.reset(Duration::ZERO, false);
                let exhausted = self.failures >= FAILURE_LIMIT;
                match exhausted {
                    true => {
                        log::warn!("playback: {FAILURE_LIMIT} tracks failed in a row, stopping");
                        Toasts::show(Outcome::Failed, "toast-playback-stopped", cx);
                    }
                    false => {
                        Toasts::linked(Outcome::Failed, "toast-track-unplayable", name, target, cx)
                    }
                }
                cx.emit(PlaybackEvent::EndedPlayback);
                match (exhausted || self.intent == Intent::Pause, self.repeat) {
                    (true, _) => self.hold_next(cx),
                    (false, Repeat::One) => self.segue_queue(cx),
                    (false, _) => self.advance(failed, cx),
                }
            }
            BackendEvent::Refused => {
                self.refuse(cx);
                cx.emit(PlaybackEvent::EndedPlayback);
            }
            BackendEvent::Gated => {
                self.gate(cx);
                cx.emit(PlaybackEvent::EndedPlayback);
            }
            BackendEvent::Blocked => self.network_blocked(cx),
        }
        cx.notify();
    }

    /// Drops the streaming engine and everything derived from its session on sign-out. A local
    /// track keeps playing.
    fn teardown(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.engine = None;
        self.stream_slug = None;
        self.signals.retain(|(id, _)| music::is_local_id(id));

        if !self.local_active() {
            self.load = None;
            self.fetch = None;
            self.sought = None;
            self.warm = None;
            self.enqueue = None;
            self.suggest = None;
            self.seeded = None;
            self.leave_station(cx);
            self.preloaded = None;
            self.skipped = None;
            self.blocked_until = None;
            self.failures = 0;
            self.throttles = 0;
            self.paused_at = None;
            self.refused = None;
            self.track = None;
            self.state = PlaybackState::Idle;
            self.position = Duration::ZERO;
            self.clock.reset(Duration::ZERO, false);
            self.resume_at = None;
            self.seek_in_flight = None;
            self.seek_next = None;
            self.seek_target = None;
            self.resume_ready = false;
            self.awaiting_reconnect = false;
            self.awaiting_start = None;
            self.resolve = None;
            self.no_resolve = false;
            self.stored = Duration::ZERO;
            self.sleep = None;
            self.sleep_task = None;
        }
        cx.notify();
    }

    /// Stops with a reason the UI can show.
    fn failed(&mut self, problem: String, cx: &mut Context<Self>) {
        log::error!("playback: {problem}");
        self.state = PlaybackState::Failed(problem);
        cx.notify();
    }

    /// Asks for the current track again after the provider turned it down for now. The wait
    /// doubles with each refusal in a row and only the first one toasts. Past `THROTTLE_LIMIT`,
    /// or once the user has paused, the track is held until play.
    fn throttled(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.track.clone() else {
            return;
        };
        let provider = self
            .session
            .read(cx)
            .provider_name()
            .unwrap_or("this provider")
            .to_owned();
        let at = self.position;
        self.throttles = self.throttles.saturating_add(1);
        if self.intent == Intent::Pause {
            return self.hold(at, cx);
        }
        if self.throttles > THROTTLE_LIMIT {
            log::warn!(
                "playback: {provider} kept turning {} down, waiting for play",
                track.name
            );
            Toasts::about(Outcome::Failed, "toast-still-throttled", provider, cx);
            return self.hold(at, cx);
        }
        let wait = throttle_wait(self.throttles);
        log::warn!(
            "playback: {provider} turned {} down for now, retrying in {}s",
            track.name,
            wait.as_secs()
        );
        if self.throttles == 1 {
            Toasts::about(Outcome::Failed, "toast-throttled", provider, cx);
        }
        self.blocked_until = Some(Instant::now() + wait);
        self.load_from(&track, at, Start::Retry, cx);
    }

    /// Holds the track the provider would not load because it refuses the network the app
    /// connects from. Every other track would fail the same way, so the queue stays put, and
    /// pressing play tries again once the listener has changed networks.
    fn network_blocked(&mut self, cx: &mut Context<Self>) {
        let provider = self
            .session
            .read(cx)
            .provider_name()
            .unwrap_or("this provider")
            .to_owned();
        log::warn!("playback: {provider} refuses requests from this network, waiting for play");
        Toasts::about(Outcome::Failed, "toast-network-blocked", provider, cx);
        self.hold(self.position, cx);
    }

    /// Turns down a track that has to be streamed while the network is gone. Whatever plays
    /// keeps playing, since a local file needs nothing, and only a track the user picked says
    /// so out loud: the queue moving on by itself would otherwise toast once a track.
    fn unreachable(reason: Unreachable, start: Start, cx: &mut App) {
        log::warn!("playback: nothing streams while the network is gone");
        if start == Start::Pick {
            reason.toast(cx);
        }
    }

    /// Why `track` cannot play here while the network is gone, or `None` when it can. A local
    /// file always plays.
    pub fn unreachable_reason(&self, track: &Track, cx: &App) -> Option<Unreachable> {
        self.reason(track.id.as_deref()?, cx)
    }

    fn reason(&self, id: &str, cx: &App) -> Option<Unreachable> {
        if music::is_local_id(id) || !Network::lost(cx) {
            return None;
        }
        Some(Unreachable::Offline)
    }

    /// Records that the provider wants a signed-in listener and stops until sign-in.
    fn gate(&mut self, cx: &mut Context<Self>) {
        let first = self.refused.is_none();
        self.refused = Some(Refusal::SignIn);
        self.track = None;
        self.blocked_until = None;
        let provider = self
            .session
            .read(cx)
            .provider_name()
            .unwrap_or("this provider");
        self.state = PlaybackState::Failed(format!(
            "{provider} only streams to a signed-in listener; nothing will play until you sign in"
        ));
        if first {
            log::warn!(
                "playback: {provider} only streams to a signed-in listener, nothing will play \
                 until you sign in"
            );
        }
        Toasts::about(
            Outcome::Failed,
            "toast-sign-in-to-play",
            provider.to_owned(),
            cx,
        );
        cx.notify();
    }

    /// Records that the provider refused this account and stops for the rest of the session,
    /// pointing at why.
    fn refuse(&mut self, cx: &mut Context<Self>) {
        let first = self.refused.is_none();
        self.refused = Some(Refusal::Keys);
        self.track = None;
        self.blocked_until = None;
        self.state = PlaybackState::Failed(
            "the provider refused this account; nothing will play in this session".to_owned(),
        );
        if first {
            log::error!(
                "playback: the provider refused this account; nothing will play in this session"
            );
        }
        let toast = Toast::new(Outcome::Failed, "toast-keys-error")
            .note("toast-keys-refused")
            .visit("toast-why", KEYS_FAQ);
        Toasts::post(toast, cx);
        cx.notify();
    }
}

fn song_target(track: &Track) -> Option<Target> {
    track
        .id
        .as_deref()
        .map(|id| Target::Song(SharedString::from(id.to_owned())))
}

/// Says which files handed in from the OS could not be opened. One file is named with its
/// reason, several are counted with their names below, and when others did open it is a
/// warning, since those still play.
fn report_unread(unread: &[Unread], nothing_read: bool, cx: &mut App) {
    match (unread, nothing_read) {
        ([], _) => {}
        ([only], true) => match &only.error {
            Some(error) => Toasts::with_detail(
                Outcome::Failed,
                "toast-open-failed",
                Some(only.name.clone().into()),
                error.clone(),
                cx,
            ),
            None => Toasts::about(
                Outcome::Failed,
                "toast-open-unsupported",
                only.name.clone(),
                cx,
            ),
        },
        (_, true) => {
            let toast = Toast::new(Outcome::Failed, "toast-open-all-failed")
                .count(unread.len())
                .detail(listed(unread));
            Toasts::post(toast, cx);
        }
        (_, false) => Toasts::with_detail(
            Outcome::Warning,
            "toast-open-some-failed",
            None,
            listed(unread),
            cx,
        ),
    }
}

/// The names of unreadable files for a toast's detail line, cut short after `LISTED_FILES`.
fn listed(unread: &[Unread]) -> String {
    let mut names = unread
        .iter()
        .take(LISTED_FILES)
        .map(|file| file.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if unread.len() > LISTED_FILES {
        names.push_str(", …");
    }
    names
}

/// How long to wait before the retry that follows the `throttles`th refusal in a row.
fn throttle_wait(throttles: u8) -> Duration {
    let doublings = u32::from(throttles.saturating_sub(1)).min(8);
    THROTTLE_WAIT
        .saturating_mul(1 << doublings)
        .min(THROTTLE_CAP)
}

/// Drops an engine on a thread of its own. An engine may wait for its player thread to end, which
/// sits out an output device that takes CoreAudio ten seconds to give up on, so dropping it here
/// would hold the window for as long.
fn retire(engine: Option<Box<dyn Player>>) {
    let Some(engine) = engine else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("engine-retire".to_owned())
        .spawn(move || drop(engine));
    if let Err(error) = spawned {
        log::warn!("playback: cannot retire the old engine off the main thread: {error}");
    }
}
/// The level `steps` notches of `VOLUME_STEP` from `level`, kept between silence and full volume.
fn volume_after(level: f32, steps: i8) -> f32 {
    (level + f32::from(steps) * VOLUME_STEP).clamp(0., 1.)
}

/// The position `SEEK_STEP` from `at` in the given direction, kept between the start of the track
/// and its end.
fn seek_target(at: Duration, total: Duration, forward: bool) -> Duration {
    match forward {
        true => at.saturating_add(SEEK_STEP).min(total),
        false => at.saturating_sub(SEEK_STEP),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        DEFAULT_UNMUTE_VOLUME, LiveClock, MuteMemory, VOLUME_STEP, gain, seek_target, volume_after,
    };

    #[test]
    fn never_amplifies_past_unity() {
        assert_eq!(gain(1.), 1.);
        for step in 0..=100 {
            assert!(gain(step as f32 / 100.) <= 1.);
        }
    }

    #[test]
    fn silences_a_closed_slider() {
        assert_eq!(gain(0.), 0.);
        assert_eq!(gain(-1.), 0.);
        assert_eq!(gain(2.), 1.);
    }

    #[test]
    fn rises_with_the_slider() {
        let mut last = gain(0.);
        for step in 1..=100 {
            let next = gain(step as f32 / 100.);
            assert!(next > last, "gain fell at {step}");
            last = next;
        }
    }

    #[test]
    fn matches_the_slider_linearly() {
        assert_eq!(gain(0.5), 0.5);
        assert_eq!(gain(0.15), 0.15);
    }

    #[test]
    fn mute_restores_the_last_volume_and_repeated_switches_are_safe() {
        let mut mute = MuteMemory::default();
        assert_eq!(mute.change(true, 0.62), Some(0.));
        assert_eq!(mute.change(true, 0.), None);
        assert_eq!(mute.change(false, 0.), Some(0.62));
        assert_eq!(mute.change(false, 0.62), None);
    }

    #[test]
    fn a_volume_change_while_muted_discards_the_stale_restore_level() {
        let mut mute = MuteMemory::default();
        assert_eq!(mute.change(true, 0.4), Some(0.));
        mute.volume_changed();
        assert_eq!(mute.change(false, 0.25), None);
    }

    #[test]
    fn mute_handles_zero_volume_without_inventing_a_restore_level() {
        let mut mute = MuteMemory::default();
        assert_eq!(mute.change(true, 0.), None);
        assert_eq!(mute.change(false, 0.), Some(DEFAULT_UNMUTE_VOLUME));
    }

    #[test]
    fn live_clock_does_not_jump_when_corrected() {
        let began = Instant::now();
        let mut clock = LiveClock {
            base: Duration::from_secs(10),
            since: Some(began),
            correction: 0.,
            settle: 1.,
            speed: 1.,
        };
        let corrected = began + Duration::from_millis(500);
        let before = clock.at(corrected);

        clock.correct_at(Duration::from_secs(10), corrected);

        assert_eq!(clock.at(corrected), before);
    }

    #[test]
    fn live_clock_covers_the_track_at_the_playback_speed() {
        let began = Instant::now();
        let clock = LiveClock {
            base: Duration::ZERO,
            since: Some(began),
            correction: 0.,
            settle: 1.,
            speed: 1.5,
        };
        assert_eq!(
            clock.at(began + Duration::from_secs(2)),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn live_clock_slews_backward_corrections_without_reversing() {
        let began = Instant::now();
        let corrected = began + Duration::from_secs(1);
        let mut clock = LiveClock {
            base: Duration::from_secs(10),
            since: Some(began),
            correction: 0.,
            settle: 1.,
            speed: 1.,
        };
        clock.correct_at(Duration::from_secs(9), corrected);

        let samples = (0..=30).map(|step| clock.at(corrected + Duration::from_millis(step * 100)));
        let mut previous = Duration::ZERO;
        for sample in samples {
            assert!(sample >= previous);
            previous = sample;
        }
    }

    #[test]
    fn volume_keys_move_by_the_wheel_step() {
        assert!((volume_after(0.5, 1) - (0.5 + VOLUME_STEP)).abs() < 1e-6);
        assert!((volume_after(0.5, -1) - (0.5 - VOLUME_STEP)).abs() < 1e-6);
        assert!((volume_after(0.5, 1) - 0.55).abs() < 1e-6);
    }

    #[test]
    fn volume_keys_stop_at_silence_and_full_volume() {
        assert_eq!(volume_after(0.98, 1), 1.);
        assert_eq!(volume_after(0.02, -1), 0.);
        assert_eq!(volume_after(1., 1), 1.);
    }

    #[test]
    fn seek_keys_move_five_seconds_each_way() {
        let total = Duration::from_secs(100);
        assert_eq!(
            seek_target(Duration::from_secs(10), total, true),
            Duration::from_secs(15)
        );
        assert_eq!(
            seek_target(Duration::from_secs(10), total, false),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn seek_keys_stop_at_the_start_and_end_of_the_track() {
        let total = Duration::from_secs(100);
        assert_eq!(
            seek_target(Duration::from_secs(2), total, false),
            Duration::ZERO
        );
        assert_eq!(seek_target(Duration::from_secs(98), total, true), total);
    }
}
