// [wisp] 扫码登录的二维码弹窗（网易云）。二维码内容由 provider 轮询更新：
// 每次提示带 url 与可选状态提示（已扫码待确认），同一个 Modal 就地刷新即可。

use std::rc::Rc;

use gpui::{App, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};
use i18n::t;
use ui::{ActiveTheme as _, Button, Modal, Qr, QrPicture, Text};

type Action = Rc<dyn Fn(&(), &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(crate) struct QrPrompt {
    url: SharedString,
    hint: SharedString,
    cancel: Option<Action>,
}

impl QrPrompt {
    pub(crate) fn new(url: impl Into<SharedString>, hint: impl Into<SharedString>) -> Self {
        Self {
            url: url.into(),
            hint: hint.into(),
            cancel: None,
        }
    }

    pub(crate) fn on_cancel(
        mut self,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.cancel = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for QrPrompt {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self { url, hint, cancel } = self;
        let theme = *cx.theme();
        let dismissed = cancel.clone();

        Modal::new("qr-prompt", t!("login-qr-title"))
            .w(px(380.))
            .close_button()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .py_2()
                    // Qr 自带白色底与静区（模块必须是深色压浅色才能扫），不要再套背景
                    .child(match QrPicture::new(&url) {
                        Some(picture) => {
                            picture.width(px(196.)).rounded(px(12.)).into_any_element()
                        }
                        None => Qr::new(url)
                            .size(px(196.))
                            .rounded(px(12.))
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(hint),
                    ),
            )
            .action(
                Button::new("cancel-qr")
                    .ghost()
                    .label(t!("common-cancel"))
                    .on_click(move |_, window, cx| {
                        if let Some(cancel) = &cancel {
                            cancel(&(), window, cx);
                        }
                    }),
            )
            .on_dismiss(move |_, window, cx| {
                if let Some(cancel) = &dismissed {
                    cancel(&(), window, cx);
                }
            })
    }
}
