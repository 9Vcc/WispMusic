use anyhow::{Context as _, Result};
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{
    CheckMenuItem, Icon as MenuIcon, IconMenuItem, Menu, MenuEvent, MenuId, MenuItem,
    PredefinedMenuItem,
};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{Art, Event, Shown};

const TOOLTIP: &str = "Wisp";
const MENU_ON_CLICK: bool = false;

/// The tray icon and its menu. The status item is built the first time it goes into the tray, so
/// a Wisp started with the icon off never shows it, not even for a frame.
pub struct Icon {
    /// The Wisp logo the status item is built with.
    logo: tray_icon::Icon,
    menu: Menu,
    icon: Option<TrayIcon>,
    /// The last caption, for the tooltip of a status item built after it was shown.
    tooltip: String,
    caption: IconMenuItem,
    toggle: MenuItem,
    previous: MenuItem,
    next: MenuItem,
    sequential: CheckMenuItem,
    list: CheckMenuItem,
    random: CheckMenuItem,
    single: CheckMenuItem,
    show: MenuItem,
    quit: MenuItem,
}

impl Icon {
    pub fn new(sender: UnboundedSender<Event>) -> Option<Self> {
        let logo = match logo() {
            Ok(logo) => logo,
            Err(error) => {
                log::warn!("tray: cannot build the tray icon: {error:#}");
                return None;
            }
        };

        let caption = IconMenuItem::with_id("caption", "", false, None, None);
        let toggle = MenuItem::with_id("toggle", "", true, None);
        let previous = MenuItem::with_id("previous", "", true, None);
        let next = MenuItem::with_id("next", "", true, None);
        let sequential = CheckMenuItem::with_id("mode-sequential", "", true, false, None);
        let list = CheckMenuItem::with_id("mode-list", "", true, false, None);
        let random = CheckMenuItem::with_id("mode-random", "", true, false, None);
        let single = CheckMenuItem::with_id("mode-single", "", true, false, None);
        let show = MenuItem::with_id("show", "", true, None);
        let quit = MenuItem::with_id("quit", "", true, None);
        let menu = Menu::new();
        if let Err(error) = menu.append_items(&[
            &caption,
            &PredefinedMenuItem::separator(),
            &toggle,
            &previous,
            &next,
            &PredefinedMenuItem::separator(),
            &sequential,
            &list,
            &random,
            &single,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::separator(),
            &show,
            &quit,
        ]) {
            log::warn!("tray: cannot build the tray menu: {error:#}");
            return None;
        }

        let menus = sender.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(event) = translate(&event.id) {
                menus.send(event).ok();
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let TrayIconEvent::Click {
                button,
                button_state,
                position,
                ..
            } = event
            else {
                return;
            };
            // [wisp] 右键打开自绘托盘菜单（Windows）；左键仍是显示/隐藏主窗口。
            #[cfg(target_os = "windows")]
            if button == tray_icon::MouseButton::Right
                && button_state == tray_icon::MouseButtonState::Up
            {
                sender
                    .send(Event::OpenMenu {
                        x: position.x,
                        y: position.y,
                    })
                    .ok();
                return;
            }
            if let Some(event) = click(button, button_state) {
                sender.send(event).ok();
            }
        }));

        Some(Self {
            logo,
            menu,
            icon: None,
            tooltip: TOOLTIP.to_owned(),
            caption,
            toggle,
            previous,
            next,
            sequential,
            list,
            random,
            single,
            show,
            quit,
        })
    }

    /// Whether there is a tray to bring Wisp back from, which Windows and macOS always have.
    pub fn hosted() -> bool {
        true
    }

    /// The rectangle the shell reports for this item, or `None` when it never entered the tray.
    pub fn rect(&self) -> Option<tray_icon::Rect> {
        self.icon.as_ref()?.rect()
    }

    /// Puts the icon in the tray, or takes it out. The status item stays alive once built, so the
    /// menu and its handlers survive a round trip.
    pub fn place(&mut self, placed: bool) -> Result<()> {
        match &self.icon {
            Some(icon) => icon
                .set_visible(placed)
                .context("cannot show or hide the status item")?,
            None if placed => self.icon = Some(self.build()?),
            None => {}
        }
        Ok(())
    }

    /// Builds the status item, which puts it in the tray.
    fn build(&self) -> Result<TrayIcon> {
        let builder = TrayIconBuilder::new();
        let builder = builder.with_icon(self.logo.clone());
        let icon = builder
            .with_tooltip(&self.tooltip)
            .with_menu(Box::new(self.menu.clone()))
            .with_menu_on_left_click(MENU_ON_CLICK)
            // [wisp] 右键改由自绘菜单接管，不再弹系统菜单。
            .with_menu_on_right_click(false)
            .build()
            .context("cannot build the status item")?;
        Ok(icon)
    }

    pub fn show(&mut self, shown: &Shown) {
        // the status notifier hosts read the caption off the tooltip themselves; here it has to
        // be pushed, or hovering the icon only ever says Wisp
        self.tooltip.clone_from(&shown.caption);
        if let Some(icon) = &self.icon
            && let Err(error) = icon.set_tooltip(Some(&shown.caption))
        {
            log::warn!("tray: cannot set the tooltip: {error:#}");
        }
        self.caption.set_text(&shown.caption);
        self.caption.set_icon(cover(shown.artwork.as_deref()));
        self.caption.set_enabled(shown.song);
        self.toggle.set_text(&shown.toggle);
        self.previous.set_text(&shown.previous);
        self.next.set_text(&shown.next);
        // 播放模式四选一：当前模式勾选，其余取消。
        self.sequential.set_checked(shown.sequential);
        self.list.set_checked(shown.list);
        self.random.set_checked(shown.random);
        self.single.set_checked(shown.single);
        self.sequential.set_text(&shown.sequential_label);
        self.list.set_text(&shown.list_label);
        self.random.set_text(&shown.random_label);
        self.single.set_text(&shown.single_label);
        self.show.set_text(&shown.show);
        self.quit.set_text(&shown.quit);
    }
}

/// Loads the Windows artwork from the executable at the system tray's current pixel size.
/// Windows 11's notification area uses a 32 px button at 96 DPI, not the 16 px small-icon
/// metric; loading the small icon makes the shell upscale it and soften the edges.
#[cfg(windows)]
fn logo() -> Result<tray_icon::Icon> {
    use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
    use windows::Win32::UI::WindowsAndMessaging::{SM_CXICON, SM_CYICON};

    let size = unsafe {
        let dpi = GetDpiForSystem();
        (
            GetSystemMetricsForDpi(SM_CXICON, dpi) as u32,
            GetSystemMetricsForDpi(SM_CYICON, dpi) as u32,
        )
    };
    tray_icon::Icon::from_resource(1, Some(size)).context("cannot load the tray icon resource")
}

/// Turns the playing cover into an icon for the menu's caption row. An unreadable
/// cover leaves the caption on its own rather than holding up the rest of the menu.
fn cover(art: Option<&Art>) -> Option<MenuIcon> {
    let art = art?;
    match MenuIcon::from_rgba(art.data.clone(), art.width, art.height) {
        Ok(icon) => Some(icon),
        Err(error) => {
            log::warn!("tray: cannot build the cover icon: {error:#}");
            None
        }
    }
}

/// What a click on the icon asks for. A left button release toggles the window, except where the
/// icon opens its menu on a left click, and every other click is left to the system.
fn click(button: MouseButton, state: MouseButtonState) -> Option<Event> {
    match (button, state, MENU_ON_CLICK) {
        (MouseButton::Left, MouseButtonState::Up, false) => Some(Event::Window),
        _ => None,
    }
}

fn translate(id: &MenuId) -> Option<Event> {
    Some(match id.0.as_str() {
        "caption" => Event::Song,
        "toggle" => Event::Toggle,
        "previous" => Event::Previous,
        "next" => Event::Next,
        "mode-sequential" => Event::Sequential,
        "mode-list" => Event::List,
        "mode-random" => Event::Random,
        "mode-single" => Event::Single,
        "show" => Event::Show,
        "quit" => Event::Quit,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_release_toggles_the_window() {
        let asked = click(MouseButton::Left, MouseButtonState::Up);
        assert_eq!(
            asked,
            match MENU_ON_CLICK {
                true => None,
                false => Some(Event::Window),
            }
        );
    }

    #[test]
    fn other_clicks_are_left_alone() {
        assert_eq!(click(MouseButton::Left, MouseButtonState::Down), None);
        assert_eq!(click(MouseButton::Right, MouseButtonState::Up), None);
        assert_eq!(click(MouseButton::Right, MouseButtonState::Down), None);
        assert_eq!(click(MouseButton::Middle, MouseButtonState::Up), None);
    }
}
