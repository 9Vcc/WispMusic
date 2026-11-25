// [wisp] vendored Sonora 界面代码，设置页裁剪后留下未用的槽位与行构造器，暂不逐个删除。
#![allow(dead_code)]
mod chrome;
mod root;
mod screens;
mod shared;
mod shells;

pub use chrome::ToastStack;
pub use chrome::tray_menu::{MENU_HEIGHT, MENU_WIDTH, TrayMenuClosed, TrayMenuView};
#[cfg(target_os = "macos")]
pub use chrome::traffic_lights;
pub use root::Root;
pub use shells::desktop_lyrics::install as install_desktop_lyrics;
/// Shows or closes the mini player from outside the interface, for the tray and the
/// second-instance hand-off, which both work on the main window instead.
pub use shells::mini::{dismiss as dismiss_mini, reveal as reveal_mini};
/// Whether the typeface picker is probing a family right now, which is the only reason a panic
/// is expected in this process. The panic hook reads it to stay quiet about those.
pub use shared::typeface::probing as probing_typefaces;
use screens::artist::ArtistView;
use screens::detail::DetailView;
use screens::equalizer::EqualizerView;
use screens::genre::GenreView;
use screens::history::HistoryView;
use screens::home::HomeView;
pub use screens::library::LibraryView;
pub use screens::login::LoginView;
pub use screens::settings::SettingsView;
use screens::song::SongView;
use screens::user::UserView;
use shared::adaptive::Adaptive;
use shells::fullscreen::FullscreenView;
