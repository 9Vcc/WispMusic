//! User preferences and runtime state
//!
//! Preferences a user sets on purpose go in `settings.json` as [`Values`]. Everything the app
//! changes on its own while running, such as the window frame or the volume, goes in
//! `state.sqlite` as [`StateValues`]. [`AppSettings`] holds both and saves each on its own
//! debounce, so a sidebar drag never rewrites the preferences file.

mod equalizer;
mod themes;

pub use self::equalizer::equalizer_profile_label;
pub use self::themes::{ThemeFamily, ThemeOffer, ThemeVariant};
pub(crate) use self::themes::{is_own_link, read_link as read_theme_link};

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use gpui::WindowDecorations;
use gpui::{
    App, Bounds, Context, DisplayId, EventEmitter, Pixels, Size, Subscription, Task, Window,
    WindowBounds, point, px, size,
};
use lyrics::LOCAL;
use music::{Quality, WritingSystem};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use storage::Database;
use ui::{
    Layout, Look, Mode, Pace, Pin, Rounding, RowSize, Saver, Sorting, Stillness, ThemeKind,
    ThemeOverrides,
};

use self::equalizer::Profile;
use crate::equalizer::DeviceRule;
use crate::pins::PinSort;
use crate::queue::{Resume, gap_target};
use crate::updates::Channel;
use crate::{Hit, Outcome, Repeat, Toasts, Wisp};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FullscreenControlsAutohide {
    #[default]
    Automatic,
    AlwaysShown,
    AlwaysHidden,
}

impl FullscreenControlsAutohide {
    pub const ALL: [Self; 3] = [Self::Automatic, Self::AlwaysShown, Self::AlwaysHidden];

    pub fn id(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::AlwaysHidden => "always-hidden",
            Self::AlwaysShown => "always-shown",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "automatic" => Self::Automatic,
            "always-hidden" => Self::AlwaysHidden,
            "always-shown" => Self::AlwaysShown,
            _ => Self::Automatic,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Automatic => "settings-fullscreen-controls-autohide-automatic",
            Self::AlwaysHidden => "settings-fullscreen-controls-autohide-always-hidden",
            Self::AlwaysShown => "settings-fullscreen-controls-autohide-always-shown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LyricMotionPreset {
    #[default]
    Wisp,
    AppleMusic,
}

impl LyricMotionPreset {
    pub const ALL: [Self; 2] = [Self::Wisp, Self::AppleMusic];

    pub fn id(self) -> &'static str {
        match self {
            Self::Wisp => "wisp",
            Self::AppleMusic => "apple-music",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "apple-music" => Self::AppleMusic,
            _ => Self::Wisp,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Wisp => "settings-lyrics-motion-wisp",
            Self::AppleMusic => "settings-lyrics-motion-apple-music",
        }
    }
}

/// Which panel the right sidebar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideTab {
    /// Settings saved while the members panel existed open the queue now.
    #[default]
    #[serde(alias = "members")]
    Queue,
    Lyrics,
}

/// The writing systems lyrics romanization applies to. Only CJK are enabled by default.
/// A partial object in `settings.json` keeps the defaults for the scripts it leaves out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RomanizationScripts {
    japanese: bool,
    chinese: bool,
    korean: bool,
    cyrillic: bool,
    greek: bool,
    arabic: bool,
    other: bool,
}

impl RomanizationScripts {
    pub fn contains(self, writing_system: WritingSystem) -> bool {
        match writing_system {
            WritingSystem::Japanese => self.japanese,
            WritingSystem::Chinese => self.chinese,
            WritingSystem::Korean => self.korean,
            WritingSystem::Cyrillic => self.cyrillic,
            WritingSystem::Greek => self.greek,
            WritingSystem::Arabic => self.arabic,
            WritingSystem::Other => self.other,
        }
    }

    fn set(&mut self, writing_system: WritingSystem, enabled: bool) {
        match writing_system {
            WritingSystem::Japanese => self.japanese = enabled,
            WritingSystem::Chinese => self.chinese = enabled,
            WritingSystem::Korean => self.korean = enabled,
            WritingSystem::Cyrillic => self.cyrillic = enabled,
            WritingSystem::Greek => self.greek = enabled,
            WritingSystem::Arabic => self.arabic = enabled,
            WritingSystem::Other => self.other = enabled,
        }
    }
}

impl Default for RomanizationScripts {
    fn default() -> Self {
        Self {
            japanese: true,
            chinese: true,
            korean: true,
            cyrillic: false,
            greek: false,
            arabic: false,
            other: false,
        }
    }
}

/// The views of the fullscreen player the visualizer is drawn in. Only the cover has it by
/// default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizerScreens {
    cover: bool,
    lyrics: bool,
    queue: bool,
}

impl VisualizerScreens {
    /// Whether the visualizer is drawn while the fullscreen player shows `panel`, which is the
    /// cover alone when there is none.
    pub fn contains(self, panel: Option<SideTab>) -> bool {
        match panel {
            None => self.cover,
            Some(SideTab::Lyrics) => self.lyrics,
            Some(SideTab::Queue) => self.queue,
        }
    }

    fn set(&mut self, panel: Option<SideTab>, shown: bool) {
        match panel {
            None => self.cover = shown,
            Some(SideTab::Lyrics) => self.lyrics = shown,
            Some(SideTab::Queue) => self.queue = shown,
        }
    }
}

impl Default for VisualizerScreens {
    fn default() -> Self {
        Self {
            cover: true,
            lyrics: false,
            queue: false,
        }
    }
}

/// A window's saved position and size in logical pixels, plus whether it was maximized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Frame {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    maximized: bool,
}

impl Frame {
    fn of(window: &Window) -> Self {
        let placement = window.window_bounds();
        let bounds = placement.get_bounds();
        Self {
            x: bounds.origin.x / px(1.),
            y: bounds.origin.y / px(1.),
            width: bounds.size.width / px(1.),
            height: bounds.size.height / px(1.),
            maximized: matches!(placement, WindowBounds::Maximized(_)),
        }
    }

    /// Rejects what a corrupt write could produce: a non-finite coordinate or an empty size.
    fn sane(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|it| it.is_finite())
            && self.width > 0.
            && self.height > 0.
    }

    /// Turns the frame back into a gpui placement, never smaller than `least`.
    fn placement(self, least: Size<Pixels>) -> WindowBounds {
        let bounds = Bounds {
            origin: point(px(self.x), px(self.y)),
            size: size(
                px(self.width).max(least.width),
                px(self.height).max(least.height),
            ),
        };
        match self.maximized {
            true => WindowBounds::Maximized(bounds),
            false => WindowBounds::Windowed(bounds),
        }
    }
}

fn system_font() -> String {
    SYSTEM_FONT.to_owned()
}

/// How long a save waits after the last change, so a slider drag lands as one write.
const SAVE_DELAY: Duration = Duration::from_millis(300);
const SAVE_RETRY_DELAY: Duration = Duration::from_secs(1);
const SAVE_RETRIES: usize = 3;
/// How long the watcher waits for files to go quiet before reading them.
const RELOAD_DELAY: Duration = Duration::from_millis(150);
const RELOAD_RETRY_DELAY: Duration = Duration::from_secs(1);
const RELOAD_RETRIES: usize = 3;
const THEMES_DIRECTORY: &str = "themes";
const DEFAULT_VOLUME: f32 = 0.7;
/// [wisp] 前置放大的边界（dB）。
pub const PREAMP_MIN: f32 = -12.;
pub const PREAMP_MAX: f32 = 12.;
/// [wisp] 解灰一次尝试的总预算边界（毫秒）。
pub const UNBLOCK_BUDGET_MIN_MS: u32 = 5_000;
pub const UNBLOCK_BUDGET_MAX_MS: u32 = 30_000;
pub const UNBLOCK_BUDGET_DEFAULT_MS: u32 = 15_000;
/// [wisp] 歌词偏移的边界（毫秒）：±10 秒足以覆盖不同母带的前奏差异。
pub const LYRIC_OFFSET_MAX_MS: i32 = 10_000;
/// [wisp] 歌词偏移微调的步长（毫秒）。
pub const LYRIC_OFFSET_NUDGE_MS: i32 = 500;
/// [wisp] 桌面歌词字号的边界：窗口大小决定字号，拖动窗口只会落在这个区间里。
pub const DESKTOP_LYRICS_FONT_MIN: f32 = 20.;
pub const DESKTOP_LYRICS_FONT_MAX: f32 = 96.;
/// [wisp] 桌面歌词笔画动效的字号倍率边界。
pub const DESKTOP_LYRICS_STROKE_SCALE_MIN: f32 = 0.5;
pub const DESKTOP_LYRICS_STROKE_SCALE_MAX: f32 = 2.;
/// How many chosen search results the history keeps, across every provider.
const PICKS: usize = 40;
const DEFAULT_SIDEBAR_WIDTH: f32 = 195.;
const DEFAULT_SIDEBAR_RIGHT_WIDTH: f32 = 254.;
const DEFAULT_FONT_SIZE: f32 = 14.;
const DEFAULT_LYRICS_SCALE: f32 = 1.;
const DEFAULT_STARTUP: &str = "home";
/// "Whatever the platform uses".
pub const SYSTEM_FONT: &str = "auto";
/// Every lyrics source Wisp knows by the name its provider reports, in the order a fresh
/// install prefers them. Only the three Wisp ships remain: the vendored sources for the
/// services Wisp no longer streams cannot answer, and a source that is offered but never
/// answers only slows every lookup down.
pub const LYRICS_SOURCES: [&str; 3] = [LOCAL, "Kugou", "NetEase"];

/// [wisp] 唯一流媒体 provider 的 slug。
const NETEASE: &str = "netease";
/// [wisp] 本地文件的 provider slug，与 `LocalProvider::slug()` 一致；它不进 providers 列表，
/// 也不代表启动会话，旧设置里的这个值需要迁移掉。
const LOCAL_PROVIDER: &str = "local";

/// A pin together with the provider it belongs to. The pin is flattened so the stored JSON
/// reads as a pin with one extra `slug` key.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Held {
    slug: String,
    #[serde(flatten)]
    pin: Pin,
}

/// A search result the listener chose, with the provider it belongs to.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Picked {
    slug: String,
    hit: Hit,
}

/// Everything `settings.json` holds. Preferences a user sets on purpose, safe to edit by hand.
/// Missing keys take their defaults, unknown keys are ignored.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Values {
    /// The provider to select on startup. Omit to restore the last-used provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    normalisation: bool,
    gapless: bool,
    /// [wisp] 缓存目录自定义；空 = 用 Wisp 默认的缓存目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_dir: Option<String>,
    /// [wisp] 新曲目的默认歌词偏移（毫秒，正值 = 歌词提前）；逐曲微调会覆盖它。
    lyric_offset_default_ms: i32,
    /// [wisp] 前置放大（dB，-12–+12），叠加在音量之外的整体增益。
    preamp_db: f32,
    /// [wisp] 解灰开关：灰歌拿不到直链时去外部音源找。
    unblock_enabled: bool,
    unblock_flac: bool,
    /// [wisp] true = 按固定顺序逐个尝试引擎；false = 全部引擎并发，谁先给出直链用谁。
    unblock_order_first: bool,
    /// [wisp] 一次解灰尝试的总预算（毫秒），限制在 `UNBLOCK_BUDGET_MIN_MS..=UNBLOCK_BUDGET_MAX_MS`。
    unblock_budget_ms: u32,
    unblock_source: String,
    unblock_qq_cookie: String,
    unblock_joox_cookie: String,
    /// Whether played tracks are kept, sealed, so playing one again downloads nothing.
    keep_tracks: bool,
    /// How many gigabytes the cache of played tracks may take.
    cache_gb: u32,
    equalizer: bool,
    /// The ten band gains an older Wisp saved. Read once to seed the profiles when there are
    /// none, never written back, and read leniently like the keys that replaced it.
    #[serde(skip_serializing, deserialize_with = "equalizer::read_gains")]
    equalizer_bands: Vec<f32>,
    /// The stored curves, kept even while `equalizer` is off. `equalizer_profile` picks the one
    /// that plays. Both read leniently, so a hand-edited mistake never makes the file unreadable.
    #[serde(deserialize_with = "equalizer::Profile::read_all")]
    equalizer_profiles: Vec<Profile>,
    #[serde(deserialize_with = "equalizer::read_index")]
    equalizer_profile: usize,
    /// Which sound plays once the sound moves to a device, one rule per device, read leniently
    /// like the profiles.
    #[serde(
        deserialize_with = "equalizer::read_rules",
        serialize_with = "equalizer::write_rules"
    )]
    equalizer_rules: Vec<DeviceRule>,
    /// Whether the equalizer screen shows its advanced controls, off so a first look shows only
    /// the switch, the curve and the presets.
    equalizer_advanced: bool,
    /// The bass and treble shelves laid over whichever sound plays, in decibels, read leniently
    /// like the profiles.
    #[serde(deserialize_with = "equalizer::read_tone")]
    equalizer_bass: f64,
    #[serde(deserialize_with = "equalizer::read_tone")]
    equalizer_treble: f64,
    lyrics_for_local_files: bool,
    /// The output device to play on, as a cpal device id, or `None` to follow the system's
    /// default. Kept while the device is gone, so playback returns to it, and cleared once it is
    /// there but will not open.
    output_device: Option<String>,
    /// The sample rate to open the output device at, in hertz, or `None` for the rate it runs
    /// at. A device that does not offer it opens at its own.
    output_rate: Option<u32>,
    /// The buffer to ask the output device for, in frames, or `None` for Wisp's own. A device
    /// that does not allow it gets the nearest it does.
    output_buffer: Option<u32>,
    /// Whether a direct output device is switched to the sample rate of each track that plays.
    output_match_rate: bool,
    /// The stream level asked of a streaming service, by its settings name. A service that does
    /// not serve it plays the nearest one it does.
    stream_quality: String,
    /// 播放速度（Wisp 变速控件），1.0 为正常速度。
    playback_speed: f32,
    /// Whether lyrics show anywhere and are looked up at all.
    lyrics_enabled: bool,
    lyrics_providers: Vec<String>,
    /// Every known lyrics source, the preferred one first. Lyrics of the same kind from two
    /// sources go to the one listed higher.
    lyrics_provider_priority: Vec<String>,
    /// Whether lyrics timed word by word are shown ahead of any others, whatever source they
    /// come from. The source order then only settles lyrics of the same kind. The JSON key is
    /// deliberately new, so an old `false` does not hide the new default.
    lyrics_prefer_word_synced: bool,
    karaoke_lyrics: bool,
    blur_lyrics: bool,
    /// Multiplier on the blur of background lyrics, 1.0 being the default strength.
    lyrics_blur_amount: f32,
    fade_lyrics: bool,
    edge_feather_lyrics: bool,
    distance_scale_lyrics: bool,
    breathing_lyrics: bool,
    gap_dots_lyrics: bool,
    /// 笔画歌词动效：当前句逐字按笔画飞入，句尾散开。
    stroke_lyrics: bool,
    /// 桌面歌词悬浮窗口是否开启。
    desktop_lyrics: bool,
    /// 桌面歌词字号（逻辑像素）。
    desktop_lyrics_font_size: f32,
    /// 桌面歌词是否竖排。
    desktop_lyrics_vertical: bool,
    /// 桌面歌词是否使用笔画动效。
    desktop_lyrics_stroke: bool,
    /// 桌面歌词颜色是否跟随封面。
    desktop_lyrics_cover_color: bool,
    /// 桌面歌词是否锁定：锁定后鼠标穿透，拖动和缩放都停下来，悬停时才浮出解锁键。
    desktop_lyrics_locked: bool,
    /// 桌面歌词笔画动效的字号倍率，1.0 与普通歌词同尺寸。
    desktop_lyrics_stroke_scale: f32,
    /// 歌词页逐字填充是否跟随封面。
    lyrics_cover_color: bool,
    parallax_lyrics: bool,
    lyrics_motion_preset: LyricMotionPreset,
    romanized_lyrics: bool,
    panel_lyrics_scale: f32,
    fullscreen_lyrics_scale: f32,
    romanization_scripts: RomanizationScripts,
    adaptive_menu: bool,
    /// The release channel chosen in settings, by its id. Empty follows the running build's own
    /// channel, and a copy a package manager installed ignores it.
    update_channel: String,
    close_to_tray: bool,
    tray_icon: bool,
    stay_awake: bool,
    /// Whether a file handed in by the OS goes straight to the now playing view.
    now_playing_on_file_open: bool,
    /// Whether the Windows taskbar button fills with the track's progress while it plays. Off
    /// clears the fill, so the button shows only its icon.
    taskbar_progress: bool,
    language: String,
    #[serde(default = "system_font")]
    font: String,
    startup: String,
    /// [wisp] 首启是否播放应用标记动画。
    startup_animation: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    local_folders: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hidden_nav: Vec<String>,
    /// Missing system requirements the user asked never to hear about again.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    silenced_requirements: Vec<String>,
    /// Providers, by slug, whose plays are not recorded on the provider's own server.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    unscrobbled: BTreeSet<String>,
    /// The keys the user chose for shortcuts under Settings, Keybinds, by shortcut id. A shortcut
    /// missing here keeps its defaults, and an empty list leaves it without a key.
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "read_keybinds"
    )]
    keybinds: BTreeMap<String, Vec<String>>,
    appearance: Appearance,
}

/// The arrangement of the workspace's player bar, independent of its surface material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayerBarLayout {
    #[default]
    Classic,
    Centered,
    Compact,
    /// A rounded pill that floats over the page instead of a bar across the window's foot.
    Island,
}

impl PlayerBarLayout {
    pub const ALL: [Self; 4] = [Self::Classic, Self::Centered, Self::Compact, Self::Island];

    pub fn id(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Centered => "centered",
            Self::Compact => "compact",
            Self::Island => "island",
        }
    }

    /// The label's Fluent key, resolved by the view when it renders the layout cards.
    pub fn key(self) -> &'static str {
        match self {
            Self::Classic => "settings-player-bar-classic",
            Self::Centered => "settings-player-bar-centered",
            Self::Compact => "settings-player-bar-compact",
            Self::Island => "settings-player-bar-island",
        }
    }

    /// Reads a saved layout id, keeping the original bar for missing or unknown choices.
    pub fn from_id(id: &str) -> Self {
        match id {
            "centered" => Self::Centered,
            "compact" => Self::Compact,
            "island" => Self::Island,
            _ => Self::Classic,
        }
    }
}

/// How wide pages stretch on a wide window. `Centered` caps a page at `metrics.page` and stands
/// it in the middle of the content area, `Wide` lets it fill the whole content area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageWidth {
    #[default]
    Centered,
    Wide,
}

impl PageWidth {
    pub const ALL: [Self; 2] = [Self::Centered, Self::Wide];

    pub fn id(self) -> &'static str {
        match self {
            Self::Centered => "centered",
            Self::Wide => "wide",
        }
    }

    /// The label's Fluent key, resolved by the view when it renders the option cards.
    pub fn key(self) -> &'static str {
        match self {
            Self::Centered => "settings-page-width-centered",
            Self::Wide => "settings-page-width-wide",
        }
    }

    /// Reads a saved width id, keeping the centred layout for missing or unknown choices.
    pub fn from_id(id: &str) -> Self {
        match id {
            "wide" => Self::Wide,
            _ => Self::Centered,
        }
    }
}

/// The `appearance` block of `settings.json`. Fixed choices are stored by id, removed variant
/// falls back to the default instead of failing the parse.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Appearance {
    theme: String,
    adaptive_theme: bool,
    ambient: bool,
    ambient_motion: bool,
    visualizer: bool,
    visualizer_style: String,
    /// Whether the visualizer draws the track at its own level rather than at the volume.
    visualizer_absolute: bool,
    visualizer_screens: VisualizerScreens,
    player_bar_layout: String,
    player_bar_transparent: bool,
    /// Whether the player bar's right clock counts down what is left of the track instead of
    /// showing its length.
    player_bar_time_left: bool,
    icons: String,
    rounding: String,
    /// How tall table rows stand, by its id. See `RowSize`.
    row_size: String,
    /// How wide pages stretch on a wide window, by its id. See `PageWidth`.
    page_width: String,
    /// Whether the app paints its frosted treatments. The key kept its old name, which stood
    /// for a blurred desktop behind the window, so a stored preference carries over.
    blur: bool,
    /// Whether a see-through window asks the platform to blur the desktop behind it.
    blur_window: bool,
    font_size: f32,
    transparent: bool,
    transparency: f32,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    server_side_decorations: bool,
    /// The window's own corner rounding, independent of `rounding` (the UI element radius).
    /// On Windows this maps onto DWM's two fixed presets and on macOS onto the toolbar styles
    /// AppKit sizes its corners by. On Linux/FreeBSD it only has an effect with client-side
    /// decorations, since server-side ones are the compositor's call.
    #[cfg(any(
        target_os = "windows",
        target_os = "linux",
        target_os = "freebsd",
        target_os = "macos"
    ))]
    window_rounding: String,
    window_controls: bool,
    #[cfg(not(target_os = "macos"))]
    traffic_light_controls: bool,
    controls_on_left: bool,
    reduce_motion: String,
    motion_pace: String,
    battery_saver: String,
    theme_overrides: ThemeOverrides,
    fullscreen_controls_autohide: String,
    os_fullscreen: bool,
}

/// Which watched sources may have changed in one filesystem event.
#[derive(Clone, Copy, Default)]
struct FileChanges {
    settings: bool,
    themes: bool,
}

impl FileChanges {
    const ALL: Self = Self {
        settings: true,
        themes: true,
    };

    fn merge(&mut self, other: Self) {
        self.settings |= other.settings;
        self.themes |= other.themes;
    }

    fn any(self) -> bool {
        self.settings || self.themes
    }
}

/// The result of trying to reload `settings.json`.
#[derive(Clone, Copy)]
enum SettingsReload {
    Unchanged,
    Changed,
    Retry,
}

/// Whether a settings save should be tried again after a transient failure, which carries the
/// error so the last attempt can show it.
enum SettingsSave {
    Complete,
    Retry(String),
}

impl Default for Values {
    fn default() -> Self {
        Self {
            provider: None,
            normalisation: false,
            gapless: true,
            cache_dir: None,
            // [wisp] 默认歌词偏移：默认不动，逐曲微调会覆盖它。
            lyric_offset_default_ms: 0,
            // [wisp] 前置放大（dB，-12–+12），叠加在音量之外的整体增益。默认不动。
            preamp_db: 0.,
            // [wisp] 解灰（Wisp network 设置）：开关、优先无损、音源、两个 Cookie。
            // 默认关闭；装有网易云插件时由插件目录同步开启。
            unblock_enabled: false,
            unblock_flac: true,
            unblock_order_first: false,
            unblock_budget_ms: UNBLOCK_BUDGET_DEFAULT_MS,
            // 空 = 自动（全部引擎），否则逗号分隔的引擎名。
            unblock_source: String::new(),
            unblock_qq_cookie: String::new(),
            unblock_joox_cookie: String::new(),
            keep_tracks: true,
            cache_gb: crate::vault::CACHE_GB,
            equalizer: false,
            equalizer_bands: Vec::new(),
            equalizer_profiles: Vec::new(),
            equalizer_profile: 0,
            equalizer_rules: Vec::new(),
            equalizer_advanced: false,
            equalizer_bass: 0.,
            equalizer_treble: 0.,
            lyrics_for_local_files: true,
            output_device: None,
            output_rate: None,
            output_buffer: None,
            output_match_rate: false,
            stream_quality: Quality::default().id().to_owned(),
            playback_speed: 1.0,
            lyrics_enabled: true,
            lyrics_provider_priority: lyrics_priority(&[]),
            // [wisp] 三个源全部默认开启：本地补自有文件，酷狗补中文曲库，网易云给 YRC 逐字。
            lyrics_providers: LYRICS_SOURCES
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            lyrics_prefer_word_synced: true,
            karaoke_lyrics: true,
            blur_lyrics: true,
            lyrics_blur_amount: 1.,
            fade_lyrics: true,
            edge_feather_lyrics: true,
            distance_scale_lyrics: false,
            breathing_lyrics: false,
            gap_dots_lyrics: false,
            stroke_lyrics: false,
            desktop_lyrics: false,
            desktop_lyrics_font_size: 40.,
            desktop_lyrics_vertical: false,
            desktop_lyrics_stroke: false,
            desktop_lyrics_cover_color: false,
            desktop_lyrics_locked: false,
            desktop_lyrics_stroke_scale: 1.,
            lyrics_cover_color: false,
            parallax_lyrics: false,
            lyrics_motion_preset: LyricMotionPreset::Wisp,
            romanized_lyrics: true,
            panel_lyrics_scale: DEFAULT_LYRICS_SCALE,
            fullscreen_lyrics_scale: DEFAULT_LYRICS_SCALE,
            romanization_scripts: RomanizationScripts::default(),
            adaptive_menu: false,
            update_channel: String::new(),
            close_to_tray: true,
            tray_icon: true,
            stay_awake: true,
            now_playing_on_file_open: true,
            taskbar_progress: true,
            language: i18n::AUTO.to_owned(),
            font: system_font(),
            startup: DEFAULT_STARTUP.to_owned(),
            startup_animation: true,
            local_folders: Vec::new(),
            hidden_nav: Vec::new(),
            silenced_requirements: Vec::new(),
            unscrobbled: BTreeSet::new(),
            keybinds: BTreeMap::new(),
            appearance: Appearance::default(),
        }
    }
}

impl Values {
    /// The values as they are used, with whatever a hand edit or an older Wisp left settled.
    fn settled(mut self) -> Self {
        self.settle_equalizer();
        self.settle_provider();
        self
    }

    /// [wisp] Provider slugs are now manifest-driven. Keep a saved slug even when no plugin is
    /// installed right now; the session picks the first available provider at runtime. Only an
    /// explicitly blank value is dropped so the file does not store a meaningless choice.
    fn settle_provider(&mut self) {
        if self
            .provider
            .as_deref()
            .is_some_and(|provider| provider.trim().is_empty() || provider == LOCAL_PROVIDER)
        {
            self.provider = None;
        }
    }
}

/// One narrowed filter axis as stored per table: a flag that is on, or a range the user
/// shrank. Whole ranges and flags that are off read as untouched and take no space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FilterValue {
    Flag(bool),
    Range(f32, f32),
}

/// Everything `state.sqlite` holds under the `runtime` key: values the app changes on its own
/// while running, so saving them never rewrites `settings.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct StateValues {
    volume: f32,
    sidebar_width: f32,
    sidebar_open: bool,
    sidebar_rail: bool,
    sidebar_right_width: f32,
    sidebar_right_open: bool,
    sidebar_right_tab: SideTab,
    fullscreen_tab: Option<SideTab>,
    shuffle: bool,
    repeat: Repeat,
    radio: bool,
    /// Claim the provider's daily membership reward when the app signs in.
    auto_checkin: bool,
    /// The last day a check-in succeeded, as `YYYY-MM-DD`.
    last_checkin_date: String,
    last_checkin_claimed: bool,
    last_checkin_upgraded: bool,
    /// The FM recommendation mode: normal, small, peak or radio.
    fm_mode: String,
    /// The FM song pool: 0 taste, 1 style, 2 explore.
    fm_song_pool: u32,
    provider: String,
    tables: HashMap<String, Layout>,
    sorting: HashMap<String, Option<Sorting>>,
    filters: HashMap<String, HashMap<String, FilterValue>>,
    views: HashMap<String, Mode>,
    pinned: Vec<Held>,
    sidebar_pinned_open: bool,
    sidebar_full_library: bool,
    sidebar_pin_sort: String,
    sidebar_pin_reversed: bool,
    resume: Option<Resume>,
    window: Option<Frame>,
    /// [wisp] 桌面歌词在横排、竖排下各自最后一次的窗口尺寸（宽、高，逻辑像素）。
    desktop_lyrics_wide: Option<[f32; 2]>,
    desktop_lyrics_tall: Option<[f32; 2]>,
    system_theme: String,
    /// The variant last chosen in each theme family, by family id.
    theme_variants: HashMap<String, String>,
    /// The search results the listener chose, newest first, without repeats.
    picks: Vec<Picked>,
    /// [wisp] 歌词偏移（毫秒，正值 = 歌词提前），按歌曲 id 记忆。
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    lyric_offsets: HashMap<String, i32>,
    /// [wisp] 手动选定的歌词版本（歌曲 id → 源名），没有记录即按优先级自动选。
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    lyric_versions: HashMap<String, String>,
}

impl Default for StateValues {
    fn default() -> Self {
        Self {
            volume: DEFAULT_VOLUME,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            sidebar_open: true,
            sidebar_rail: false,
            sidebar_right_width: DEFAULT_SIDEBAR_RIGHT_WIDTH,
            sidebar_right_open: false,
            sidebar_right_tab: SideTab::Queue,
            fullscreen_tab: Some(SideTab::Lyrics),
            shuffle: false,
            repeat: Repeat::Off,
            radio: true,
            auto_checkin: true,
            last_checkin_date: String::new(),
            last_checkin_claimed: false,
            last_checkin_upgraded: false,
            fm_mode: "normal".to_owned(),
            fm_song_pool: 0,
            provider: NETEASE.to_owned(),
            tables: HashMap::new(),
            sorting: HashMap::new(),
            filters: HashMap::new(),
            views: HashMap::new(),
            pinned: Vec::new(),
            sidebar_pinned_open: false,
            sidebar_full_library: false,
            sidebar_pin_sort: String::new(),
            sidebar_pin_reversed: false,
            resume: None,
            window: None,
            desktop_lyrics_wide: None,
            desktop_lyrics_tall: None,
            system_theme: ThemeKind::Dark.id().to_owned(),
            theme_variants: HashMap::new(),
            picks: Vec::new(),
            lyric_offsets: HashMap::new(),
            lyric_versions: HashMap::new(),
        }
    }
}

impl StateValues {
    /// [wisp] The runtime state as it is used, with whatever an older Wisp left settled. Only the
    /// last-used provider needs settling: it is a slug, and the slugs of the providers Wisp no
    /// longer ships would select nothing at startup.
    fn settled(mut self) -> Self {
        if self.provider.trim().is_empty() || self.provider == LOCAL_PROVIDER {
            self.provider = NETEASE.to_owned();
        }
        // A resumed queue belongs to whatever provider is available at launch; dynamic plugin
        // slugs are not rewritten here.
        self
    }
}

/// The `runtime` row of the `app_state` table: one JSON blob, replaced whole on every save.
/// Every call goes through one kept connection, because playback saves every few seconds and
/// opening the database again parses its whole schema each time.
struct StateStore {
    database: Database,
    connection: RefCell<Option<rusqlite::Connection>>,
}

impl StateStore {
    fn new(database: Database) -> Self {
        Self {
            database,
            connection: RefCell::new(None),
        }
    }

    /// Runs `work` on the kept connection, opening one when there is none. A failure drops the
    /// connection, so the next call starts from a fresh one.
    fn with<T>(&self, work: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        let mut kept = self.connection.borrow_mut();
        let connection = match kept.take() {
            Some(connection) => connection,
            None => self.database.open()?,
        };
        let done = work(&connection);
        if done.is_ok() {
            *kept = Some(connection);
        }
        done
    }

    fn load(&self) -> Result<Option<StateValues>> {
        let encoded: Option<String> = self.with(|connection| {
            connection
                .query_row(
                    "SELECT value FROM app_state WHERE key = 'runtime'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .context("cannot read app state")
        })?;
        encoded
            .map(|encoded| serde_json::from_str(&encoded).context("cannot decode app state"))
            .transpose()
    }

    fn save(&self, state: &StateValues) -> Result<()> {
        let encoded = serde_json::to_string(state).context("cannot encode app state")?;
        self.with(|connection| {
            connection
                .execute(
                    "INSERT INTO app_state (key, value) VALUES ('runtime', ?)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![encoded],
                )
                .context("cannot save app state")
        })?;
        Ok(())
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeKind::System.id().to_owned(),
            adaptive_theme: false,
            ambient: true,
            ambient_motion: true,
            visualizer: true,
            visualizer_style: ui::VisualizerStyle::default().id().to_owned(),
            visualizer_absolute: false,
            visualizer_screens: VisualizerScreens::default(),
            player_bar_layout: PlayerBarLayout::default().id().to_owned(),
            player_bar_transparent: true,
            player_bar_time_left: false,
            icons: icons::BASE.to_owned(),
            rounding: Rounding::Rounded.id().to_owned(),
            row_size: RowSize::default().id().to_owned(),
            page_width: PageWidth::default().id().to_owned(),
            blur: true,
            blur_window: true,
            font_size: DEFAULT_FONT_SIZE,
            transparent: false,
            transparency: ui::BACKDROP_TRANSPARENCY,
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            server_side_decorations: true,
            #[cfg(target_os = "windows")]
            window_rounding: Rounding::Rounded.id().to_owned(),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            window_rounding: Rounding::Square.id().to_owned(),
            #[cfg(target_os = "macos")]
            window_rounding: Rounding::Subtle.id().to_owned(),
            window_controls: true,
            #[cfg(not(target_os = "macos"))]
            traffic_light_controls: false,
            controls_on_left: false,
            reduce_motion: Stillness::default().id().to_owned(),
            motion_pace: Pace::default().id().to_owned(),
            battery_saver: Saver::default().id().to_owned(),
            theme_overrides: ThemeOverrides::default(),
            fullscreen_controls_autohide: FullscreenControlsAutohide::Automatic.id().to_owned(),
            os_fullscreen: false,
        }
    }
}

/// Preferences from `settings.json` and runtime state from `state.sqlite`, each saved on its own
/// debounce. `broken` protects nonempty invalid JSON from being overwritten with defaults.
pub struct AppSettings {
    values: Values,
    state: StateValues,
    path: PathBuf,
    /// The theme files in the user's themes folder.
    themes: Vec<ThemeVariant>,
    /// The bundled themes and `themes`, grouped the way the Appearance page lists them.
    families: Vec<ThemeFamily>,
    store: StateStore,
    save: Option<Task<()>>,
    save_state: Option<Task<()>>,
    watch: Option<Subscription>,
    writable: bool,
    /// What `settings.json` held when it was last read or written, so the watcher can tell our
    /// own writes from another program's.
    disk: Option<Vec<u8>>,
    /// The line of the parse error while `settings.json` does not parse.
    broken: Option<usize>,
    /// The watch on the config folder. Dropping it ends the watch.
    watcher: Option<RecommendedWatcher>,
    reload: Option<Task<()>>,
    /// Set once a save gives up after its retries, so the failure toast shows once until a
    /// save lands again.
    failing: bool,
}

/// Emitted after settings reload or the selected custom palette changes.
/// Theme owners repaint because a reload never calls `Theme::set` itself.
pub struct Reloaded;

impl EventEmitter<Reloaded> for AppSettings {}

impl AppSettings {
    /// Loads from the standard config and data paths.
    pub fn load(database: Database) -> Self {
        Self::load_from(settings_path(), StateStore::new(database))
    }

    /// A missing or unreadable `state.sqlite` row yields the default runtime state.
    fn load_from(path: PathBuf, store: StateStore) -> Self {
        let (bytes, writable) = match fs::read(&path) {
            Ok(bytes) => (Some(bytes), true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, true),
            Err(error) => {
                log::warn!("settings: cannot read {}: {error}", path.display());
                (None, false)
            }
        };
        let parsed = bytes.map(|bytes| (serde_json::from_slice::<Values>(json(&bytes)), bytes));
        let parsed = parsed.map(|(values, bytes)| (values.map(offer_new_lyrics), bytes));
        let (values, writable, disk, broken) = match parsed {
            Some((Ok(values), bytes)) => (values.settled(), writable, Some(bytes), None),
            Some((Err(_), bytes)) if empty_settings(&bytes) => {
                (Values::default().settled(), writable, Some(bytes), None)
            }
            Some((Err(error), _)) => {
                log::warn!("settings: cannot parse {}: {error}", path.display());
                (Values::default().settled(), false, None, Some(error.line()))
            }
            // Nothing to settle on a fresh install: the defaults already hold the sources Wisp ships.
            None => (Values::default().settled(), writable, None, None),
        };

        let state = match store.load() {
            Ok(Some(saved)) => saved.settled(),
            Ok(None) => StateValues::default(),
            Err(error) => {
                log::warn!("settings: cannot load app state: {error:#}");
                StateValues::default()
            }
        };
        let themes = themes::load(&themes_path(&path), &[]).themes;
        let families = themes::catalog(&themes);

        Self {
            values,
            state,
            path,
            themes,
            families,
            store,
            save: None,
            save_state: None,
            watch: None,
            writable,
            disk,
            broken,
            watcher: None,
            reload: None,
            failing: false,
        }
    }

    /// Tells the user that `settings.json` does not parse and that changes are not saved until
    /// it does. Does nothing while the file is fine.
    pub fn report_broken(&self, cx: &mut App) {
        if let Some(line) = self.broken {
            Toasts::about(
                Outcome::Failed,
                "toast-settings-broken",
                line.to_string(),
                cx,
            );
        }
    }

    pub fn volume(&self) -> f32 {
        self.state.volume.clamp(0., 1.)
    }

    pub fn normalisation(&self) -> bool {
        self.values.normalisation
    }

    pub fn gapless(&self) -> bool {
        self.values.gapless
    }

    /// [wisp] 自定义缓存目录；`None` 表示用默认位置。
    pub fn cache_dir(&self) -> Option<&str> {
        self.values
            .cache_dir
            .as_deref()
            .filter(|dir| !dir.is_empty())
    }

    /// [wisp] 默认歌词偏移（毫秒，正值 = 歌词提前），限制在 ±10 秒。
    pub fn lyric_offset_default_ms(&self) -> i32 {
        self.values
            .lyric_offset_default_ms
            .clamp(-LYRIC_OFFSET_MAX_MS, LYRIC_OFFSET_MAX_MS)
    }

    /// [wisp] 某首歌的歌词偏移（毫秒）：设置过读曲目值，否则读全局默认。
    pub fn lyric_offset(&self, id: &str) -> i32 {
        self.state
            .lyric_offsets
            .get(id)
            .copied()
            .unwrap_or_else(|| self.lyric_offset_default_ms())
            .clamp(-LYRIC_OFFSET_MAX_MS, LYRIC_OFFSET_MAX_MS)
    }

    /// [wisp] 某首歌手动选定的歌词源。
    pub fn lyric_version(&self, id: &str) -> Option<&str> {
        self.state.lyric_versions.get(id).map(String::as_str)
    }

    /// [wisp] 记住某首歌选定的歌词源，`None` 即回到自动优先级。静默保存。
    pub fn set_lyric_version(&mut self, id: &str, source: Option<&str>, cx: &mut Context<Self>) {
        let changed = match source {
            Some(source) => self
                .state
                .lyric_versions
                .insert(id.to_owned(), source.to_owned())
                .is_none_or(|held| held != source),
            None => self.state.lyric_versions.remove(id).is_some(),
        };
        if changed {
            self.save_state_quietly(cx);
        }
    }

    /// [wisp] 记住某首歌的歌词偏移；等于全局默认即删掉这条记录。静默保存，不打断观察者。
    pub fn set_lyric_offset(&mut self, id: &str, offset_ms: i32, cx: &mut Context<Self>) {
        let offset_ms = offset_ms.clamp(-LYRIC_OFFSET_MAX_MS, LYRIC_OFFSET_MAX_MS);
        let changed = match offset_ms == self.lyric_offset_default_ms() {
            true => self.state.lyric_offsets.remove(id).is_some(),
            false => self.state.lyric_offsets.insert(id.to_owned(), offset_ms) != Some(offset_ms),
        };
        if changed {
            self.save_state_quietly(cx);
        }
    }

    /// [wisp] 新曲目的默认歌词偏移；正在播放的曲目保持它自己的值。
    pub fn set_lyric_offset_default_ms(&mut self, offset_ms: i32, cx: &mut Context<Self>) {
        let offset_ms = offset_ms.clamp(-LYRIC_OFFSET_MAX_MS, LYRIC_OFFSET_MAX_MS);
        if self.values.lyric_offset_default_ms == offset_ms {
            return;
        }
        self.values.lyric_offset_default_ms = offset_ms;
        self.schedule_save(cx);
    }

    pub fn set_cache_dir(&mut self, dir: Option<&str>, cx: &mut Context<Self>) {
        let dir = dir.map(str::to_owned).filter(|dir| !dir.is_empty());
        if self.values.cache_dir == dir {
            return;
        }
        self.values.cache_dir = dir;
        self.schedule_save(cx);
    }

    /// [wisp] 前置放大，dB，限制在 ±12。
    pub fn preamp_db(&self) -> f32 {
        self.values.preamp_db.clamp(PREAMP_MIN, PREAMP_MAX)
    }

    /// [wisp] 解灰：开关 / 优先无损 / 音源（空 = 自动）/ QQ、Joox Cookie。
    pub fn unblock_enabled(&self) -> bool {
        self.values.unblock_enabled
    }

    pub fn unblock_flac(&self) -> bool {
        self.values.unblock_flac
    }

    /// [wisp] 解灰顺序优先：true 逐个尝试引擎，false 并发抢最快。
    pub fn unblock_order_first(&self) -> bool {
        self.values.unblock_order_first
    }

    /// [wisp] 解灰总预算（毫秒），限制在 5–30 秒。
    pub fn unblock_budget_ms(&self) -> u32 {
        self.values
            .unblock_budget_ms
            .clamp(UNBLOCK_BUDGET_MIN_MS, UNBLOCK_BUDGET_MAX_MS)
    }

    pub fn unblock_source(&self) -> &str {
        &self.values.unblock_source
    }

    pub fn unblock_qq_cookie(&self) -> &str {
        &self.values.unblock_qq_cookie
    }

    pub fn unblock_joox_cookie(&self) -> &str {
        &self.values.unblock_joox_cookie
    }

    /// 把解灰设置灌成 music 侧的共享配置；Cookie 空串视为未填。
    pub fn unblock_config(&self) -> host_sources::UnblockConfig {
        host_sources::UnblockConfig::new(
            self.values.unblock_enabled,
            self.values.unblock_flac,
            self.values.unblock_order_first,
            self.unblock_budget_ms(),
            self.values.unblock_source.clone(),
            (!self.values.unblock_qq_cookie.is_empty())
                .then(|| self.values.unblock_qq_cookie.clone()),
            (!self.values.unblock_joox_cookie.is_empty())
                .then(|| self.values.unblock_joox_cookie.clone()),
        )
    }

    pub fn keep_tracks(&self) -> bool {
        self.values.keep_tracks
    }

    /// The cache limit in gigabytes, held within the setting's bounds whatever a hand-edited
    /// settings file says.
    pub fn cache_gb(&self) -> u32 {
        let (low, high) = crate::vault::CACHE_GB_RANGE;
        self.values.cache_gb.clamp(low, high)
    }

    /// Whether lyrics are fetched for local files.
    pub fn lyrics_for_local_files(&self) -> bool {
        self.values.lyrics_for_local_files
    }

    pub fn output_device(&self) -> Option<&str> {
        self.values.output_device.as_deref()
    }

    pub fn output_rate(&self) -> Option<u32> {
        self.values.output_rate
    }

    pub fn output_buffer(&self) -> Option<u32> {
        self.values.output_buffer
    }

    pub fn output_match_rate(&self) -> bool {
        self.values.output_match_rate
    }

    /// The stream level asked of a streaming service. A level this build does not know reads as
    /// the best.
    pub fn quality(&self) -> Quality {
        Quality::from_id(&self.values.stream_quality).unwrap_or_default()
    }

    pub fn playback_speed(&self) -> f32 {
        self.values.playback_speed.clamp(0.5, 2.0)
    }

    pub fn lyrics_enabled(&self) -> bool {
        self.values.lyrics_enabled
    }

    pub fn lyrics_providers(&self) -> &[String] {
        &self.values.lyrics_providers
    }

    /// Every known lyrics source, the preferred one first, disabled ones included.
    pub fn lyrics_provider_priority(&self) -> Vec<String> {
        lyrics_priority(&self.values.lyrics_provider_priority)
    }

    /// The enabled lyrics sources, the preferred one first.
    pub fn enabled_lyrics_provider_priority(&self) -> Vec<String> {
        self.lyrics_provider_priority()
            .into_iter()
            .filter(|name| self.lyrics_provider_enabled(name))
            .collect()
    }

    /// Moves an enabled source into `gap`, counted between the enabled sources from 0 before the
    /// first to their count after the last. Disabled sources keep their place.
    pub fn place_lyrics_provider(&mut self, provider: &str, gap: usize, cx: &mut Context<Self>) {
        let mut order = self.lyrics_provider_priority();
        if !place_lyrics_provider(&mut order, &self.values.lyrics_providers, provider, gap) {
            return;
        }
        self.values.lyrics_provider_priority = order;
        self.schedule_save(cx);
    }

    /// Whether lyrics timed word by word are shown ahead of any others, whatever their source.
    pub fn lyrics_word_synced_first(&self) -> bool {
        self.values.lyrics_prefer_word_synced
    }

    pub fn lyrics_provider_enabled(&self, provider: &str) -> bool {
        self.values
            .lyrics_providers
            .iter()
            .any(|name| name == provider)
    }

    pub fn karaoke_lyrics(&self) -> bool {
        self.values.karaoke_lyrics
    }

    pub fn blur_lyrics(&self) -> bool {
        self.values.blur_lyrics
    }

    pub fn lyrics_blur_amount(&self) -> f32 {
        self.values.lyrics_blur_amount
    }

    pub fn fade_lyrics(&self) -> bool {
        self.values.fade_lyrics
    }

    pub fn edge_feather_lyrics(&self) -> bool {
        self.values.edge_feather_lyrics
    }

    pub fn distance_scale_lyrics(&self) -> bool {
        self.values.distance_scale_lyrics
    }

    pub fn breathing_lyrics(&self) -> bool {
        self.values.breathing_lyrics
    }

    pub fn gap_dots_lyrics(&self) -> bool {
        self.values.gap_dots_lyrics
    }

    pub fn stroke_lyrics(&self) -> bool {
        self.values.stroke_lyrics
    }

    pub fn desktop_lyrics(&self) -> bool {
        self.values.desktop_lyrics
    }

    pub fn desktop_lyrics_font_size(&self) -> f32 {
        self.values
            .desktop_lyrics_font_size
            .clamp(DESKTOP_LYRICS_FONT_MIN, DESKTOP_LYRICS_FONT_MAX)
    }

    pub fn desktop_lyrics_vertical(&self) -> bool {
        self.values.desktop_lyrics_vertical
    }

    pub fn desktop_lyrics_stroke(&self) -> bool {
        self.values.desktop_lyrics_stroke
    }

    pub fn desktop_lyrics_cover_color(&self) -> bool {
        self.values.desktop_lyrics_cover_color
    }

    pub fn desktop_lyrics_locked(&self) -> bool {
        self.values.desktop_lyrics_locked
    }

    /// The size the desktop strip was last left at in this orientation, if it has one.
    pub fn desktop_lyrics_size(&self, vertical: bool) -> Option<(f32, f32)> {
        let [width, height] = match vertical {
            true => self.state.desktop_lyrics_tall,
            false => self.state.desktop_lyrics_wide,
        }?;
        (width.is_finite() && height.is_finite() && width > 0. && height > 0.)
            .then_some((width, height))
    }

    /// Remembers the strip's size for this orientation, so turning it back brings the size up.
    pub fn set_desktop_lyrics_size(
        &mut self,
        vertical: bool,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) {
        if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
            return;
        }
        let slot = match vertical {
            true => &mut self.state.desktop_lyrics_tall,
            false => &mut self.state.desktop_lyrics_wide,
        };
        if *slot == Some([width, height]) {
            return;
        }
        *slot = Some([width, height]);
        self.schedule_state_save(cx);
    }

    pub fn desktop_lyrics_stroke_scale(&self) -> f32 {
        self.values.desktop_lyrics_stroke_scale.clamp(
            DESKTOP_LYRICS_STROKE_SCALE_MIN,
            DESKTOP_LYRICS_STROKE_SCALE_MAX,
        )
    }

    pub fn lyrics_cover_color(&self) -> bool {
        self.values.lyrics_cover_color
    }

    pub fn parallax_lyrics(&self) -> bool {
        self.values.parallax_lyrics
    }

    pub fn lyrics_motion_preset(&self) -> LyricMotionPreset {
        self.values.lyrics_motion_preset
    }

    pub fn romanized_lyrics(&self) -> bool {
        self.values.romanized_lyrics
    }

    pub fn panel_lyrics_scale(&self) -> f32 {
        self.values
            .panel_lyrics_scale
            .clamp(ui::MIN_LYRICS_SCALE, ui::MAX_LYRICS_SCALE)
    }

    pub fn fullscreen_lyrics_scale(&self) -> f32 {
        self.values
            .fullscreen_lyrics_scale
            .clamp(ui::MIN_LYRICS_SCALE, ui::MAX_LYRICS_SCALE)
    }

    pub fn romanization_scripts(&self) -> RomanizationScripts {
        self.values.romanization_scripts
    }

    pub fn adaptive_menu(&self) -> bool {
        self.values.adaptive_menu
    }

    pub fn update_channel(&self) -> Channel {
        Channel::from_id(&self.values.update_channel)
    }

    pub fn close_to_tray(&self) -> bool {
        self.values.close_to_tray
    }

    pub fn tray_icon(&self) -> bool {
        self.values.tray_icon
    }

    /// Whether music keeps the system awake and, in fullscreen, the display.
    pub fn stay_awake(&self) -> bool {
        self.values.stay_awake
    }

    /// Whether a file opened from a file manager goes straight to the now playing view.
    pub fn now_playing_on_file_open(&self) -> bool {
        self.values.now_playing_on_file_open
    }

    /// Whether the Windows taskbar button fills with the track's progress while it plays.
    pub fn taskbar_progress(&self) -> bool {
        self.values.taskbar_progress
    }

    /// Whether plays from the provider with this slug are recorded on the provider's own server.
    pub fn scrobbles(&self, provider: &str) -> bool {
        !self.values.unscrobbled.contains(provider)
    }

    pub fn sidebar_width(&self) -> f32 {
        self.state.sidebar_width
    }

    pub fn sidebar_open(&self) -> bool {
        self.state.sidebar_open
    }

    pub fn sidebar_rail(&self) -> bool {
        self.state.sidebar_rail
    }

    pub fn sidebar_right_width(&self) -> f32 {
        self.state.sidebar_right_width
    }

    pub fn sidebar_right_open(&self) -> bool {
        self.state.sidebar_right_open
    }

    /// The queue stands in for the lyrics while they are turned off.
    pub fn sidebar_right_tab(&self) -> SideTab {
        match self.state.sidebar_right_tab {
            SideTab::Lyrics if !self.lyrics_enabled() => SideTab::Queue,
            tab => tab,
        }
    }

    /// The panel the fullscreen view last showed beside the cover, or `None` when it showed the
    /// artwork alone, or while lyrics are turned off.
    pub fn fullscreen_tab(&self) -> Option<SideTab> {
        self.state
            .fullscreen_tab
            .filter(|tab| *tab != SideTab::Lyrics || self.lyrics_enabled())
    }

    pub fn shuffle(&self) -> bool {
        self.state.shuffle
    }

    pub fn repeat(&self) -> Repeat {
        self.state.repeat
    }

    pub fn radio(&self) -> bool {
        self.state.radio
    }

    pub fn auto_checkin(&self) -> bool {
        self.state.auto_checkin
    }

    pub fn last_checkin_date(&self) -> &str {
        &self.state.last_checkin_date
    }

    pub fn last_checkin_claimed(&self) -> bool {
        self.state.last_checkin_claimed
    }

    pub fn last_checkin_upgraded(&self) -> bool {
        self.state.last_checkin_upgraded
    }

    pub fn fm_mode(&self) -> &str {
        &self.state.fm_mode
    }

    pub fn fm_song_pool(&self) -> u32 {
        self.state.fm_song_pool
    }

    pub fn language(&self) -> &str {
        &self.values.language
    }

    pub fn font(&self) -> &str {
        &self.values.font
    }

    pub fn provider(&self) -> &str {
        self.values
            .provider
            .as_deref()
            .unwrap_or(&self.state.provider)
    }

    /// The provider explicitly selected for startup, or `None` to restore the last-used provider.
    pub fn startup_provider(&self) -> Option<&str> {
        self.values.provider.as_deref()
    }

    pub fn local_folders(&self) -> &[PathBuf] {
        &self.values.local_folders
    }

    pub fn startup(&self) -> &str {
        &self.values.startup
    }

    pub fn startup_animation(&self) -> bool {
        self.values.startup_animation
    }
    pub fn theme(&self) -> &str {
        &self.values.appearance.theme
    }

    /// Every theme file, bundled first and then the user's, grouped into families. The built-in
    /// System, Dark and Light themes are not in it and stay in `ThemeKind::ALL`.
    pub fn theme_families(&self) -> &[ThemeFamily] {
        &self.families
    }

    /// The family and variant of the selected theme, or `None` while a built-in theme is
    /// selected or the selected file is gone.
    pub fn theme_variant(&self) -> Option<(&ThemeFamily, &ThemeVariant)> {
        self.families.iter().find_map(|family| {
            family
                .variant(self.theme())
                .map(|variant| (family, variant))
        })
    }

    /// The variant choosing `family` would select: the one last chosen in it, else the first
    /// whose lightness matches the system appearance, else the first.
    pub fn family_variant(&self, family: &str, cx: &App) -> Option<&ThemeVariant> {
        let family = self.families.iter().find(|found| found.id == family)?;
        let light = ThemeKind::System.resolved(cx) == ThemeKind::Light;
        self.state
            .theme_variants
            .get(&family.id)
            .and_then(|id| family.variant(id))
            .or_else(|| {
                family
                    .variants
                    .iter()
                    .find(|variant| variant.is_light() == light)
            })
            .or_else(|| family.variants.first())
    }

    pub fn adaptive_theme(&self) -> bool {
        self.values.appearance.adaptive_theme
    }

    /// The arrangement of the workspace's playback controls. Unknown saved ids use Classic.
    pub fn player_bar_layout(&self) -> PlayerBarLayout {
        PlayerBarLayout::from_id(&self.values.appearance.player_bar_layout)
    }

    /// Whether the listener asked for a see-through player bar. What the bar does is
    /// `player_bar_translucent`, which also needs the blur.
    pub fn player_bar_transparent(&self) -> bool {
        self.values.appearance.player_bar_transparent
    }

    /// Whether the player bar is drawn see-through with pages scrolling under it. It takes Blur
    /// UI as well, since without the blur the bar's text would sit on the bare page.
    pub fn player_bar_translucent(&self) -> bool {
        self.player_bar_transparent() && self.blur()
    }

    /// Whether the player bar shows the time left in the track where it would show its length.
    pub fn player_bar_time_left(&self) -> bool {
        self.values.appearance.player_bar_time_left
    }

    /// Whether fullscreen paints the ambient background sampled from the cover.
    pub fn ambient(&self) -> bool {
        self.values.appearance.ambient
    }

    /// Whether the ambient background drifts. Off leaves it a still gradient, which is what
    /// the system reduce-motion preference does too.
    pub fn ambient_motion(&self) -> bool {
        self.values.appearance.ambient_motion
    }

    /// Whether the playing cover should colour the theme, given whether fullscreen is up. The
    /// ambient background is painted out of the tint, so fullscreen tints whatever the adaptive
    /// theme setting says.
    pub fn cover_tint(&self, fullscreen: bool) -> bool {
        self.adaptive_theme() || (fullscreen && self.ambient())
    }

    /// The visualizer's style, `None` when it is off. The old `visualizer` switch is still the
    /// off state, so a settings file written before the two were one setting keeps its answer.
    pub fn visualizer_style(&self) -> ui::VisualizerStyle {
        match self.values.appearance.visualizer {
            true => ui::VisualizerStyle::from_id(&self.values.appearance.visualizer_style),
            false => ui::VisualizerStyle::None,
        }
    }

    /// Whether the visualizer ignores Wisp's volume and draws the track at its own level.
    pub fn visualizer_absolute(&self) -> bool {
        self.values.appearance.visualizer_absolute
    }

    /// The views of the fullscreen player the visualizer is drawn in.
    pub fn visualizer_screens(&self) -> VisualizerScreens {
        self.values.appearance.visualizer_screens
    }

    pub fn fullscreen_controls_autohide(&self) -> FullscreenControlsAutohide {
        FullscreenControlsAutohide::from_id(&self.values.appearance.fullscreen_controls_autohide)
    }

    pub fn icons(&self) -> &str {
        &self.values.appearance.icons
    }

    pub fn rounding(&self) -> &str {
        &self.values.appearance.rounding
    }

    /// How tall table rows stand. An unknown saved id reads as regular.
    pub fn row_size(&self) -> RowSize {
        RowSize::from_id(&self.values.appearance.row_size)
    }

    /// How wide pages stretch on a wide window. An unknown saved id reads as centred.
    pub fn page_width(&self) -> PageWidth {
        PageWidth::from_id(&self.values.appearance.page_width)
    }

    /// Whether the app paints its frosted treatments. See `ui::blurring`.
    pub fn blur(&self) -> bool {
        self.values.appearance.blur
    }

    pub fn blur_window(&self) -> bool {
        self.values.appearance.blur_window
    }

    /// Whether opening the fullscreen view also puts the window into OS fullscreen.
    pub fn os_fullscreen(&self) -> bool {
        self.values.appearance.os_fullscreen
    }

    pub fn stillness(&self) -> Stillness {
        Stillness::from_id(&self.values.appearance.reduce_motion)
    }

    pub fn pace(&self) -> Pace {
        Pace::from_id(&self.values.appearance.motion_pace)
    }

    pub fn saver(&self) -> Saver {
        Saver::from_id(&self.values.appearance.battery_saver)
    }

    pub fn system_theme(&self) -> ThemeKind {
        ThemeKind::from_id(&self.state.system_theme)
    }

    pub fn look(&self) -> Look {
        Look {
            kind: ThemeKind::from_id(self.theme()),
            rounding: Rounding::from_id(self.rounding()),
            rows: self.row_size(),
            font: self.font_size(),
            transparent: self.transparent(),
            transparency: self.transparency(),
            blur: self.blur(),
            blur_window: self.blur_window(),
            tint: None,
            tint_secondary: None,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn server_side_decorations(&self) -> bool {
        self.values.appearance.server_side_decorations
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn window_decorations(&self) -> WindowDecorations {
        match self.server_side_decorations() {
            true => WindowDecorations::Server,
            false => WindowDecorations::Client,
        }
    }

    #[cfg(any(
        target_os = "windows",
        target_os = "linux",
        target_os = "freebsd",
        target_os = "macos"
    ))]
    pub fn window_rounding(&self) -> Rounding {
        Rounding::from_id(&self.values.appearance.window_rounding)
    }

    pub fn window_controls(&self) -> bool {
        self.values.appearance.window_controls
    }

    #[cfg(not(target_os = "macos"))]
    pub fn traffic_light_controls(&self) -> bool {
        self.values.appearance.traffic_light_controls
    }

    pub fn controls_on_left(&self) -> bool {
        self.values.appearance.controls_on_left
    }

    pub fn font_size(&self) -> f32 {
        self.values
            .appearance
            .font_size
            .clamp(ui::MIN_FONT, ui::MAX_FONT)
    }

    pub fn transparent(&self) -> bool {
        self.values.appearance.transparent
    }

    pub fn transparency(&self) -> f32 {
        self.values
            .appearance
            .transparency
            .clamp(0., ui::MAX_TRANSPARENCY)
    }

    /// Combines the selected theme file with legacy overrides from `settings.json`.
    pub fn theme_overrides(&self) -> ThemeOverrides {
        let inline = &self.values.appearance.theme_overrides;
        self.theme_variant()
            .map(|(_, variant)| variant.overrides.clone().merged(inline))
            .unwrap_or_else(|| inline.clone())
    }

    /// Writes the theme or family a link offered into the themes folder and selects it the way
    /// choosing its card does, turning the adaptive theme off since only the neutral themes wear
    /// it. Returns the family's id.
    pub fn install_theme(&mut self, offer: &ThemeOffer, cx: &mut Context<Self>) -> Result<String> {
        let directory = themes_path(&self.path);
        let id = themes::install(offer, &directory, &self.themes)?;
        let loaded = themes::load(&directory, &self.themes);
        let count = loaded
            .themes
            .iter()
            .filter(|theme| theme.family == id)
            .count();
        anyhow::ensure!(
            count >= offer.family.variants.len(),
            "cannot load the installed theme {id}"
        );
        self.families = themes::catalog(&loaded.themes);
        self.themes = loaded.themes;
        if self.adaptive_theme() {
            self.set_adaptive_theme(false, cx);
        }
        self.set_theme_family(&id, cx);
        cx.notify();
        Ok(id)
    }

    /// Deletes the user's theme family `id` from the themes folder and says whether it worked.
    /// A selected theme in it falls back to Dark. A failure is logged and shown as a toast.
    pub fn delete_theme_family(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(family) = self.families.iter().find(|family| family.id == id).cloned() else {
            return false;
        };
        let directory = themes_path(&self.path);
        if let Err(error) = themes::remove(&family, &directory) {
            log::warn!("settings: cannot delete theme {id}: {error:#}");
            Toasts::about(
                Outcome::Failed,
                "toast-theme-delete-failed",
                family.name,
                cx,
            );
            return false;
        }
        log::info!("settings: deleted theme {id}");
        let loaded = themes::load(&directory, &self.themes);
        self.families = themes::catalog(&loaded.themes);
        self.themes = loaded.themes;
        if family.variant(self.theme()).is_some() {
            self.set_theme(ThemeKind::Dark.id(), cx);
        }
        if self.state.theme_variants.remove(id).is_some() {
            self.schedule_state_save(cx);
        }
        cx.notify();
        true
    }

    /// Creates and returns the folder that holds custom themes.
    pub fn ensure_themes_directory(&self) -> PathBuf {
        let path = themes_path(&self.path);
        if let Err(error) = fs::create_dir_all(&path) {
            log::warn!("settings: cannot create {}: {error}", path.display());
        }
        path
    }

    pub fn set_local_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.values.local_folders == folders {
            return;
        }
        self.values.local_folders = folders;
        self.schedule_save(cx);
    }

    /// Persists playback's volume without invalidating views that observe preferences.
    /// Live volume changes are published by `Playback`, not by settings.
    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.state.volume = volume.clamp(0., 1.);
        self.save_state_quietly(cx);
    }

    pub fn set_normalisation(&mut self, normalisation: bool, cx: &mut Context<Self>) {
        self.values.normalisation = normalisation;
        self.schedule_save(cx);
    }

    pub fn set_gapless(&mut self, gapless: bool, cx: &mut Context<Self>) {
        self.values.gapless = gapless;
        self.schedule_save(cx);
    }

    pub fn set_preamp_db(&mut self, db: f32, cx: &mut Context<Self>) {
        let db = db.clamp(PREAMP_MIN, PREAMP_MAX);
        if self.values.preamp_db == db {
            return;
        }
        self.values.preamp_db = db;
        self.schedule_save(cx);
    }

    pub fn set_unblock_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.values.unblock_enabled == enabled {
            return;
        }
        self.values.unblock_enabled = enabled;
        self.schedule_save(cx);
    }

    pub fn set_unblock_flac(&mut self, flac: bool, cx: &mut Context<Self>) {
        self.values.unblock_flac = flac;
        self.schedule_save(cx);
    }

    pub fn set_unblock_order_first(&mut self, order_first: bool, cx: &mut Context<Self>) {
        self.values.unblock_order_first = order_first;
        self.schedule_save(cx);
    }

    pub fn set_unblock_budget_ms(&mut self, budget_ms: u32, cx: &mut Context<Self>) {
        let budget_ms = budget_ms.clamp(UNBLOCK_BUDGET_MIN_MS, UNBLOCK_BUDGET_MAX_MS);
        if self.values.unblock_budget_ms == budget_ms {
            return;
        }
        self.values.unblock_budget_ms = budget_ms;
        self.schedule_save(cx);
    }

    pub fn set_unblock_source(&mut self, source: impl Into<String>, cx: &mut Context<Self>) {
        self.values.unblock_source = source.into();
        self.schedule_save(cx);
    }

    pub fn set_unblock_qq_cookie(&mut self, cookie: impl Into<String>, cx: &mut Context<Self>) {
        self.values.unblock_qq_cookie = cookie.into();
        self.schedule_save(cx);
    }

    pub fn set_unblock_joox_cookie(&mut self, cookie: impl Into<String>, cx: &mut Context<Self>) {
        self.values.unblock_joox_cookie = cookie.into();
        self.schedule_save(cx);
    }

    pub fn set_keep_tracks(&mut self, keep_tracks: bool, cx: &mut Context<Self>) {
        self.values.keep_tracks = keep_tracks;
        self.schedule_save(cx);
    }

    pub fn set_cache_gb(&mut self, cache_gb: u32, cx: &mut Context<Self>) {
        self.values.cache_gb = cache_gb;
        self.schedule_save(cx);
    }

    pub fn set_fullscreen_controls_autohide(
        &mut self,
        fca: FullscreenControlsAutohide,
        cx: &mut Context<Self>,
    ) {
        self.values.appearance.fullscreen_controls_autohide = fca.id().to_owned();
        self.schedule_save(cx);
    }

    pub fn set_lyrics_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.values.lyrics_enabled = enabled;
        self.schedule_save(cx);
    }

    pub fn set_lyrics_for_local_files(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.values.lyrics_for_local_files = enabled;
        self.schedule_save(cx);
    }

    pub fn set_output_device(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.values.output_device = id;
        self.schedule_save(cx);
    }

    pub fn set_output_rate(&mut self, rate: Option<u32>, cx: &mut Context<Self>) {
        self.values.output_rate = rate;
        self.schedule_save(cx);
    }

    pub fn set_output_buffer(&mut self, frames: Option<u32>, cx: &mut Context<Self>) {
        self.values.output_buffer = frames;
        self.schedule_save(cx);
    }

    pub fn set_output_match_rate(&mut self, on: bool, cx: &mut Context<Self>) {
        self.values.output_match_rate = on;
        self.schedule_save(cx);
    }

    pub fn set_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        self.values.stream_quality = quality.id().to_owned();
        self.schedule_save(cx);
    }
    pub fn set_playback_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        self.values.playback_speed = speed.clamp(0.5, 2.0);
        self.schedule_save(cx);
    }

    pub fn set_lyrics_provider(&mut self, provider: &str, enabled: bool, cx: &mut Context<Self>) {
        self.values.lyrics_providers.retain(|name| name != provider);
        if enabled {
            self.values.lyrics_providers.push(provider.to_owned());
        }
        self.values.lyrics_providers.sort();
        self.schedule_save(cx);
    }

    pub fn set_lyrics_word_synced_first(&mut self, first: bool, cx: &mut Context<Self>) {
        self.values.lyrics_prefer_word_synced = first;
        self.schedule_save(cx);
    }

    pub fn set_karaoke_lyrics(&mut self, karaoke: bool, cx: &mut Context<Self>) {
        self.values.karaoke_lyrics = karaoke;
        self.schedule_save(cx);
    }

    pub fn set_desktop_lyrics(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.values.desktop_lyrics == on {
            return;
        }
        self.values.desktop_lyrics = on;
        self.schedule_save(cx);
    }

    pub fn set_desktop_lyrics_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(DESKTOP_LYRICS_FONT_MIN, DESKTOP_LYRICS_FONT_MAX);
        if (self.values.desktop_lyrics_font_size - size).abs() < f32::EPSILON {
            return;
        }
        self.values.desktop_lyrics_font_size = size;
        self.schedule_save(cx);
    }

    pub fn set_desktop_lyrics_vertical(&mut self, vertical: bool, cx: &mut Context<Self>) {
        if self.values.desktop_lyrics_vertical == vertical {
            return;
        }
        self.values.desktop_lyrics_vertical = vertical;
        self.schedule_save(cx);
    }

    pub fn set_desktop_lyrics_stroke(&mut self, stroke: bool, cx: &mut Context<Self>) {
        if self.values.desktop_lyrics_stroke == stroke {
            return;
        }
        self.values.desktop_lyrics_stroke = stroke;
        self.schedule_save(cx);
    }

    pub fn set_desktop_lyrics_cover_color(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.values.desktop_lyrics_cover_color == on {
            return;
        }
        self.values.desktop_lyrics_cover_color = on;
        self.schedule_save(cx);
    }

    pub fn set_desktop_lyrics_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.values.desktop_lyrics_locked == locked {
            return;
        }
        self.values.desktop_lyrics_locked = locked;
        self.schedule_save(cx);
    }

    pub fn set_desktop_lyrics_stroke_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        let scale = (scale.clamp(
            DESKTOP_LYRICS_STROKE_SCALE_MIN,
            DESKTOP_LYRICS_STROKE_SCALE_MAX,
        ) * 100.)
            .round()
            / 100.;
        if (self.values.desktop_lyrics_stroke_scale - scale).abs() < f32::EPSILON {
            return;
        }
        self.values.desktop_lyrics_stroke_scale = scale;
        self.schedule_save(cx);
    }

    pub fn set_lyrics_cover_color(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.values.lyrics_cover_color == on {
            return;
        }
        self.values.lyrics_cover_color = on;
        self.schedule_save(cx);
    }

    pub fn set_blur_lyrics(&mut self, blur: bool, cx: &mut Context<Self>) {
        self.values.blur_lyrics = blur;
        self.schedule_save(cx);
    }

    pub fn set_lyrics_blur_amount(&mut self, amount: f32, cx: &mut Context<Self>) {
        self.values.lyrics_blur_amount = amount.clamp(0., 2.);
        self.schedule_save(cx);
    }

    pub fn set_fade_lyrics(&mut self, fade: bool, cx: &mut Context<Self>) {
        self.values.fade_lyrics = fade;
        self.schedule_save(cx);
    }

    pub fn set_edge_feather_lyrics(&mut self, edge: bool, cx: &mut Context<Self>) {
        self.values.edge_feather_lyrics = edge;
        self.schedule_save(cx);
    }

    pub fn set_distance_scale_lyrics(&mut self, scale: bool, cx: &mut Context<Self>) {
        self.values.distance_scale_lyrics = scale;
        self.schedule_save(cx);
    }

    pub fn set_breathing_lyrics(&mut self, breathing: bool, cx: &mut Context<Self>) {
        self.values.breathing_lyrics = breathing;
        self.schedule_save(cx);
    }

    pub fn set_gap_dots_lyrics(&mut self, dots: bool, cx: &mut Context<Self>) {
        self.values.gap_dots_lyrics = dots;
        self.schedule_save(cx);
    }

    pub fn set_stroke_lyrics(&mut self, stroke: bool, cx: &mut Context<Self>) {
        self.values.stroke_lyrics = stroke;
        self.schedule_save(cx);
    }

    pub fn set_parallax_lyrics(&mut self, parallax: bool, cx: &mut Context<Self>) {
        self.values.parallax_lyrics = parallax;
        self.schedule_save(cx);
    }

    pub fn set_lyrics_motion_preset(&mut self, preset: LyricMotionPreset, cx: &mut Context<Self>) {
        self.values.lyrics_motion_preset = preset;
        self.schedule_save(cx);
    }

    pub fn set_romanized_lyrics(&mut self, romanized: bool, cx: &mut Context<Self>) {
        self.values.romanized_lyrics = romanized;
        self.schedule_save(cx);
    }

    pub fn set_panel_lyrics_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        self.values.panel_lyrics_scale = scale.clamp(ui::MIN_LYRICS_SCALE, ui::MAX_LYRICS_SCALE);
        self.schedule_save(cx);
    }

    pub fn set_fullscreen_lyrics_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        self.values.fullscreen_lyrics_scale =
            scale.clamp(ui::MIN_LYRICS_SCALE, ui::MAX_LYRICS_SCALE);
        self.schedule_save(cx);
    }

    pub fn set_romanization_script(
        &mut self,
        writing_system: WritingSystem,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.values
            .romanization_scripts
            .set(writing_system, enabled);
        self.schedule_save(cx);
    }

    pub fn set_adaptive_menu(&mut self, adaptive_menu: bool, cx: &mut Context<Self>) {
        self.values.adaptive_menu = adaptive_menu;
        self.schedule_save(cx);
    }

    /// The keys the user chose for shortcuts, by shortcut id.
    pub fn keybinds(&self) -> &BTreeMap<String, Vec<String>> {
        &self.values.keybinds
    }

    /// Replaces the keys the user chose for shortcuts. The app binds them once observers hear
    /// of the change.
    pub fn set_keybinds(
        &mut self,
        keybinds: BTreeMap<String, Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        self.values.keybinds = keybinds;
        self.schedule_save(cx);
    }

    pub fn set_update_channel(&mut self, channel: Channel, cx: &mut Context<Self>) {
        self.values.update_channel = channel.id().to_owned();
        self.schedule_save(cx);
    }

    pub fn set_close_to_tray(&mut self, close_to_tray: bool, cx: &mut Context<Self>) {
        self.values.close_to_tray = close_to_tray;
        self.schedule_save(cx);
    }

    pub fn set_tray_icon(&mut self, tray_icon: bool, cx: &mut Context<Self>) {
        self.values.tray_icon = tray_icon;
        self.schedule_save(cx);
    }

    pub fn set_stay_awake(&mut self, stay_awake: bool, cx: &mut Context<Self>) {
        self.values.stay_awake = stay_awake;
        self.schedule_save(cx);
    }

    pub fn set_now_playing_on_file_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.values.now_playing_on_file_open = open;
        self.schedule_save(cx);
    }

    pub fn set_taskbar_progress(&mut self, taskbar_progress: bool, cx: &mut Context<Self>) {
        self.values.taskbar_progress = taskbar_progress;
        self.schedule_save(cx);
    }

    /// Whether plays from this provider are recorded on its own server.
    pub fn set_scrobbles(&mut self, provider: &str, enabled: bool, cx: &mut Context<Self>) {
        match enabled {
            true => self.values.unscrobbled.remove(provider),
            false => self.values.unscrobbled.insert(provider.to_owned()),
        };
        self.schedule_save(cx);
    }

    pub fn table(&self, table: &str) -> Layout {
        self.state.tables.get(table).cloned().unwrap_or_default()
    }

    pub fn set_table(&mut self, table: &str, layout: Layout, cx: &mut Context<Self>) {
        if self.state.tables.get(table) == Some(&layout) {
            return;
        }
        self.state.tables.insert(table.to_owned(), layout);
        self.schedule_state_save(cx);
    }

    pub fn view_or(&self, table: &str, fallback: Mode) -> Mode {
        self.state.views.get(table).copied().unwrap_or(fallback)
    }

    pub fn set_view(&mut self, table: &str, mode: Mode, cx: &mut Context<Self>) {
        if self.state.views.get(table) == Some(&mode) {
            return;
        }
        self.state.views.insert(table.to_owned(), mode);
        self.schedule_state_save(cx);
    }

    pub fn sorting(&self, table: &str) -> Option<Option<Sorting>> {
        self.state.sorting.get(table).cloned()
    }

    pub fn set_sorting(&mut self, table: &str, sorting: Option<Sorting>, cx: &mut Context<Self>) {
        if self.state.sorting.get(table) == Some(&sorting) {
            return;
        }
        self.state.sorting.insert(table.to_owned(), sorting);
        self.schedule_state_save(cx);
    }

    /// The narrowed filter axes stored under a table key, if any.
    pub fn filters(&self, table: &str) -> Option<HashMap<String, FilterValue>> {
        self.state.filters.get(table).cloned()
    }

    /// Stores the narrowed filter axes of a table. An empty map drops the entry, so resetting
    /// a table clears its stored filters on the next store.
    pub fn set_filters(
        &mut self,
        table: &str,
        filters: HashMap<String, FilterValue>,
        cx: &mut Context<Self>,
    ) {
        if filters.is_empty() {
            if self.state.filters.remove(table).is_none() {
                return;
            }
        } else {
            if self.state.filters.get(table) == Some(&filters) {
                return;
            }
            self.state.filters.insert(table.to_owned(), filters);
        }
        self.schedule_state_save(cx);
    }

    pub fn pinned(&self, slugs: &[&str]) -> Vec<Pin> {
        gather(&self.state.pinned, slugs)
    }

    pub fn resume(&self) -> Option<&Resume> {
        self.state.resume.as_ref()
    }

    /// Records where playback picks up on the next launch. See `carry` for what survives.
    pub fn set_resume(&mut self, resume: Option<Resume>, cx: &mut Context<Self>) {
        let mut resume = resume;
        if let Some(next) = resume.as_mut() {
            carry(self.state.resume.as_ref(), next);
        }
        if self.state.resume == resume {
            return;
        }
        self.state.resume = resume;
        self.schedule_state_save(cx);
    }

    /// Saves quietly, since playback calls this on every position tick.
    pub fn set_resume_position(&mut self, position: f32, cx: &mut Context<Self>) {
        let Some(resume) = self.state.resume.as_mut() else {
            return;
        };
        if resume.position == position {
            return;
        }
        resume.position = position;
        self.save_state_quietly(cx);
    }

    /// Pins at `gap`, a slot counted among the pins of the providers in `slugs`. An existing pin
    /// moves instead of duplicating, and nothing is saved when it already sits there.
    pub fn pin(
        &mut self,
        slug: &str,
        pin: Pin,
        gap: Option<usize>,
        slugs: &[&str],
        cx: &mut Context<Self>,
    ) {
        if !place(&mut self.state.pinned, slug, pin, gap, slugs) {
            return;
        }
        self.schedule_state_save(cx);
    }

    pub fn unpin(&mut self, slug: &str, pin: &Pin, cx: &mut Context<Self>) {
        if !take(&mut self.state.pinned, slug, pin) {
            return;
        }
        self.schedule_state_save(cx);
    }

    pub fn set_sidebar(&mut self, width: f32, open: bool, rail: bool, cx: &mut Context<Self>) {
        self.state.sidebar_width = width;
        self.state.sidebar_open = open;
        self.state.sidebar_rail = rail;
        self.schedule_state_save(cx);
    }

    pub fn set_sidebar_right_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.state.sidebar_right_width = width;
        self.schedule_state_save(cx);
    }

    pub fn set_sidebar_right_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.state.sidebar_right_open = open;
        self.schedule_state_save(cx);
    }

    pub fn set_sidebar_right_tab(&mut self, tab: SideTab, cx: &mut Context<Self>) {
        if self.state.sidebar_right_tab == tab {
            return;
        }
        self.state.sidebar_right_tab = tab;
        self.schedule_state_save(cx);
    }

    pub fn set_fullscreen_tab(&mut self, tab: Option<SideTab>, cx: &mut Context<Self>) {
        if self.state.fullscreen_tab == tab {
            return;
        }
        self.state.fullscreen_tab = tab;
        self.schedule_state_save(cx);
    }

    pub fn set_shuffle(&mut self, shuffle: bool, cx: &mut Context<Self>) {
        self.state.shuffle = shuffle;
        self.schedule_state_save(cx);
    }

    pub fn set_repeat(&mut self, repeat: Repeat, cx: &mut Context<Self>) {
        self.state.repeat = repeat;
        self.schedule_state_save(cx);
    }

    pub fn set_radio(&mut self, radio: bool, cx: &mut Context<Self>) {
        self.state.radio = radio;
        self.schedule_state_save(cx);
    }

    pub fn set_auto_checkin(&mut self, auto: bool, cx: &mut Context<Self>) {
        if self.state.auto_checkin == auto {
            return;
        }
        self.state.auto_checkin = auto;
        self.schedule_state_save(cx);
    }

    pub fn set_last_checkin(
        &mut self,
        date: &str,
        claimed: bool,
        upgraded: bool,
        cx: &mut Context<Self>,
    ) {
        self.state.last_checkin_date = date.to_owned();
        self.state.last_checkin_claimed = claimed;
        self.state.last_checkin_upgraded = upgraded;
        self.schedule_state_save(cx);
    }

    pub fn set_fm_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        let mode = match mode {
            "small" | "peak" | "radio" => mode,
            _ => "normal",
        };
        if self.state.fm_mode == mode {
            return;
        }
        self.state.fm_mode = mode.to_owned();
        self.schedule_state_save(cx);
    }

    pub fn set_fm_song_pool(&mut self, pool: u32, cx: &mut Context<Self>) {
        let pool = pool.min(2);
        if self.state.fm_song_pool == pool {
            return;
        }
        self.state.fm_song_pool = pool;
        self.schedule_state_save(cx);
    }

    pub fn set_language(&mut self, language: impl Into<String>, cx: &mut Context<Self>) {
        self.values.language = language.into();
        i18n::set(i18n::resolve(&self.values.language));
        cx.refresh_windows();
        self.schedule_save(cx);
    }

    pub fn set_font(&mut self, font: impl Into<String>, cx: &mut Context<Self>) {
        let font = font.into();
        if self.values.font == font {
            return;
        }
        self.values.font = font;
        cx.refresh_windows();
        self.schedule_save(cx);
    }

    /// Whether the sidebar's pinned section is expanded. Collapsed on a first run.
    pub fn sidebar_pinned_open(&self) -> bool {
        self.state.sidebar_pinned_open
    }

    pub fn set_sidebar_pinned_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.state.sidebar_pinned_open = open;
        self.schedule_state_save(cx);
    }

    /// Whether the sidebar lists the rest of the library under the pins.
    pub fn sidebar_full_library(&self) -> bool {
        self.state.sidebar_full_library
    }

    pub fn set_sidebar_full_library(&mut self, full: bool, cx: &mut Context<Self>) {
        self.state.sidebar_full_library = full;
        self.schedule_state_save(cx);
    }

    pub fn sidebar_pin_sort(&self) -> Option<PinSort> {
        PinSort::from_id(&self.state.sidebar_pin_sort)
    }

    pub fn sidebar_pin_reversed(&self) -> bool {
        self.state.sidebar_pin_reversed
    }

    pub fn set_sidebar_pin_sort(
        &mut self,
        sort: Option<PinSort>,
        reversed: bool,
        cx: &mut Context<Self>,
    ) {
        self.state.sidebar_pin_sort = sort.map(PinSort::id).unwrap_or_default().to_owned();
        self.state.sidebar_pin_reversed = reversed;
        self.schedule_state_save(cx);
    }

    /// The search results chosen on the providers in `slugs`, newest first.
    pub fn picks(&self, slugs: &[&str]) -> Vec<Hit> {
        self.state
            .picks
            .iter()
            .filter(|picked| slugs.contains(&picked.slug.as_str()))
            .map(|picked| picked.hit.clone())
            .collect()
    }

    /// Puts `hit` at the top of the search history, dropping an earlier copy and the oldest
    /// entry past the cap. The save skips settings observers, so the caller repaints what shows
    /// the history.
    pub fn remember_pick(&mut self, slug: &str, hit: Hit, cx: &mut Context<Self>) {
        let key = hit.key();
        let picks = &mut self.state.picks;
        picks.retain(|picked| picked.hit.key() != key);
        picks.insert(
            0,
            Picked {
                slug: slug.to_owned(),
                hit,
            },
        );
        picks.truncate(PICKS);
        self.save_state_quietly(cx);
    }

    /// Drops the hit with `key` from the search history. Like `remember_pick`, it wakes no
    /// observers.
    pub fn forget_pick(&mut self, key: &str, cx: &mut Context<Self>) {
        let kept = self.state.picks.len();
        self.state.picks.retain(|picked| picked.hit.key() != key);
        if self.state.picks.len() != kept {
            self.save_state_quietly(cx);
        }
    }

    /// Rewrites the pins of `slugs` into `order`, leaving another provider's pins in their slots.
    /// Nothing happens unless `order` holds exactly the pins already there.
    pub fn rearrange(&mut self, order: &[Pin], slugs: &[&str], cx: &mut Context<Self>) {
        let slots: Vec<usize> = shown(&self.state.pinned, slugs)
            .map(|(index, _)| index)
            .collect();
        let moved: Vec<Held> = order
            .iter()
            .filter_map(|pin| {
                self.state
                    .pinned
                    .iter()
                    .find(|held| held.pin.same(pin))
                    .cloned()
            })
            .collect();
        if moved.len() != slots.len() {
            return;
        }
        for (slot, held) in slots.into_iter().zip(moved) {
            self.state.pinned[slot] = held;
        }
        self.schedule_state_save(cx);
    }

    pub fn nav_shown(&self, entry: &str) -> bool {
        !self.values.hidden_nav.iter().any(|hidden| hidden == entry)
    }

    pub fn set_nav_shown(&mut self, entry: &str, shown: bool, cx: &mut Context<Self>) {
        if self.nav_shown(entry) == shown {
            return;
        }
        match shown {
            true => self.values.hidden_nav.retain(|hidden| hidden != entry),
            false => self.values.hidden_nav.push(entry.to_owned()),
        }
        self.schedule_save(cx);
    }

    pub fn requirement_silenced(&self, slug: &str) -> bool {
        self.values
            .silenced_requirements
            .iter()
            .any(|silenced| silenced == slug)
    }

    pub fn silence_requirement(&mut self, slug: &str, cx: &mut Context<Self>) {
        if self.requirement_silenced(slug) {
            return;
        }
        self.values.silenced_requirements.push(slug.to_owned());
        self.schedule_save(cx);
    }

    pub fn set_startup(&mut self, screen: impl Into<String>, cx: &mut Context<Self>) {
        let screen = screen.into();
        if self.values.startup == screen {
            return;
        }
        self.values.startup = screen;
        self.schedule_save(cx);
    }

    pub fn set_startup_animation(&mut self, on: bool, cx: &mut Context<Self>) {
        self.values.startup_animation = on;
        self.schedule_save(cx);
    }

    pub fn set_provider(&mut self, provider: impl Into<String>, cx: &mut Context<Self>) {
        let provider = provider.into();
        if self.state.provider == provider {
            return;
        }
        self.state.provider = provider;
        self.schedule_state_save(cx);
    }

    /// Selects a provider for startup, or clears the preference to restore the last-used provider.
    pub fn set_startup_provider(&mut self, provider: Option<&str>, cx: &mut Context<Self>) {
        let provider = provider.map(str::to_owned);
        if self.values.provider == provider {
            return;
        }
        self.values.provider = provider;
        self.schedule_save(cx);
    }

    /// Selects a built-in theme by its `ThemeKind` id or a theme file by its variant id, and
    /// remembers a variant as the one its family opens on next time.
    pub fn set_theme(&mut self, theme: impl Into<String>, cx: &mut Context<Self>) {
        self.values.appearance.theme = theme.into();
        if let Some(family) = self.theme_variant().map(|(family, _)| family.id.clone())
            && self.state.theme_variants.get(&family) != Some(&self.values.appearance.theme)
        {
            let variant = self.values.appearance.theme.clone();
            self.state.theme_variants.insert(family, variant);
            self.schedule_state_save(cx);
        }
        self.schedule_save(cx);
    }

    /// Selects the variant `family_variant` picks for `family`. Returns false and changes
    /// nothing when there is no such family.
    pub fn set_theme_family(&mut self, family: &str, cx: &mut Context<Self>) -> bool {
        let Some(variant) = self
            .family_variant(family, cx)
            .map(|variant| variant.id.clone())
        else {
            return false;
        };
        self.set_theme(variant, cx);
        true
    }

    pub fn set_adaptive_theme(&mut self, adaptive: bool, cx: &mut Context<Self>) {
        self.values.appearance.adaptive_theme = adaptive;
        self.schedule_save(cx);
    }

    pub fn set_ambient(&mut self, ambient: bool, cx: &mut Context<Self>) {
        self.values.appearance.ambient = ambient;
        self.schedule_save(cx);
    }

    /// Applies a player bar layout immediately and saves it in the appearance preferences.
    pub fn set_player_bar_layout(&mut self, layout: PlayerBarLayout, cx: &mut Context<Self>) {
        self.values.appearance.player_bar_layout = layout.id().to_owned();
        self.schedule_save(cx);
    }

    /// Lets the page scroll under the player bar, or restores the solid bar below the page.
    pub fn set_player_bar_transparent(&mut self, transparent: bool, cx: &mut Context<Self>) {
        self.values.appearance.player_bar_transparent = transparent;
        self.schedule_save(cx);
    }

    pub fn set_player_bar_time_left(&mut self, time_left: bool, cx: &mut Context<Self>) {
        self.values.appearance.player_bar_time_left = time_left;
        self.schedule_save(cx);
    }

    pub fn set_ambient_motion(&mut self, motion: bool, cx: &mut Context<Self>) {
        self.values.appearance.ambient_motion = motion;
        self.schedule_save(cx);
    }

    /// Picking a style turns the visualizer on; picking `None` turns it off and leaves the style
    /// behind it alone, so the old choice comes back with it.
    pub fn set_visualizer_style(&mut self, style: ui::VisualizerStyle, cx: &mut Context<Self>) {
        self.values.appearance.visualizer = style.shown();
        if style.shown() {
            self.values.appearance.visualizer_style = style.id().to_owned();
        }
        self.schedule_save(cx);
    }

    pub fn set_visualizer_absolute(&mut self, absolute: bool, cx: &mut Context<Self>) {
        self.values.appearance.visualizer_absolute = absolute;
        self.schedule_save(cx);
    }

    /// Draws the visualizer, or stops drawing it, while the fullscreen player shows `panel`,
    /// which is the cover alone when there is none.
    pub fn set_visualizer_screen(
        &mut self,
        panel: Option<SideTab>,
        shown: bool,
        cx: &mut Context<Self>,
    ) {
        self.values.appearance.visualizer_screens.set(panel, shown);
        self.schedule_save(cx);
    }

    pub fn set_icons(&mut self, pack: impl Into<String>, cx: &mut Context<Self>) {
        let pack = pack.into();
        if self.values.appearance.icons == pack {
            return;
        }
        icons::set(&pack);
        self.values.appearance.icons = pack;
        cx.refresh_windows();
        self.schedule_save(cx);
    }

    pub fn set_rounding(&mut self, rounding: impl Into<String>, cx: &mut Context<Self>) {
        self.values.appearance.rounding = rounding.into();
        self.schedule_save(cx);
    }

    pub fn set_row_size(&mut self, rows: RowSize, cx: &mut Context<Self>) {
        self.values.appearance.row_size = rows.id().to_owned();
        self.schedule_save(cx);
    }

    /// Saves the page width and redraws every window, since each page reads it while rendering.
    pub fn set_page_width(&mut self, width: PageWidth, cx: &mut Context<Self>) {
        if self.page_width() == width {
            return;
        }
        self.values.appearance.page_width = width.id().to_owned();
        cx.refresh_windows();
        self.schedule_save(cx);
    }

    pub fn set_blur(&mut self, blur: bool, cx: &mut Context<Self>) {
        self.values.appearance.blur = blur;
        self.schedule_save(cx);
    }

    pub fn set_blur_window(&mut self, blur: bool, cx: &mut Context<Self>) {
        self.values.appearance.blur_window = blur;
        self.schedule_save(cx);
    }

    pub fn set_os_fullscreen(&mut self, value: bool, cx: &mut Context<Self>) {
        self.values.appearance.os_fullscreen = value;
        self.schedule_save(cx);
    }

    pub fn set_stillness(&mut self, stillness: Stillness, cx: &mut Context<Self>) {
        if self.stillness() == stillness {
            return;
        }
        self.values.appearance.reduce_motion = stillness.id().to_owned();
        ui::motion::apply(stillness, self.pace(), cx);
        self.schedule_save(cx);
    }

    pub fn set_pace(&mut self, pace: Pace, cx: &mut Context<Self>) {
        if self.pace() == pace {
            return;
        }
        self.values.appearance.motion_pace = pace.id().to_owned();
        ui::motion::apply(self.stillness(), pace, cx);
        self.schedule_save(cx);
    }

    pub fn set_system_theme(&mut self, kind: ThemeKind, cx: &mut Context<Self>) {
        if self.system_theme() == kind {
            return;
        }
        self.state.system_theme = kind.id().to_owned();
        self.schedule_state_save(cx);
    }

    pub fn set_saver(&mut self, saver: Saver, cx: &mut Context<Self>) {
        if self.saver() == saver {
            return;
        }
        self.values.appearance.battery_saver = saver.id().to_owned();
        self.schedule_save(cx);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn set_server_side_decorations(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.values.appearance.server_side_decorations = shown;
        self.schedule_save(cx);
    }

    #[cfg(any(
        target_os = "windows",
        target_os = "linux",
        target_os = "freebsd",
        target_os = "macos"
    ))]
    pub fn set_window_rounding(&mut self, rounding: Rounding, cx: &mut Context<Self>) {
        self.values.appearance.window_rounding = rounding.id().to_owned();
        self.schedule_save(cx);
    }

    pub fn set_window_controls(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.values.appearance.window_controls = shown;
        self.schedule_save(cx);
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_traffic_light_controls(&mut self, traffic_light: bool, cx: &mut Context<Self>) {
        self.values.appearance.traffic_light_controls = traffic_light;
        self.schedule_save(cx);
    }

    pub fn set_controls_on_left(&mut self, left: bool, cx: &mut Context<Self>) {
        self.values.appearance.controls_on_left = left;
        self.schedule_save(cx);
    }

    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.values.appearance.font_size = size.clamp(ui::MIN_FONT, ui::MAX_FONT);
        self.schedule_save(cx);
    }

    pub fn reset_font_size(&mut self, cx: &mut Context<Self>) {
        self.set_font_size(DEFAULT_FONT_SIZE, cx);
    }

    pub fn set_transparent(&mut self, transparent: bool, cx: &mut Context<Self>) {
        self.values.appearance.transparent = transparent;
        self.schedule_save(cx);
    }

    pub fn set_transparency(&mut self, transparency: f32, cx: &mut Context<Self>) {
        self.values.appearance.transparency = transparency.clamp(0., ui::MAX_TRANSPARENCY);
        self.schedule_save(cx);
    }

    /// Saves the window frame now and on every move or resize.
    pub fn watch_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_frame(window, cx);
        self.watch = Some(cx.observe_window_bounds(window, |this, window, cx| {
            this.keep_frame(window, cx);
        }));
    }

    fn keep_frame(&mut self, window: &Window, cx: &mut Context<Self>) {
        let frame = Frame::of(window);
        if !frame.sane() || self.state.window == Some(frame) {
            return;
        }
        self.state.window = Some(frame);
        self.schedule_state_save(cx);
    }

    /// Wakes observers and debounces a `settings.json` write.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.save_quietly(cx);
    }

    /// Debounces a `settings.json` write. Replacing the task restarts the delay.
    fn save_quietly(&mut self, cx: &mut Context<Self>) {
        self.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            for attempt in 0..=SAVE_RETRIES {
                let Ok(result) = this.update(cx, |this, cx| this.save_now(cx)) else {
                    break;
                };
                let SettingsSave::Retry(error) = result else {
                    break;
                };
                if attempt == SAVE_RETRIES {
                    this.update(cx, |this, cx| this.give_up(error, cx)).ok();
                    break;
                }
                cx.background_executor().timer(SAVE_RETRY_DELAY).await;
            }
        }));
    }

    /// Wakes observers and debounces a `state.sqlite` write.
    fn schedule_state_save(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.save_state_quietly(cx);
    }

    /// Debounces a `state.sqlite` write without waking settings observers.
    fn save_state_quietly(&mut self, cx: &mut Context<Self>) {
        self.save_state = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update(cx, |this, _| this.save_state_now()).ok();
        }));
    }

    fn save_state_now(&self) {
        if let Err(error) = self.store.save(&self.state) {
            log::error!("settings: cannot save app state: {error:#}");
        }
    }

    /// Tells the user that `settings.json` could not be written, once per run of failed saves.
    fn give_up(&mut self, error: String, cx: &mut App) {
        if std::mem::replace(&mut self.failing, true) {
            return;
        }
        Toasts::with_detail(
            Outcome::Failed,
            "toast-settings-save-failed",
            None,
            error,
            cx,
        );
    }

    /// Writes `settings.json` now, or logs why it cannot. A save skipped because another
    /// program changed the file warns the user when the reload that follows drops their change.
    fn save_now(&mut self, cx: &mut App) -> SettingsSave {
        if !self.writable {
            return SettingsSave::Complete;
        }
        let Some(parent) = self.path.parent() else {
            return SettingsSave::Complete;
        };
        if let Err(error) = fs::create_dir_all(parent) {
            log::error!("settings: cannot create {}: {error}", parent.display());
            return SettingsSave::Retry(error.to_string());
        }

        let bytes = match serde_json::to_vec_pretty(&self.values) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::error!("settings: cannot serialize values: {error}");
                return SettingsSave::Complete;
            }
        };
        let current = match fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                log::warn!("settings: cannot read {}: {error}", self.path.display());
                return SettingsSave::Retry(error.to_string());
            }
        };
        if current != self.disk {
            log::warn!(
                "settings: not saving {}, file changed on disk",
                self.path.display()
            );
            if current
                .as_deref()
                .is_some_and(|current| overrides(current, &bytes))
            {
                Toasts::show(Outcome::Warning, "toast-settings-overwritten", cx);
            }
            return SettingsSave::Complete;
        }
        if let Err(error) = save_settings(&self.path, &bytes, current.as_deref()) {
            log::error!("settings: cannot write {}: {error:#}", self.path.display());
            return SettingsSave::Retry(format!("{error:#}"));
        }
        self.disk = Some(bytes);
        self.failing = false;
        SettingsSave::Complete
    }

    /// Watches the config and themes folders for changes made by another program.
    pub fn watch_files(&mut self, cx: &mut Context<Self>) {
        let (Some(folder), Some(settings_name)) = (self.path.parent(), self.path.file_name())
        else {
            return;
        };
        if let Err(error) = fs::create_dir_all(folder) {
            log::warn!("settings: cannot create {}: {error}", folder.display());
            return;
        }
        let themes = themes_path(&self.path);
        if let Err(error) = fs::create_dir_all(&themes) {
            log::warn!("settings: cannot create {}: {error}", themes.display());
        }
        let case_insensitive = filesystem_ignores_case(&themes);

        let folder = fs::canonicalize(folder).unwrap_or_else(|_| absolute_path(folder));
        let mut settings_paths = vec![folder.join(settings_name)];
        if let Ok(path) = fs::canonicalize(&self.path)
            && !settings_paths.contains(&path)
        {
            settings_paths.push(path);
        }
        let mut theme_roots = vec![folder.join(THEMES_DIRECTORY)];
        if fs::symlink_metadata(&themes).is_ok_and(|metadata| metadata.file_type().is_dir())
            && let Ok(path) = fs::canonicalize(&themes)
            && !theme_roots
                .iter()
                .any(|root| same_path(root, &path, case_insensitive))
        {
            theme_roots.push(path);
        }

        let (sender, mut changes) = tokio::sync::mpsc::unbounded_channel();
        let trigger = sender.clone();
        let watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    let changed =
                        changed_files(&event, &settings_paths, &theme_roots, case_insensitive);
                    if changed.any() {
                        sender.send(changed).ok();
                    }
                }
                Err(error) => log::warn!("settings: watch failed: {error}"),
            },
            Config::default().with_follow_symlinks(false),
        );
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                log::warn!("settings: cannot watch {}: {error}", folder.display());
                return;
            }
        };
        if let Err(error) = watcher.watch(&folder, RecursiveMode::Recursive) {
            log::warn!("settings: cannot watch {}: {error}", folder.display());
            return;
        }

        self.watcher = Some(watcher);
        self.reload = Some(cx.spawn(async move |this, cx| {
            let mut retry = FileChanges::default();
            let mut retry_attempts = 0;
            loop {
                let mut pending = if retry.any() {
                    let delay = cx.background_executor().timer(RELOAD_RETRY_DELAY);
                    tokio::pin!(delay);
                    tokio::select! {
                        biased;
                        next = changes.recv() => {
                            let Some(next) = next else {
                                break;
                            };
                            retry_attempts = 0;
                            retry.merge(next);
                            retry
                        },
                        _ = &mut delay => retry,
                    }
                } else {
                    let Some(next) = changes.recv().await else {
                        break;
                    };
                    retry_attempts = 0;
                    next
                };
                let mut disconnected = false;
                loop {
                    let delay = cx.background_executor().timer(RELOAD_DELAY);
                    tokio::pin!(delay);
                    tokio::select! {
                        biased;
                        next = changes.recv() => match next {
                            Some(next) => {
                                retry_attempts = 0;
                                pending.merge(next);
                            }
                            None => {
                                disconnected = true;
                                break;
                            }
                        },
                        _ = &mut delay => break,
                    }
                }

                let requested = match this.update(cx, |this, cx| this.reload_files(pending, cx)) {
                    Ok(retry) => retry,
                    Err(_) => break,
                };
                if requested.any() && retry_attempts < RELOAD_RETRIES {
                    retry_attempts += 1;
                    retry = requested;
                } else {
                    retry_attempts = 0;
                    retry = FileChanges::default();
                }
                if disconnected {
                    break;
                }
            }
        }));
        trigger.send(FileChanges::ALL).ok();
    }

    /// Reloads changed sources together so a theme and its selection appear at once.
    fn reload_files(&mut self, changed: FileChanges, cx: &mut Context<Self>) -> FileChanges {
        let previous_theme = self.theme_overrides();
        let (themes_changed, themes_retry) = if changed.themes {
            let loaded = themes::load(&themes_path(&self.path), &self.themes);
            let different = loaded.themes != self.themes;
            if different {
                self.families = themes::catalog(&loaded.themes);
                self.themes = loaded.themes;
            }
            (different, loaded.retry)
        } else {
            (false, false)
        };
        let settings = match changed.settings {
            true => self.reload_settings(cx),
            false => SettingsReload::Unchanged,
        };
        let settings_changed = matches!(settings, SettingsReload::Changed);
        if settings_changed || previous_theme != self.theme_overrides() {
            cx.emit(Reloaded);
        }
        if themes_changed || settings_changed {
            cx.notify();
        }
        FileChanges {
            settings: matches!(settings, SettingsReload::Retry),
            themes: themes_retry,
        }
    }

    /// Accepts valid external settings, treats an empty file like a deletion, and protects
    /// nonempty broken files from app saves.
    fn reload_settings(&mut self, cx: &mut Context<Self>) -> SettingsReload {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let changed = self.disk.is_some() || self.broken.is_some();
                self.disk = None;
                self.writable = true;
                self.broken = None;
                if changed {
                    self.save = None;
                }
                return SettingsReload::Unchanged;
            }
            Err(error) => {
                log::warn!("settings: cannot read {}: {error}", self.path.display());
                return SettingsReload::Retry;
            }
        };
        if self.disk.as_ref() == Some(&bytes) && self.broken.is_none() {
            self.writable = true;
            self.broken = None;
            return SettingsReload::Unchanged;
        }
        if empty_settings(&bytes) {
            self.disk = Some(bytes);
            self.writable = true;
            self.broken = None;
            return SettingsReload::Unchanged;
        }
        let values = match serde_json::from_slice::<Values>(json(&bytes)) {
            Ok(values) => offer_new_lyrics(values).settled(),
            Err(error) => {
                log::warn!("settings: cannot parse {}: {error}", self.path.display());
                let newly_broken = self.broken != Some(error.line());
                self.writable = false;
                self.broken = Some(error.line());
                self.save = None;
                if newly_broken {
                    self.report_broken(cx);
                }
                return SettingsReload::Unchanged;
            }
        };

        log::info!("settings: reloaded {}", self.path.display());
        let previous = std::mem::replace(&mut self.values, values);
        self.disk = Some(bytes);
        self.writable = true;
        self.broken = None;
        self.save = None;
        self.push_globals(&previous, cx);
        SettingsReload::Changed
    }

    /// Pushes the globals that the setters push themselves, for whatever a reload changed.
    fn push_globals(&self, previous: &Values, cx: &mut App) {
        let (before, now) = (&previous.appearance, &self.values.appearance);
        if previous.language != self.values.language {
            i18n::set(i18n::resolve(&self.values.language));
        }
        if before.icons != now.icons {
            icons::set(&now.icons);
        }
        if before.reduce_motion != now.reduce_motion || before.motion_pace != now.motion_pace {
            ui::motion::apply(self.stillness(), self.pace(), cx);
        }
        cx.refresh_windows();
    }
}

/// The saved window frame and its display, if its centre still lands on a connected display.
pub fn window_placement(least: Size<Pixels>, cx: &App) -> Option<(WindowBounds, DisplayId)> {
    let frame = Wisp::global(cx).settings.read(cx).state.window?;
    if !frame.sane() {
        return None;
    }

    let placement = frame.placement(least);
    let bounds = placement.get_bounds();
    cx.displays()
        .iter()
        .find(|display| display.bounds().contains(&bounds.center()))
        .map(|display| (placement, display.id()))
}

/// Starts saving the window frame for the next launch.
pub fn remember_window(window: &mut Window, cx: &mut App) {
    let settings = Wisp::global(cx).settings.clone();
    settings.update(cx, |settings, cx| settings.watch_window(window, cx));
}

/// Whether reloading `current` from disk would replace the values that serialize to `ours`. A
/// file that does not parse is left to the broken-file toast, since the reload keeps our values.
fn overrides(current: &[u8], ours: &[u8]) -> bool {
    serde_json::from_slice::<Values>(json(current))
        .ok()
        .and_then(|theirs| serde_json::to_vec_pretty(&theirs).ok())
        .is_some_and(|theirs| theirs != ours)
}

/// The JSON in a file without the UTF-8 byte order mark that Notepad and PowerShell can put in
/// front of it on Windows, which serde_json rejects as an unexpected character.
fn json(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// Whether a file contains no settings to protect, allowing a later preference change to save.
fn empty_settings(bytes: &[u8]) -> bool {
    json(bytes).iter().all(u8::is_ascii_whitespace)
}

/// A saved provider list replaces the default one, so each source added since reaches an
/// existing user once.
fn offer_new_lyrics(mut values: Values) -> Values {
    // [wisp] 歌词源已裁到只剩本地、酷狗、网易云。老用户的 settings.json 里仍带着随
    // provider 一起删掉的源名，加载时一次性剔除，避免歌词面板反复请求拿不到东西。
    const DEAD: [&str; 7] = [
        "Spotify",
        "YouTube Music",
        "Apple Music",
        "Musixmatch",
        "LrcLib",
        "Subsonic",
        "Jellyfin",
    ];
    values
        .lyrics_providers
        .retain(|name| !DEAD.contains(&name.as_str()));
    for name in LYRICS_SOURCES {
        if !values.lyrics_providers.iter().any(|held| held == name) {
            values.lyrics_providers.push(name.to_owned());
        }
    }
    values
}

fn themes_path(settings: &Path) -> PathBuf {
    settings
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(THEMES_DIRECTORY)
}

/// Classifies one notify event without relying on backend-specific path spelling.
fn changed_files(
    event: &Event,
    settings: &[PathBuf],
    theme_roots: &[PathBuf],
    case_insensitive: bool,
) -> FileChanges {
    if event.need_rescan() {
        return FileChanges::ALL;
    }
    if !matches!(
        event.kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return FileChanges::default();
    }

    let mut changed = FileChanges::default();
    for path in &event.paths {
        let absolute = absolute_path(path);
        let canonical = fs::canonicalize(&absolute).ok();
        let is_setting = settings.iter().any(|setting| {
            same_path(setting, &absolute, case_insensitive)
                || canonical
                    .as_ref()
                    .is_some_and(|path| same_path(setting, path, case_insensitive))
        });
        if is_setting {
            changed.settings = true;
            continue;
        }

        let theme_path = |path: &Path| {
            let json = path
                .extension()
                .is_some_and(|extension| extension == "json");
            let within = |path: Option<&Path>| {
                path.is_some_and(|path| {
                    theme_roots
                        .iter()
                        .any(|root| same_path(path, root, case_insensitive))
                })
            };
            within(Some(path))
                || within(path.parent())
                || (json && within(path.parent().and_then(Path::parent)))
        };
        if theme_path(&absolute) || canonical.as_deref().is_some_and(theme_path) {
            changed.themes = true;
        }
    }
    changed
}

fn same_path(left: &Path, right: &Path, case_insensitive: bool) -> bool {
    match case_insensitive {
        true => left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()),
        false => left == right,
    }
}

#[cfg(target_os = "windows")]
fn filesystem_ignores_case(_: &Path) -> bool {
    true
}

#[cfg(target_os = "macos")]
fn filesystem_ignores_case(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let name = name.to_string_lossy();
    let toggled = match name.bytes().any(|byte| byte.is_ascii_lowercase()) {
        true => name.to_ascii_uppercase(),
        false => name.to_ascii_lowercase(),
    };
    let other = path.with_file_name(toggled);
    if fs::symlink_metadata(&other).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return false;
    }
    match (fs::canonicalize(path), fs::canonicalize(other)) {
        (Ok(path), Ok(other)) => path == other,
        _ => false,
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn filesystem_ignores_case(_: &Path) -> bool {
    false
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_owned();
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .unwrap_or_else(|_| path.to_owned())
}

/// Keeps the previous valid settings in `wisp.backup.json` before saving a new version.
/// Unchanged saves and empty or broken previous files leave the backup alone. The caller must
/// check that `previous` still matches the last contents it accepted from disk.
fn save_settings(path: &Path, bytes: &[u8], previous: Option<&[u8]>) -> Result<()> {
    if previous == Some(bytes) {
        return Ok(());
    }
    if let Some(previous) =
        previous.filter(|previous| serde_json::from_slice::<Values>(json(previous)).is_ok())
    {
        write_settings(&path.with_file_name("wisp.backup.json"), previous)
            .context("cannot back up settings")?;
    }
    write_settings(path, bytes)
}

/// Replaces settings with a synced file beside the destination, so an interrupted save leaves
/// the previous contents intact. Symlinks keep pointing to their target. The file can hold
/// account credentials, so on unix only its owner may read it.
fn write_settings(path: &Path, bytes: &[u8]) -> Result<()> {
    let path = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::canonicalize(path).context("cannot resolve settings symlink")?
        }
        Ok(_) => path.to_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_owned(),
        Err(error) => return Err(error).context("cannot inspect settings file"),
    };
    let parent = path.parent().context("cannot find settings directory")?;
    match fs::metadata(&path) {
        Ok(metadata) => anyhow::ensure!(
            !metadata.permissions().readonly(),
            "cannot write read-only settings file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot read settings permissions"),
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".settings-")
        .tempfile_in(parent)
        .context("cannot create temporary settings file")?;
    temporary
        .write_all(bytes)
        .context("cannot write temporary settings file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("cannot restrict settings permissions")?;
    }
    temporary
        .as_file()
        .sync_all()
        .context("cannot sync temporary settings file")?;
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .context("cannot replace settings file")?;
    #[cfg(unix)]
    if let Err(error) = fs::File::open(parent).and_then(|folder| folder.sync_all()) {
        log::debug!("settings: cannot sync {}: {error}", parent.display());
    }
    Ok(())
}

/// Where the settings file lives.
fn settings_path() -> PathBuf {
    paths::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wisp")
        .join("settings.json")
}

/// The pins of the given providers, in stored order.
fn gather(pinned: &[Held], slugs: &[&str]) -> Vec<Pin> {
    shown(pinned, slugs)
        .map(|(_, held)| held.pin.clone())
        .collect()
}

/// The pins of `slugs` with their index in the full list. Other providers' pins are skipped, not
/// removed, so they keep their place.
fn shown<'a>(
    pinned: &'a [Held],
    slugs: &'a [&str],
) -> impl Iterator<Item = (usize, &'a Held)> + 'a {
    pinned
        .iter()
        .enumerate()
        .filter(move |(_, held)| slugs.contains(&held.slug.as_str()))
}

/// Removes one pin and reports whether it was there.
fn take(pinned: &mut Vec<Held>, slug: &str, pin: &Pin) -> bool {
    let Some(index) = pinned
        .iter()
        .position(|held| held.slug == slug && held.pin.same(pin))
    else {
        return false;
    };
    pinned.remove(index);
    true
}

/// Carries the position over from the previous record while the same track is current on the
/// same provider.
fn carry(previous: Option<&Resume>, next: &mut Resume) {
    let playing = |resume: &Resume| resume.current.as_ref().map(|stub| stub.id.clone());
    let same = previous.filter(|old| old.provider == next.provider);
    next.position = same
        .filter(|old| playing(old) == playing(next))
        .map_or(0., |old| old.position);
}

/// Inserts or moves `pin` into the `gap`th slot among the pins of `slugs`. `None` or a gap past
/// the end means the end. Returns false when nothing moved.
fn place(pinned: &mut Vec<Held>, slug: &str, pin: Pin, gap: Option<usize>, slugs: &[&str]) -> bool {
    let visible: Vec<usize> = shown(pinned, slugs).map(|(index, _)| index).collect();
    let gap = gap.unwrap_or(visible.len()).min(visible.len());
    let target = match gap {
        0 => visible.first().copied().unwrap_or(pinned.len()),
        gap => visible[gap - 1] + 1,
    };

    let Some(from) = pinned.iter().position(|held| held.pin.same(&pin)) else {
        pinned.insert(
            target,
            Held {
                slug: slug.to_owned(),
                pin,
            },
        );
        return true;
    };

    let to = gap_target(from, target, pinned.len());
    if to == from {
        return false;
    }

    let moved = pinned.remove(from);
    pinned.insert(to, moved);
    true
}

/// The saved order with unknown and repeated names dropped and every source it lacks appended
/// in the default order.
/// Reads the `keybinds` map leniently: a shortcut saved as one string counts as a list of one, and
/// any other value is dropped with a warning, so a hand edit never makes the file unreadable.
fn read_keybinds<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Vec<String>>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let Some(entries) = value.as_object() else {
        log::warn!("settings: keybinds {value} is not an object, keeping the default shortcuts");
        return Ok(BTreeMap::new());
    };
    Ok(entries
        .iter()
        .filter_map(|(id, keys)| {
            let parsed = match keys {
                serde_json::Value::String(key) => Some(vec![key.clone()]),
                serde_json::Value::Array(list) => list
                    .iter()
                    .map(|key| key.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>(),
                _ => None,
            };
            if parsed.is_none() {
                log::warn!("settings: keybind {id} is {keys}, keeping its default keys");
            }
            parsed.map(|parsed| (id.clone(), parsed))
        })
        .collect())
}

/// The saved source order with every source it does not know put in, each right after the nearest
/// source before it in the default order, or last when the list has none of those. A source added
/// since a list was saved then ranks where a fresh install has it.
fn lyrics_priority(saved: &[String]) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    for name in saved.iter().map(String::as_str) {
        if LYRICS_SOURCES.contains(&name) && !order.iter().any(|held| held == name) {
            order.push(name.to_owned());
        }
    }
    for (index, name) in LYRICS_SOURCES.iter().enumerate() {
        if order.iter().any(|held| held == name) {
            continue;
        }
        let after = LYRICS_SOURCES[..index]
            .iter()
            .rev()
            .find_map(|before| order.iter().position(|held| held == before));
        order.insert(after.map_or(order.len(), |at| at + 1), (*name).to_owned());
    }
    order
}

/// The positions in `order` that hold an enabled source.
fn enabled_slots(order: &[String], enabled: &[String]) -> Vec<usize> {
    (0..order.len())
        .filter(|index| enabled.contains(&order[*index]))
        .collect()
}

/// Moves `provider` into a gap between the enabled sources and writes them back into the slots
/// they held, so disabled sources stay where they were. Returns whether anything moved.
fn place_lyrics_provider(
    order: &mut [String],
    enabled: &[String],
    provider: &str,
    gap: usize,
) -> bool {
    let slots = enabled_slots(order, enabled);
    let mut visible: Vec<String> = slots.iter().map(|index| order[*index].clone()).collect();
    let Some(from) = visible.iter().position(|name| name == provider) else {
        return false;
    };
    let to = match gap > from {
        true => gap - 1,
        false => gap,
    };
    if to == from || to >= visible.len() {
        return false;
    }
    let moved = visible.remove(from);
    visible.insert(to, moved);
    for (index, name) in slots.into_iter().zip(visible) {
        order[index] = name;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::queue::Stub;
    use ui::PinKind;

    #[test]
    fn unblock_starts_off_until_a_provider_turns_it_on() {
        assert!(!Values::default().unblock_enabled);
    }

    #[test]
    fn the_default_theme_follows_the_system_without_overriding_a_saved_choice() {
        let values: Values = serde_json::from_str("{}").unwrap();
        assert_eq!(
            ThemeKind::from_id(&values.appearance.theme),
            ThemeKind::System
        );

        let saved: Values =
            serde_json::from_str(r#"{"appearance":{"theme":"wisp/dark"}}"#).unwrap();
        assert_eq!(ThemeKind::from_id(&saved.appearance.theme), ThemeKind::Dark);
    }

    #[test]
    fn a_saved_members_tab_reads_as_the_queue() {
        let tab: SideTab = serde_json::from_str("\"members\"").unwrap();
        assert_eq!(tab, SideTab::Queue);
    }

    #[test]
    fn lyric_effects_keep_the_current_look_and_round_trip_the_motion_preset() {
        let values = Values::default();
        assert!(values.fade_lyrics);
        assert_eq!(values.lyrics_blur_amount, 1.);
        assert!(values.edge_feather_lyrics);
        assert!(!values.distance_scale_lyrics);
        assert!(!values.breathing_lyrics);
        assert!(!values.gap_dots_lyrics);
        assert!(!values.stroke_lyrics);
        assert!(!values.parallax_lyrics);
        assert_eq!(values.lyrics_motion_preset, LyricMotionPreset::Wisp);

        let preset: LyricMotionPreset = serde_json::from_str("\"apple-music\"").unwrap();
        assert_eq!(preset, LyricMotionPreset::AppleMusic);
        assert_eq!(serde_json::to_string(&preset).unwrap(), "\"apple-music\"");
        assert_eq!(LyricMotionPreset::from_id("apple-music"), preset);
        assert_eq!(
            LyricMotionPreset::from_id("unknown"),
            LyricMotionPreset::Wisp
        );
    }

    #[test]
    fn taskbar_progress_defaults_on_and_loads_off_from_settings_json_after_a_restart() {
        let values: Values = serde_json::from_str("{}").unwrap();
        assert!(values.taskbar_progress);

        let root = scratch("taskbar-progress");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        fs::write(&path, r#"{"taskbar_progress": false}"#).unwrap();
        let store = StateStore::new(Database::at(root.join("state.sqlite")));
        let settings = AppSettings::load_from(path, store);
        assert!(!settings.taskbar_progress());
    }

    #[test]
    fn page_width_defaults_to_centered_when_no_setting_is_stored() {
        let values: Values = serde_json::from_str("{}").unwrap();
        assert_eq!(
            PageWidth::from_id(&values.appearance.page_width),
            PageWidth::Centered
        );

        let root = scratch("page-width-default");
        fs::create_dir_all(&root).unwrap();
        let store = StateStore::new(Database::at(root.join("state.sqlite")));
        let settings = AppSettings::load_from(root.join("settings.json"), store);
        assert_eq!(settings.page_width(), PageWidth::Centered);
    }

    #[test]
    fn page_width_wide_loads_from_settings_json_and_unknown_ids_read_as_centered() {
        assert_eq!(PageWidth::from_id("wide"), PageWidth::Wide);
        assert_eq!(PageWidth::from_id("bogus"), PageWidth::Centered);

        let root = scratch("page-width-wide");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        fs::write(&path, r#"{"appearance": {"page_width": "wide"}}"#).unwrap();
        let store = StateStore::new(Database::at(root.join("state.sqlite")));
        let settings = AppSettings::load_from(path, store);
        assert_eq!(settings.page_width(), PageWidth::Wide);
    }

    #[test]
    fn page_width_survives_a_restart() {
        let root = scratch("page-width-restart");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        fs::write(&path, r#"{"appearance": {"page_width": "wide"}}"#).unwrap();
        let store = StateStore::new(Database::at(root.join("state.sqlite")));
        let before = AppSettings::load_from(path.clone(), store);
        assert_eq!(before.page_width(), PageWidth::Wide);

        // Write what a save would write, then load again as a fresh launch does.
        let saved = serde_json::to_vec(&before.values).unwrap();
        fs::write(&path, saved).unwrap();
        let store = StateStore::new(Database::at(root.join("state.sqlite")));
        let after = AppSettings::load_from(path, store);
        assert_eq!(after.page_width(), PageWidth::Wide);
    }

    #[test]
    fn a_provider_slug_in_settings_json_is_kept() {
        let root = scratch("provider-settled");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        fs::write(&path, r#"{"provider": "kugou"}"#).unwrap();
        let store = StateStore::new(Database::at(root.join("state.sqlite")));
        let settings = AppSettings::load_from(path, store);

        assert_eq!(settings.provider(), "kugou");
        assert_eq!(settings.startup_provider(), Some("kugou"));

        drop(settings);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_retired_local_session_choice_migrates_and_a_fresh_file_keeps_no_choice() {
        let mut values: Values = serde_json::from_str(r#"{"provider": "local"}"#).unwrap();
        values.settle_provider();
        assert_eq!(values.provider, None);

        // 没写 provider 的文件保持「跟随上次使用」，迁移不替它写死一个值。
        let mut values: Values = serde_json::from_str("{}").unwrap();
        values.settle_provider();
        assert_eq!(values.provider, None);
    }

    #[test]
    fn a_saved_dynamic_provider_slug_survives_settling() {
        let retired = StateValues {
            provider: "retired-provider".to_owned(),
            ..StateValues::default()
        }
        .settled();
        assert_eq!(retired.provider, "retired-provider");

        let migrated = StateValues {
            provider: LOCAL_PROVIDER.to_owned(),
            ..StateValues::default()
        }
        .settled();
        assert_eq!(migrated.provider, NETEASE);
    }

    #[test]
    fn word_synced_lyrics_start_preferred_and_the_old_key_is_ignored() {
        let fresh: Values = serde_json::from_str("{}").unwrap();
        assert!(fresh.lyrics_prefer_word_synced);

        let old: Values = serde_json::from_str(r#"{"lyrics_word_synced_first":false}"#).unwrap();
        assert!(old.lyrics_prefer_word_synced);
    }

    #[test]
    fn lyrics_priority_defaults_and_round_trips_independently_of_enablement() {
        let mut values: Values = serde_json::from_str("{}").unwrap();
        assert_eq!(
            values.lyrics_provider_priority,
            [LOCAL, "Kugou", "NetEase"].map(str::to_owned)
        );
        assert_eq!(
            lyrics_priority(&values.lyrics_provider_priority),
            lyrics_priority(&[])
        );
        values.lyrics_provider_priority = vec!["Kugou".into(), "NetEase".into()];
        values.lyrics_providers = vec!["NetEase".into()];
        let loaded: Values =
            serde_json::from_str(&serde_json::to_string(&values).unwrap()).unwrap();
        assert_eq!(
            loaded.lyrics_provider_priority,
            values.lyrics_provider_priority
        );
        assert_eq!(loaded.lyrics_providers, values.lyrics_providers);
    }

    #[test]
    fn lyrics_priority_ignores_unknown_and_duplicate_sources_and_appends_missing_ones() {
        let order = lyrics_priority(&["Kugou".into(), "unknown".into(), "Kugou".into()]);
        assert_eq!(order[0], "Kugou");
        assert_eq!(order.len(), 3);
        assert!(!order.iter().any(|name| name == "unknown"));
        assert_eq!(order.iter().filter(|name| *name == "Kugou").count(), 1);
    }

    #[test]
    fn dropping_lyrics_sources_lands_in_the_gap_and_preserves_disabled_slots() {
        let mut order = vec!["NetEase".into(), "Local".into(), "Kugou".into()];
        let enabled = vec!["NetEase".into(), "Kugou".into()];
        assert!(place_lyrics_provider(&mut order, &enabled, "Kugou", 0));
        assert_eq!(order, vec!["Kugou", "Local", "NetEase"]);
        assert!(place_lyrics_provider(&mut order, &enabled, "Kugou", 2));
        assert_eq!(order, vec!["NetEase", "Local", "Kugou"]);
        // a source already where the drop would put it does not move
        assert!(!place_lyrics_provider(&mut order, &enabled, "NetEase", 1));
        assert!(place_lyrics_provider(&mut order, &enabled, "NetEase", 2));
        assert_eq!(order, vec!["Kugou", "Local", "NetEase"]);
        // a source that is switched off has no slot to move between, and neither has a gap
        // past the last enabled one
        assert!(!place_lyrics_provider(&mut order, &enabled, "Local", 0));
        assert!(!place_lyrics_provider(&mut order, &enabled, "NetEase", 9));
        assert!(!place_lyrics_provider(&mut order, &[], "NetEase", 0));
    }

    fn resume(provider: &str, playing: &str, position: f32) -> Resume {
        Resume {
            provider: provider.to_owned(),
            position,
            current: Some(Stub {
                id: playing.to_owned(),
                ..Stub::default()
            }),
            ..Resume::default()
        }
    }

    #[test]
    fn the_pinned_section_starts_closed_in_the_dragged_order() {
        let state = StateValues::default();
        assert!(!state.sidebar_pinned_open);
        assert!(!state.sidebar_full_library);
        assert!(!state.sidebar_pin_reversed);
        assert!(PinSort::from_id(&state.sidebar_pin_sort).is_none());
        assert!(PinSort::from_id("nonsense").is_none());
    }

    #[test]
    fn lyrics_start_karaoke_and_romanize_only_cjk() {
        let values: Values = serde_json::from_str("{}").expect("empty settings use defaults");

        assert!(values.karaoke_lyrics);
        assert!(values.romanized_lyrics);
        let romanized = [
            WritingSystem::Japanese,
            WritingSystem::Chinese,
            WritingSystem::Korean,
        ];
        for system in WritingSystem::ALL {
            assert_eq!(
                values.romanization_scripts.contains(system),
                romanized.contains(&system)
            );
        }
    }

    #[test]
    fn one_saved_romanization_choice_keeps_the_other_defaults() {
        let values: Values = serde_json::from_str(
            r#"{
                "romanization_scripts": { "japanese": false }
            }"#,
        )
        .expect("partial script preferences use defaults");

        assert!(
            !values
                .romanization_scripts
                .contains(WritingSystem::Japanese)
        );
        assert!(values.romanization_scripts.contains(WritingSystem::Chinese));
        assert!(!values.romanization_scripts.contains(WritingSystem::Other));
    }

    #[test]
    fn the_saved_position_follows_the_same_track() {
        let previous = resume("netease", "abc", 42.);
        let mut next = resume("netease", "abc", 0.);

        carry(Some(&previous), &mut next);

        assert_eq!(next.position, 42.);
    }

    #[test]
    fn a_new_track_starts_from_the_beginning() {
        let previous = resume("netease", "abc", 42.);
        let mut next = resume("netease", "def", 0.);

        carry(Some(&previous), &mut next);

        assert_eq!(next.position, 0.);
    }

    #[test]
    fn another_provider_never_inherits_a_position() {
        let previous = resume("netease", "abc", 42.);
        let mut next = resume("retired-provider", "abc", 0.);

        carry(Some(&previous), &mut next);

        assert_eq!(next.position, 0.);
    }

    #[test]
    fn a_first_record_starts_from_the_beginning() {
        let mut next = resume("netease", "abc", 42.);

        carry(None, &mut next);

        assert_eq!(next.position, 0.);
    }

    const SLUGS: [&str; 2] = ["netease", "local"];

    fn pin(id: &str) -> Pin {
        Pin::new(PinKind::Album, id, id)
    }

    fn held(slug: &str, id: &str) -> Held {
        Held {
            slug: slug.to_owned(),
            pin: pin(id),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "wisp-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn runtime_state_round_trips_through_sqlite() {
        let root = scratch("state");
        let path = root.join("state.sqlite");
        let store = StateStore::new(Database::at(path.clone()));
        let state = StateValues {
            volume: 0.2,
            provider: "netease".to_owned(),
            sidebar_right_open: true,
            pinned: vec![held("netease", "album")],
            ..StateValues::default()
        };

        store.save(&state).expect("state saves");
        let loaded = store.load().expect("state loads").expect("state exists");

        assert_eq!(loaded.volume, 0.2);
        assert_eq!(loaded.provider, "netease");
        assert!(loaded.sidebar_right_open);
        assert_eq!(loaded.pinned.len(), 1);

        drop(store);
        fs::remove_file(path).expect("test database is removed");
        fs::remove_dir(root).expect("test directory is removed");
    }

    fn ids(pinned: &[Held]) -> Vec<&str> {
        pinned.iter().map(|held| held.pin.id.as_str()).collect()
    }

    #[test]
    fn a_fresh_pin_lands_at_the_gap() {
        let mut pinned = vec![held("netease", "a"), held("netease", "b")];

        assert!(place(&mut pinned, "netease", pin("c"), Some(1), &SLUGS));
        assert_eq!(ids(&pinned), ["a", "c", "b"]);
    }

    #[test]
    fn no_gap_appends() {
        let mut pinned = vec![held("netease", "a")];

        assert!(place(&mut pinned, "netease", pin("b"), None, &SLUGS));
        assert_eq!(ids(&pinned), ["a", "b"]);
    }

    #[test]
    fn a_gap_past_the_end_still_appends() {
        let mut pinned = vec![held("netease", "a")];

        assert!(place(&mut pinned, "netease", pin("b"), Some(9), &SLUGS));
        assert_eq!(ids(&pinned), ["a", "b"]);
    }

    #[test]
    fn pinning_twice_moves_instead_of_duplicating() {
        let mut pinned = vec![
            held("netease", "a"),
            held("netease", "b"),
            held("netease", "c"),
        ];

        assert!(place(&mut pinned, "netease", pin("a"), Some(3), &SLUGS));
        assert_eq!(ids(&pinned), ["b", "c", "a"]);
    }

    #[test]
    fn a_move_backwards_keeps_the_gap() {
        let mut pinned = vec![
            held("netease", "a"),
            held("netease", "b"),
            held("netease", "c"),
        ];

        assert!(place(&mut pinned, "netease", pin("c"), Some(0), &SLUGS));
        assert_eq!(ids(&pinned), ["c", "a", "b"]);
    }

    #[test]
    fn the_gaps_around_an_item_are_no_ops() {
        let mut pinned = vec![
            held("netease", "a"),
            held("netease", "b"),
            held("netease", "c"),
        ];

        assert!(!place(&mut pinned, "netease", pin("b"), Some(1), &SLUGS));
        assert!(!place(&mut pinned, "netease", pin("b"), Some(2), &SLUGS));
        assert_eq!(ids(&pinned), ["a", "b", "c"]);
    }

    #[test]
    fn kinds_with_the_same_id_stay_apart() {
        let mut pinned = vec![held("netease", "x")];

        assert!(place(
            &mut pinned,
            "netease",
            Pin::new(PinKind::Song, "x", "x"),
            None,
            &SLUGS
        ));
        assert_eq!(pinned.len(), 2);
    }
}
