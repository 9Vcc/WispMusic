mod native;

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use futures::AsyncReadExt as _;
use gpui::http_client::{AsyncBody, HttpClient};
use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use i18n::t;
use router::Destination;
use state::{Outcome, PlaybackState, Repeat, Wisp, Toasts};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use native::Icon;

const CAPTION_LIMIT: usize = 26;

/// The longest side of the cover handed to the menu. macOS draws it 18pt tall whatever it
/// measures, Windows and the status notifier hosts draw it at its own size.
const COVER: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Show,
    /// [wisp] 打开自绘托盘菜单（光标处的物理像素坐标）。
    OpenMenu { x: f64, y: f64 },
    /// Show the window when it is hidden or minimized and hide it when it is visible.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Window,
    Song,
    /// [wisp] 播放模式四选一（托盘菜单的 CheckMenuItem）。
    Sequential,
    List,
    Random,
    Single,
    /// [wisp] 音量三项。
    Toggle,
    Previous,
    Next,
    Quit,
}

/// The cover of the playing track, decoded and scaled for a menu row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Art {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub artwork: Option<Arc<Art>>,
    pub caption: String,
    /// Whether the caption opens anything, so a row that leads nowhere stays inert.
    pub song: bool,
    pub toggle: SharedString,
    pub previous: SharedString,
    pub next: SharedString,
    pub shuffle: SharedString,
    pub shuffle_on: bool,
    pub repeat: SharedString,
    pub repeat_on: bool,
    /// [wisp] 自绘/原生托盘菜单的播放模式四选一（当前模式勾选）。
    pub sequential: bool,
    pub sequential_label: SharedString,
    pub list: bool,
    pub list_label: SharedString,
    pub random: bool,
    pub random_label: SharedString,
    pub single: bool,
    pub single_label: SharedString,
    pub show: SharedString,
    pub quit: SharedString,
    pub playing: bool,
}

impl Shown {
    /// What the Dock menu draws. GPUI keeps every action a Dock menu was built with for the life of
    /// the app, so the menu is rebuilt only when this changes, never on a caption or cover alone.
    fn docked(&self) -> (&str, &str, &str, &str, bool, &str, bool) {
        (
            &self.toggle,
            &self.previous,
            &self.next,
            &self.shuffle,
            self.shuffle_on,
            &self.repeat,
            self.repeat_on,
        )
    }
}

struct Installed {
    _tray: Entity<Tray>,
}

impl Global for Installed {}

/// Sets up the tray icon and its menu. Returns whether Wisp may keep running once its window
/// closes, which it only does when the desktop has a tray to bring it back from.
pub fn install(
    show: impl Fn(&mut App) + 'static,
    toggle: impl Fn(&mut App) + 'static,
    open_menu: impl Fn(f64, f64, &mut App) + 'static,
    cx: &mut App,
) -> bool {
    let (sender, receiver) = mpsc::unbounded_channel();
    let Some(icon) = Icon::new(sender) else {
        return false;
    };
    let tray = cx.new(|cx| Tray::new(icon, receiver, show, toggle, open_menu, cx));
    cx.set_global(Installed { _tray: tray });
    Icon::hosted()
}

pub struct Tray {
    icon: Icon,
    shown: Shown,
    /// Whether the icon is in the tray now, which `place` compares against the setting.
    placed: bool,
    /// A shell that refused the status item once will not take it later in this run.
    failed: bool,
    /// The cover the art below was loaded from, so a repeat of the same track loads nothing.
    cover: Option<String>,
    art: Option<Arc<Art>>,
    artwork: Option<Task<()>>,
    _events: Task<()>,
}

impl Tray {
    fn new(
        mut icon: Icon,
        mut receiver: UnboundedReceiver<Event>,
        show: impl Fn(&mut App) + 'static,
        toggle: impl Fn(&mut App) + 'static,
        open_menu: impl Fn(f64, f64, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let _events = cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                if this.upgrade().is_none() {
                    break;
                }
                cx.update(|cx| match event {
                    Event::Show => show(cx),
                    Event::OpenMenu { x, y } => open_menu(x, y, cx),
                    Event::Window => toggle(cx),
                    Event::Song => {
                        show(cx);
                        open(cx);
                    }
                    Event::Quit => cx.quit(),
                    Event::Toggle
                    | Event::Previous
                    | Event::Next
                    | Event::Sequential
                    | Event::List
                    | Event::Random
                    | Event::Single
                    => {
                        let playback = Wisp::global(cx).playback.clone();
                        playback.update(cx, |playback, cx| match event {
                            Event::Toggle => playback.toggle_play(cx),
                            Event::Previous => playback.previous(cx),
                            Event::Sequential => {
                                playback.set_shuffle(false, cx);
                                playback.set_repeat(Repeat::Off, cx);
                            }
                            Event::List => {
                                playback.set_shuffle(false, cx);
                                playback.set_repeat(Repeat::All, cx);
                            }
                            Event::Random => playback.set_shuffle(true, cx),
                            Event::Single => playback.set_repeat(Repeat::One, cx),
                            _ => playback.next(cx),
                        });
                    }
                });
            }
        });

        let playback = Wisp::global(cx).playback.clone();
        cx.observe(&playback, |this, _, cx| this.publish(cx))
            .detach();
        let queue = Wisp::global(cx).queue.clone();
        cx.observe(&queue, |this, _, cx| this.publish(cx)).detach();
        // only `place` here: settings notifies on every window move, and rebuilding `Shown`
        // allocates the caption and clones the cover each time
        let settings = Wisp::global(cx).settings.clone();
        cx.observe(&settings, |this, _, cx| this.place(cx)).detach();

        let shown = shown(None, cx);
        icon.show(&shown);
        crate::dock::menu(&shown, cx);
        let mut tray = Self {
            icon,
            shown,
            placed: false,
            failed: false,
            cover: None,
            art: None,
            artwork: None,
            _events,
        };
        tray.place(cx);
        tray.follow(cx);
        tray
    }

    /// Puts the icon in the tray, or takes it out, following `tray_icon` alone. An icon that cannot
    /// be placed turns `tray_icon` off and says so.
    fn place(&mut self, cx: &mut Context<Self>) {
        if self.failed {
            return;
        }
        let placed = Wisp::global(cx).settings.read(cx).tray_icon();
        if placed == self.placed {
            return;
        }
        if let Err(error) = self.icon.place(placed) {
            log::warn!("tray: cannot place the tray icon: {error:#}");
            if placed {
                Toasts::show(Outcome::Failed, "toast-tray-unavailable", cx);
                let settings = Wisp::global(cx).settings.clone();
                settings.update(cx, |settings, cx| settings.set_tray_icon(false, cx));
                return;
            }
        }
        self.placed = placed;
        if !placed {
            return;
        }
        if let Some(rect) = self.icon.rect() {
            log::info!(
                "tray: icon rect {}x{} at {},{}",
                rect.size.width,
                rect.size.height,
                rect.position.x,
                rect.position.y
            );
        } else {
            log::warn!("tray: the shell refused the status item");
            self.failed = true;
            self.placed = false;
            // Do this after the tray entity finishes constructing: updating settings or showing
            // a toast here re-enters GPUI while `Tray::new` is still on the stack.
            let settings = Wisp::global(cx).settings.clone();
            cx.defer(move |cx| {
                settings.update(cx, |settings, cx| {
                    if settings.close_to_tray() {
                        settings.set_close_to_tray(false, cx);
                    }
                });
                Toasts::show(Outcome::Failed, "toast-tray-unavailable", cx);
            });
            return;
        }
        // the cover of whatever is playing went unfetched while the icon was out, so forget
        // the one `follow` last saw and let it load again
        if self.art.is_none() {
            self.cover = None;
        }
        self.publish(cx);
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        self.follow(cx);
        let shown = shown(self.art.clone(), cx);
        if shown == self.shown {
            return;
        }
        self.icon.show(&shown);
        if shown.docked() != self.shown.docked() {
            crate::dock::menu(&shown, cx);
        }
        self.shown = shown;
    }

    /// Starts loading the cover of the track that is playing now. Dropping the task cancels the
    /// load for the track that was playing before, so a run of skips only draws the last cover.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let cover = Wisp::global(cx)
            .playback
            .read(cx)
            .shown_track(cx)
            .and_then(|track| track.cover);
        if cover == self.cover {
            return;
        }

        self.cover = cover.clone();
        self.art = None;
        // only the tray menu draws the cover, so an icon that is out of the tray fetches none
        self.artwork = match self.placed {
            true => cover.map(|cover| self.load(cover, cx)),
            false => None,
        };
    }

    fn load(&self, cover: String, cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this, cx| {
            let art = match cx.background_spawn(art(cover, http)).await {
                Ok(art) => art,
                Err(error) => return log::warn!("tray: cannot draw the cover: {error:#}"),
            };
            this.update(cx, |this, cx| {
                this.art = Some(Arc::new(art));
                this.publish(cx);
            })
            .ok();
        })
    }
}

/// Opens the song page of whatever is playing. A track without an id is not an error: the caption
/// row is only enabled when there is one.
fn open(cx: &mut App) {
    let playing = Wisp::global(cx).playback.read(cx).shown_track(cx);
    let Some(id) = playing.and_then(|track| track.id) else {
        return;
    };
    router::navigate(Destination::Song(id.into()), cx);
}

/// Reads a cover, over http or from the disk, and scales it down to a menu row. An unreadable
/// cover leaves the caption on its own rather than holding up the rest of the menu.
async fn art(cover: String, http: Arc<dyn HttpClient>) -> Result<Art> {
    let bytes = match cover.starts_with("http://") || cover.starts_with("https://") {
        true => fetch(&http, &cover).await?,
        false => {
            let path = cover.strip_prefix("file://").unwrap_or(&cover);
            std::fs::read(path).context("cannot read the cover")?
        }
    };
    let image = image::load_from_memory(&bytes)
        .context("cannot decode the cover")?
        .thumbnail(COVER, COVER)
        .into_rgba8();

    let (width, height) = image.dimensions();
    Ok(Art {
        data: image.into_raw(),
        width,
        height,
    })
}

async fn fetch(http: &Arc<dyn HttpClient>, url: &str) -> Result<Vec<u8>> {
    let mut response = http
        .get(url, AsyncBody::empty(), true)
        .await
        .context("cannot fetch the cover")?;
    if !response.status().is_success() {
        bail!("the cover request answered {}", response.status());
    }

    let mut bytes = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut bytes)
        .await
        .context("cannot read the cover")?;

    Ok(bytes)
}

fn shown(artwork: Option<Arc<Art>>, cx: &App) -> Shown {
    let playback = Wisp::global(cx).playback.read(cx);
    let playing = matches!(
        playback.state(),
        PlaybackState::Playing | PlaybackState::Loading
    );
    let track = playback.shown_track(cx);
    let caption = match &track {
        Some(track) => {
            let full = match track.artists.is_empty() {
                true => track.name.clone(),
                false => format!("{} – {}", track.artists, track.name),
            };
            clip(&full, CAPTION_LIMIT)
        }
        None => t!("player-nothing-playing").to_string(),
    };
    let song = track.is_some_and(|track| track.id.is_some());
    let shuffle_on = Wisp::global(cx).queue.read(cx).shuffle();
    let repeat_on = playback.repeat() != Repeat::Off;
    // 播放模式四选一：随机优先于循环（与主窗口的语义一致）。
    let sequential = !shuffle_on && playback.repeat() == Repeat::Off;
    let list = !shuffle_on && playback.repeat() == Repeat::All;
    let random = shuffle_on;
    let single = !shuffle_on && playback.repeat() == Repeat::One;
    Shown {
        artwork,
        caption,
        song,
        toggle: match playing {
            true => t!("tray-pause"),
            false => t!("tray-play"),
        },
        previous: t!("player-previous"),
        next: t!("player-next"),
        shuffle: t!("player-shuffle"),
        shuffle_on,
        repeat: t!("player-repeat"),
        repeat_on,
        sequential,
        sequential_label: t!("tray-mode-sequential"),
        list,
        list_label: t!("tray-mode-list"),
        random,
        random_label: t!("tray-mode-random"),
        single,
        single_label: t!("tray-mode-single"),
        show: t!("tray-show"),
        quit: t!("app-quit"),
        playing,
    }
}

/// Truncates `text` to at most `limit` characters, appending an ellipsis if shortened.
fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some(_) => {
            let cut = limit.saturating_sub(1);
            let offset = text
                .char_indices()
                .nth(cut)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            let trimmed = text[..offset].trim_end_matches(|c: char| {
                c.is_whitespace() || c == '-' || c == '–' || c == '—' || c == ',' || c == '('
            });
            format!("{trimmed}…")
        }
        None => text.to_string(),
    }
}
