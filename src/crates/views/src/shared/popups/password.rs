// [wisp] Kugou account + password sign-in prompt.

use std::rc::Rc;

use gpui::{App, Entity, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};
use i18n::t;
use ui::{ActiveTheme as _, Button, Field, Form, Input, Modal, Text};

type Action = Rc<dyn Fn(&(), &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(crate) struct PasswordPrompt {
    notice: Option<SharedString>,
    username: Entity<Input>,
    password: Entity<Input>,
    username_error: Option<SharedString>,
    password_error: Option<SharedString>,
    submit: Option<Action>,
    cancel: Option<Action>,
}

impl PasswordPrompt {
    pub(crate) fn new(username: Entity<Input>, password: Entity<Input>) -> Self {
        Self {
            notice: None,
            username,
            password,
            username_error: None,
            password_error: None,
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
        username: Option<SharedString>,
        password: Option<SharedString>,
    ) -> Self {
        self.username_error = username;
        self.password_error = password;
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

impl RenderOnce for PasswordPrompt {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            notice,
            username,
            password,
            username_error,
            password_error,
            submit,
            cancel,
        } = self;
        let theme = *cx.theme();
        let (submitted, escaped, dismissed, cancelled) = (
            submit.clone(),
            cancel.clone(),
            cancel.clone(),
            cancel.clone(),
        );

        Modal::new("password-prompt", t!("login-password-title"))
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
                Form::new("password-form")
                    .field(
                        Field::new(&username, cx)
                            .label(t!("login-password-username"))
                            .error(username_error),
                    )
                    .field(
                        Field::new(&password, cx)
                            .label(t!("login-password-password"))
                            .error(password_error),
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
                Button::new("cancel-password")
                    .ghost()
                    .label(t!("common-cancel"))
                    .on_click(move |_, window, cx| {
                        if let Some(cancel) = &cancelled {
                            cancel(&(), window, cx);
                        }
                    }),
            )
            .action(
                Button::new("submit-password")
                    .label(t!("login-password-submit"))
                    .primary()
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
