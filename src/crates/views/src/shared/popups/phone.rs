// [wisp] 手机号 + 短信验证码登录弹窗（网易云，与 Wisp 现版的「验证码登录」一致）。

use std::rc::Rc;

use gpui::{App, Entity, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};
use i18n::t;
use ui::{ActiveTheme as _, Button, Field, Form, Input, Modal, Text};

type Action = Rc<dyn Fn(&(), &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(crate) struct PhonePrompt {
    /// 状态提示（「验证码已发送」等），由 provider 每次重发提示时更新。
    notice: Option<SharedString>,
    phone: Entity<Input>,
    code: Entity<Input>,
    phone_error: Option<SharedString>,
    code_error: Option<SharedString>,
    cooldown: Option<u32>,
    can_submit: bool,
    send: Option<Action>,
    submit: Option<Action>,
    cancel: Option<Action>,
}

impl PhonePrompt {
    pub(crate) fn new(phone: Entity<Input>, code: Entity<Input>) -> Self {
        Self {
            notice: None,
            phone,
            code,
            phone_error: None,
            code_error: None,
            cooldown: None,
            can_submit: true,
            send: None,
            submit: None,
            cancel: None,
        }
    }

    pub(crate) fn notice(mut self, notice: Option<SharedString>) -> Self {
        self.notice = notice;
        self
    }

    pub(crate) fn errors(
        mut self,
        phone: Option<SharedString>,
        code: Option<SharedString>,
    ) -> Self {
        self.phone_error = phone;
        self.code_error = code;
        self
    }

    pub(crate) fn cooldown(mut self, seconds: Option<u32>) -> Self {
        self.cooldown = seconds;
        self
    }

    pub(crate) fn can_submit(mut self, can_submit: bool) -> Self {
        self.can_submit = can_submit;
        self
    }

    /// 点「发送验证码」时通知调用方（由它把手机号交给 provider）。
    pub(crate) fn on_send(
        mut self,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.send = Some(Rc::new(handler));
        self
    }

    pub(crate) fn on_submit(
        mut self,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.submit = Some(Rc::new(handler));
        self
    }

    pub(crate) fn on_cancel(
        mut self,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.cancel = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for PhonePrompt {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            notice,
            phone,
            code,
            phone_error,
            code_error,
            cooldown,
            can_submit,
            send,
            submit,
            cancel,
        } = self;
        let theme = *cx.theme();
        let (submitted, escaped, dismissed, sent, cancelled) = (
            submit.clone(),
            cancel.clone(),
            cancel.clone(),
            send.clone(),
            cancel.clone(),
        );
        let seconds = cooldown.unwrap_or(0);
        let send_label = match seconds {
            0 => t!("login-phone-send"),
            seconds => t!("login-phone-retry", seconds = seconds),
        };
        let send_button = Button::new("send-phone-code")
            .label(send_label)
            .secondary()
            .disabled(seconds > 0)
            .on_click(move |_, window, cx| {
                if let Some(send) = &sent {
                    send(&(), window, cx);
                }
            });

        Modal::new("phone-prompt", t!("login-phone-title"))
            .w(px(420.))
            .close_button()
            .when_some(notice, |this, notice| {
                this.child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(notice),
                )
            })
            .child(
                Form::new("phone-form")
                    .field(
                        Field::new(&phone, cx)
                            .label(t!("login-phone-number"))
                            .error(phone_error),
                    )
                    .field(
                        Field::new(&code, cx)
                            .label(t!("login-phone-code"))
                            .error(code_error)
                            .end(send_button),
                    )
                    .on_submit(move |_, window, cx| {
                        if let Some(submit) = &submitted {
                            submit(&(), window, cx);
                        }
                    })
                    .on_dismiss(move |_, window, cx| {
                        if let Some(cancel) = &escaped {
                            cancel(&(), window, cx);
                        }
                    }),
            )
            .action(
                Button::new("cancel-phone")
                    .ghost()
                    .label(t!("common-cancel"))
                    .on_click(move |_, window, cx| {
                        if let Some(cancel) = &cancelled {
                            cancel(&(), window, cx);
                        }
                    }),
            )
            .action(
                Button::new("submit-phone")
                    .label(t!("login-phone-submit"))
                    .primary()
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        if let Some(submit) = &submit {
                            submit(&(), window, cx);
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
