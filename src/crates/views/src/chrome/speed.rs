//! 播放条上的变速控件（Wisp 口径），占用原先「共享收听」按钮的位置。
//! 点开是 0.5×–2.0× 的六档；选定后引擎在原位置重开当前曲目以立即生效。

use i18n::t;
use gpui::{Context, Entity, IntoElement, SharedString, Window};
use state::{Playback, Wisp};
use ui::{ActiveTheme as _, MenuItem, Picker, Popovers};

/// 弹层在按钮所属 Popovers 组里的 key。
const KEY: &str = "playback-speed";
const PRESETS: [f32; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

pub(crate) struct SpeedButton {
    playback: Entity<Playback>,
    popovers: Popovers,
}

impl SpeedButton {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let playback = Wisp::global(cx).playback.clone();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        Self {
            playback,
            popovers: Popovers::default(),
        }
    }
}

impl gpui::Render for SpeedButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let speed = self.playback.read(cx).speed();
        let theme = *cx.theme();
        let items: Vec<MenuItem> = PRESETS
            .iter()
            .map(|value| {
                let value = *value;
                MenuItem::new(
                    SharedString::from(format!("speed-{value}")),
                    SharedString::from(if (value - 1.0).abs() < 0.001 {
                        "1.0× 正常".to_owned()
                    } else {
                        format!("{value:.2}×")
                    }),
                )
                .selected((speed - value).abs() < 0.001)
                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.set_speed(value, cx));
                }))
            })
            .collect();

        Picker::icon(KEY, &self.popovers, "icons/gauge.svg")
            .name(t!("speed-control"))
            .tooltip("speed-control")
            .tint(if (speed - 1.0).abs() < 0.001 {
                theme.muted_foreground
            } else {
                theme.primary
            })
            .items(items)
            .into_any_element()
    }
}

