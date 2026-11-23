use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, Render,
    SharedString, Window, div, img, px, svg,
};
use i18n::t;
use state::{AppSettings, Playback};
use ui::{ActiveTheme as _, Text};

use icons;

/// 菜单窗口的宽（与原版 tray-menu 窗口一致）。
pub const MENU_WIDTH: f32 = 200.;
/// 窗口高度：按内容实算——内边距 16 + 头部 54 + 分隔 15 + 文字行 32×2
/// + 图标行 44 = 193，留 3px 余量，退出按钮完整显示、不裁切。
pub const MENU_HEIGHT: f32 = 208.;

/// 自绘托盘菜单（与原版 TrayMenu.tsx 像素级同款）：200×412 的浮层窗口，
/// 卡片色 86% 起底、前景 22% 描边，歌曲头 + 菜单项 + 播放模式子菜单。
pub struct TrayMenuView {
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    focus: FocusHandle,
    /// 由 wisp 侧注入：显示主窗口 / 关闭本菜单窗口。
    show_main: Rc<dyn Fn(&mut App)>,
    close: Rc<dyn Fn(&mut App)>,
}

pub struct TrayMenuClosed;

impl EventEmitter<TrayMenuClosed> for TrayMenuView {}

impl Focusable for TrayMenuView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// 一条菜单行：16px 图标 + 文本，右侧可带勾或 chevron，禁用行灰化不响应。
/// 点击行为由调用处链上 `on_click`。
#[allow(clippy::too_many_arguments)]
fn row(
    id: &'static str,
    icon: &'static str,
    label: SharedString,
    checked: bool,
    chevron: bool,
    disabled: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let theme = *cx.theme();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(8.))
        .py(px(6.))
        .rounded_sm()
        .text_size(theme.text(Text::Body))
        .text_color(theme.foreground)
        .when(disabled, |this| {
            this.text_color(theme.muted_foreground).cursor_default()
        })
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(move |style| style.bg(theme.secondary_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    svg()
                        .path(icons::path(icon))
                        .size(px(16.))
                        .text_color(theme.foreground),
                )
                .child(label),
        )
        .when(chevron, |this| {
            this.child(
                svg()
                    .path(icons::path("icons/chevron-right.svg"))
                    .size(px(16.))
                    .text_color(theme.muted_foreground),
            )
        })
        .when(checked, |this| {
            this.child(
                div()
                    .flex_none()
                    .w(px(16.))
                    .text_center()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.primary)
                    .child("✓"),
            )
        })
}

/// 播放控制里的图标按钮：圆形、悬停有底色、只有图标没有文字。
fn transport_button(
    id: &'static str,
    icon: &'static str,
    theme: ui::Theme,
    on_click: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded_md()
        .cursor_pointer()
        .hover(move |style| style.bg(theme.secondary_hover))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| on_click(cx))
        .child(
            svg()
                .path(icons::path(icon))
                .size(px(18.))
                .text_color(theme.foreground),
        )
}

fn separator(cx: &App) -> AnyElement {
    let theme = *cx.theme();
    div()
        .w_full()
        .h(px(1.))
        .flex_none()
        .my(px(2.))
        .bg(theme.foreground.opacity(0.15))
        .into_any_element()
}

impl TrayMenuView {
    pub fn new(
        playback: Entity<Playback>,
        settings: Entity<AppSettings>,
        show_main: Rc<dyn Fn(&mut App)>,
        close: Rc<dyn Fn(&mut App)>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            playback,
            settings,
            focus: cx.focus_handle(),
            show_main,
            close,
        }
    }

    /// 托盘菜单每次弹出时调用：刷新一次视图（与原版 tray-menu-shown 一致）。
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }
}

impl Render for TrayMenuView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let playback = self.playback.read(cx);
        let playing = playback.wants_playing();
        let (title, artist, cover) = match playback.shown_track(cx) {
            Some(track) => (
                SharedString::from(track.name.clone()),
                SharedString::from(track.artists.clone()),
                track.cover,
            ),
            None => (
                SharedString::from("WispMusic"),
                SharedString::from(t!("tray-artist-none")),
                None,
            ),
        };
        let playback = self.playback.clone();

        // 底色与模糊跟随设置：模糊开着用卡片色 86% 起底 + backdrop 模糊，
        // 关掉时退回不透明卡片色（与原版 tray-menu-root 的说明一致）。
        let frosted = self.settings.read(cx).look().blur_window;
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(8.))
            .bg(match frosted {
                true => theme.popover.opacity(0.86),
                false => theme.popover,
            })
            .when(frosted, |this| this.backdrop_blur(ui::GLASS_BLUR))
            .border_1()
            .border_color(theme.foreground.opacity(0.22))
            .text_color(theme.foreground)
            .font_family("Microsoft Yahei")
            .track_focus(&self.focus)
            // 歌曲头：封面 40px + 歌名/歌手。
            .child(
                div()
                    .id("tray-song")
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(8.))
                    .py(px(6.))
                    .mb(px(2.))
                    .rounded_sm()
                    .cursor_default()
                    .hover(move |style| style.bg(theme.secondary_hover))
                    .child(
                        div()
                            .flex_none()
                            .size(px(40.))
                            .rounded_sm()
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.foreground.opacity(0.12))
                            .bg(theme.foreground.opacity(0.06))
                            .when_some(cover, |this, cover| {
                                this.child(img(cover).size_full().rounded_sm().overflow_hidden())
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.foreground)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(artist),
                            ),
                    ),
            )
            .child(separator(cx))
            .child(
                row(
                    "tray-show",
                    "icons/monitor.svg",
                    t!("tray-show"),
                    false,
                    false,
                    false,
                    cx,
                )
                .on_click({
                    let show_main = self.show_main.clone();
                    move |_, _, cx| {
                        cx.stop_propagation();
                        (show_main)(cx);
                    }
                }),
            )
            .child(separator(cx))
            // 播放控制：只留图标，一行三个（原版那种带文字的行去掉）。
            .child(
                div()
                    .id("tray-transport")
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(24.))
                    .py(px(7.))
                    .child(transport_button(
                        "tray-previous",
                        "icons/skip-back.svg",
                        theme,
                        {
                            let playback = playback.clone();
                            move |cx: &mut App| {
                                playback.update(cx, |playback, cx| playback.previous(cx));
                            }
                        },
                    ))
                    .child(transport_button(
                        "tray-toggle",
                        if playing {
                            "icons/pause.svg"
                        } else {
                            "icons/play.svg"
                        },
                        theme,
                        {
                            let playback = playback.clone();
                            move |cx: &mut App| {
                                playback.update(cx, |playback, cx| playback.toggle_play(cx));
                            }
                        },
                    ))
                    .child(transport_button(
                        "tray-next",
                        "icons/skip-forward.svg",
                        theme,
                        {
                            let playback = playback.clone();
                            move |cx: &mut App| {
                                playback.update(cx, |playback, cx| playback.next(cx));
                            }
                        },
                    )),
            )
            .child(separator(cx))
            .child(
                row(
                    "tray-quit",
                    "icons/x.svg",
                    t!("app-quit"),
                    false,
                    false,
                    false,
                    cx,
                )
                .on_click(|_, _, cx| cx.quit()),
            )
    }
}
