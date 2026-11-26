use std::time::{Duration, Instant};

use crate::shared::device_code::device_code;
use crate::shared::popups::{AccountPicker, CookiePrompt, PasswordPrompt, PhonePrompt, QrPrompt};
use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnimationExt as _, AnyElement, App, ClipboardItem, Context, Entity, FontWeight, Hsla,
    IntoElement, Pixels, Point, Render, SharedString, Task, Window, div, hsla, point, px, relative,
    rgb, svg,
};
use i18n::t;
use music::{AccountChoices, SignIn, SignInPrompt};
use router::{Destination, SettingsPage, navigate};
use state::{ProviderInfo, Session, SessionState};
use ui::ActiveTheme as _;
use ui::{
    Button, Field, Form, Input, LEADING, Lattice, Modal, Qr, QrPicture, Scrollbar, Scroller,
    Separator, Text, mix,
};

/// How wide the sign-in card stands.
const CARD: Pixels = px(400.);
/// The app icon over the welcome.
const ICON: Pixels = px(64.);
/// A provider's mark in its row.
const MARK: Pixels = px(22.);
/// The space between two provider rows.
const ROW_GAP: Pixels = px(4.);
/// How hard the frosted card blurs the lattice behind it.
const FROST: Pixels = px(28.);
/// How tall the bands are where the provider list fades out at its ends when the window is
/// too short to show it whole.
const FADE: Pixels = px(16.);
/// How wide a sign-in choice or code dialog stands, as on the settings page.
const DIALOG: Pixels = px(420.);
/// The provider method slider: equal segments inside the card's content width.
const LOGIN_METHOD_H: f32 = 34.;
const LOGIN_METHOD_PAD: f32 = 3.;
const LOGIN_METHOD_GAP: f32 = 2.;
const LOGIN_METHOD_TRACK_W: f32 = 336.;
const LOGIN_METHOD_ICON: f32 = 16.;
/// How many 120 Hz ticks pass between frames while the pools only drift, which makes 30 Hz, and
/// while a glow fades in or out, which makes 60 Hz.
const DRIFT_TICKS: u32 = 4;
const FADE_TICKS: u32 = 2;
/// How far the glow's strength and colour move toward their targets each frame.
const EASE: f32 = 0.3;
/// How close the glow has to come to its target before it counts as settled.
const SETTLED: f32 = 0.01;
/// The pools that light the lattice: indigo, violet and a cool teal, so no provider's brand
/// colour owns the backdrop. The glow behind a hovered provider takes that provider's brand.
const POOLS: [(f32, f32, f32, f32); 3] = [
    (0.66, 0.75, 0.62, 0.95),
    (0.78, 0.70, 0.64, 0.85),
    (0.50, 0.65, 0.52, 0.7),
];

pub struct LoginView {
    session: Entity<Session>,
    secret: Entity<Input>,
    /// [wisp] 手机号验证码登录用的两个输入框。
    phone: Entity<Input>,
    code: Entity<Input>,
    phone_error: Option<SharedString>,
    code_error: Option<SharedString>,
    /// 已经请求过验证码的手机号；当前输入与它不同时不能提交。
    phone_sent_to: Option<String>,
    /// 距离可以重新发送验证码还剩多少秒。
    phone_cooldown: Option<u32>,
    cooldown_task: Option<Task<()>>,
    /// [wisp] 酷狗账号密码登录的两个输入框。
    username: Entity<Input>,
    password: Entity<Input>,
    username_error: Option<SharedString>,
    password_error: Option<SharedString>,
    /// [wisp] provider 二次安全验证的验证码。
    verify: Entity<Input>,
    manual_secret: Option<(SharedString, SharedString)>,
    /// The provider under the pointer, whose brand the glow behind it takes.
    hovered: Option<SharedString>,
    /// Where the glow sits: behind the middle of the card at the height the pointer entered
    /// the hovered row. It stays put while the glow fades out.
    glow_at: Option<Point<Pixels>>,
    glow: f32,
    lamp: Hsla,
    started: Instant,
    pacing: Option<Task<()>>,
    scrollbar: Entity<Scrollbar>,
    /// The method the last click left, and the frame key that glides the highlight across.
    login_slide_from: usize,
    login_slide_seq: u64,
}

impl LoginView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        let phone = cx.new(|cx| Input::new("login-phone-number", cx));
        let code = cx.new(|cx| Input::new("login-phone-code", cx));
        let username = cx.new(|cx| Input::new("login-password-username", cx));
        let password = cx.new(|cx| Input::new("login-password-password", cx).masked());
        let verify = cx.new(|cx| Input::new("login-verify-code", cx));
        cx.observe(&phone, |this, _, cx| this.phone_changed(cx))
            .detach();
        cx.observe(&code, |this, _, cx| this.code_changed(cx))
            .detach();
        cx.observe(&username, |this, _, cx| this.username_changed(cx))
            .detach();
        cx.observe(&password, |this, _, cx| this.password_changed(cx))
            .detach();
        Self {
            session,
            secret: cx.new(|cx| Input::new("login-cookie-hint", cx)),
            phone,
            code,
            phone_error: None,
            code_error: None,
            phone_sent_to: None,
            phone_cooldown: None,
            cooldown_task: None,
            username,
            password,
            username_error: None,
            password_error: None,
            verify,
            manual_secret: None,
            hovered: None,
            glow_at: None,
            glow: 0.,
            lamp: cx.theme().foreground,
            started: Instant::now(),
            pacing: None,
            scrollbar: cx.new(|_| Scrollbar::inset()),
            login_slide_from: 0,
            login_slide_seq: 0,
        }
    }

    fn acted(&self, _cx: &mut Context<Self>) {}

    fn abandon(&mut self, cx: &mut Context<Self>) {
        self.acted(cx);
        self.clear_secret(cx);
        self.reset_phone(cx);
        self.reset_password(cx);
        self.session
            .update(cx, |session, cx| session.cancel_sign_in(cx));
    }

    /// Closes a QR or phone prompt but stays on the sign-in page, so the user can pick another
    /// method instead of being dropped back into the app.
    fn dismiss_prompt(&mut self, cx: &mut Context<Self>) {
        self.reset_phone(cx);
        self.session
            .update(cx, |session, cx| session.dismiss_sign_in(cx));
    }

    fn clear_secret(&mut self, cx: &mut Context<Self>) {
        self.manual_secret = None;
        self.secret.update(cx, |input, cx| input.set_text("", cx));
    }

    fn reset_phone(&mut self, cx: &mut Context<Self>) {
        self.phone_error = None;
        self.code_error = None;
        self.phone_sent_to = None;
        self.phone_cooldown = None;
        self.cooldown_task = None;
        self.phone
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.code
            .update(cx, |input, cx| input.set_invalid(false, cx));
        cx.notify();
    }

    fn reset_password(&mut self, cx: &mut Context<Self>) {
        self.username_error = None;
        self.password_error = None;
        self.username
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.password
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.password.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn phone_changed(&mut self, cx: &mut Context<Self>) {
        if self
            .phone_sent_to
            .as_deref()
            .is_some_and(|sent| sent != self.phone.read(cx).text().trim())
        {
            self.phone_sent_to = None;
        }
        if self.phone_error.take().is_some() {
            let phone = self.phone.clone();
            cx.defer(move |cx| {
                phone.update(cx, |input, cx| input.set_invalid(false, cx));
            });
        }
        cx.notify();
    }

    fn code_changed(&mut self, cx: &mut Context<Self>) {
        if self.code_error.take().is_some() {
            let code = self.code.clone();
            cx.defer(move |cx| {
                code.update(cx, |input, cx| input.set_invalid(false, cx));
            });
        }
        cx.notify();
    }

    fn username_changed(&mut self, cx: &mut Context<Self>) {
        if self.username_error.take().is_some() {
            let username = self.username.clone();
            cx.defer(move |cx| {
                username.update(cx, |input, cx| input.set_invalid(false, cx));
            });
        }
        cx.notify();
    }

    fn password_changed(&mut self, cx: &mut Context<Self>) {
        if self.password_error.take().is_some() {
            let password = self.password.clone();
            cx.defer(move |cx| {
                password.update(cx, |input, cx| input.set_invalid(false, cx));
            });
        }
        cx.notify();
    }

    /// [wisp] Submits `账号\n密码`, the input the plugin's password flow parses.
    fn submit_password(&mut self, cx: &mut Context<Self>) {
        let username = self.username.read(cx).text().trim().to_string();
        let password = self.password.read(cx).text().to_string();
        let username_ok = !username.is_empty();
        let password_ok = !password.is_empty();
        if !username_ok || !password_ok {
            self.username_error =
                (!username_ok).then(|| t!("login-password-username-empty").into());
            self.password_error =
                (!password_ok).then(|| t!("login-password-password-empty").into());
            self.username
                .update(cx, |input, cx| input.set_invalid(!username_ok, cx));
            self.password
                .update(cx, |input, cx| input.set_invalid(!password_ok, cx));
            cx.notify();
            return;
        }
        self.username_error = None;
        self.password_error = None;
        self.username
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.password
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.acted(cx);
        self.session.update(cx, |session, cx| {
            session.submit_input(format!("{username}\n{password}"), cx)
        });
    }

    fn phone_ready(&self, cx: &App) -> bool {
        let phone = self.phone.read(cx).text();
        self.phone_sent_to.as_deref() == Some(phone.trim())
            && valid_phone(phone)
            && valid_code(self.code.read(cx).text())
    }

    fn start_phone_cooldown(&mut self, cx: &mut Context<Self>) {
        self.phone_cooldown = Some(60);
        cx.notify();
        self.cooldown_task = Some(cx.spawn(async move |this, cx| {
            for remaining in (0..60).rev() {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |this, cx| {
                        this.phone_cooldown = (remaining > 0).then_some(remaining);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    fn start(&mut self, slug: &str, method: SignIn, cx: &mut Context<Self>) {
        self.acted(cx);
        self.manual_secret = None;
        self.session
            .update(cx, |session, cx| session.sign_in(slug, method, cx));
    }

    fn start_manual(&mut self, slug: &str, provider: SharedString, cx: &mut Context<Self>) {
        self.acted(cx);
        self.manual_secret = Some((slug.into(), provider));
        let hint = CookiePrompt::hint();
        self.secret.update(cx, |input, cx| input.set_hint(hint, cx));
        self.session
            .update(cx, |session, cx| session.sign_in_with_cookies(slug, cx));
    }

    fn submit_secret(&mut self, cx: &mut Context<Self>) {
        let text = self.secret.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.acted(cx);
        self.clear_secret(cx);
        self.session
            .update(cx, |session, cx| session.submit_input(text, cx));
    }

    /// [wisp] 请求发送短信验证码（provider 侧用 `send:` 前缀识别）。
    fn send_phone_code(&mut self, cx: &mut Context<Self>) {
        let phone = self.phone.read(cx).text().trim().to_string();
        if !valid_phone(&phone) {
            self.phone_error = Some(t!("login-phone-invalid").into());
            self.phone
                .update(cx, |input, cx| input.set_invalid(true, cx));
            cx.notify();
            return;
        }
        if self.phone_cooldown.is_some() {
            return;
        }
        self.phone_error = None;
        self.phone_sent_to = Some(phone.clone());
        self.phone
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.start_phone_cooldown(cx);
        self.acted(cx);
        self.session.update(cx, |session, cx| {
            session.submit_input(format!("send:{phone}"), cx)
        });
    }

    /// [wisp] 提交手机号 + 验证码（`login:<手机号>\n<验证码>`）。
    fn submit_phone(&mut self, cx: &mut Context<Self>) {
        let phone = self.phone.read(cx).text().trim().to_string();
        let code = self.code.read(cx).text().trim().to_string();
        let phone_ok = valid_phone(&phone);
        let sent = self.phone_sent_to.as_deref() == Some(phone.as_str());
        let code_ok = valid_code(&code);
        if !phone_ok || !sent || !code_ok {
            self.phone_error = (!phone_ok).then(|| t!("login-phone-invalid").into());
            self.code_error = if !sent {
                Some(t!("login-phone-request-code").into())
            } else {
                (!code_ok).then(|| t!("login-phone-code-invalid").into())
            };
            self.phone
                .update(cx, |input, cx| input.set_invalid(!phone_ok, cx));
            self.code
                .update(cx, |input, cx| input.set_invalid(!sent || !code_ok, cx));
            cx.notify();
            return;
        }
        self.phone_error = None;
        self.code_error = None;
        self.phone
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.code
            .update(cx, |input, cx| input.set_invalid(false, cx));
        self.acted(cx);
        self.session.update(cx, |session, cx| {
            session.submit_input(format!("login:{phone}\n{code}"), cx)
        });
    }

    /// Starts a sign-in the way the provider asks for it. The provider's card then lets the user
    /// switch methods from its own slider; there is no method-choice dialog.
    fn pick(&mut self, slug: &str, cx: &mut Context<Self>) {
        let Some(info) = self
            .session
            .read(cx)
            .providers()
            .find(|info| info.slug.as_ref() == slug)
        else {
            return;
        };
        let methods: Vec<SignIn> = info.options.into_iter().filter(offered).collect();
        match methods.first() {
            Some(SignIn::Secret) if !info.web_sign_in => {
                self.start_manual(slug, info.name.clone(), cx)
            }
            Some(method) => self.start(slug, method.clone(), cx),
            None => {}
        }
    }

    /// Lights the glow behind the card at the height the pointer came into the row, in the
    /// provider's brand, and lets it fade once the pointer leaves.
    fn hover(&mut self, slug: &str, inside: bool, window: &Window, cx: &mut Context<Self>) {
        match inside {
            true => {
                let entered = window.mouse_position();
                self.hovered = Some(slug.into());
                self.glow_at = Some(point(window.viewport_size().width / 2., entered.y));
            }
            false if self.hovered.as_deref() == Some(slug) => self.hovered = None,
            false => {}
        }
        cx.notify();
    }

    /// Eases the glow one step toward its targets and asks for the lattice's next frame on the
    /// shared tick, faster while the glow is still moving.
    fn pace(&mut self, lamp: Hsla, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.hovered {
            Some(_) => 1.,
            None => 0.,
        };
        self.glow += (target - self.glow) * EASE;
        if self.hovered.is_some() {
            self.lamp = mix(self.lamp, lamp, EASE);
        }
        let fading = (target - self.glow).abs() > SETTLED;
        if self.pacing.is_some() {
            return;
        }
        let ticks = match fading {
            true => FADE_TICKS,
            false => DRIFT_TICKS,
        };
        let timer = crate::shared::animation::next(ticks, cx);
        let handle = window.window_handle();
        self.pacing = Some(cx.spawn(async move |this, cx| {
            timer.await;
            if this
                .update(cx, |this, cx| {
                    this.pacing = None;
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            handle
                .update(cx, |_, window, _| window.on_next_frame(|_, _| {}))
                .ok();
        }));
    }

    /// The Wisp icon, the welcome and the line under it saying what the screen is waiting on.
    fn welcome(&self, status: SharedString, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let title = t!("login-welcome-title");
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(crate::brand::icon(ICON).mb_2())
            .child(
                div()
                    .id("login-title")
                    .role(Role::Heading)
                    .aria_level(1)
                    .aria_label(title.clone())
                    .text_size(theme.text(Text::Title))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .id("login-status")
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .text_center()
                    .line_height(relative(LEADING))
                    .text_size(theme.text(Text::Body))
                    .text_color(theme.muted_foreground)
                    .child(status),
            )
    }

    /// The providers as plain ghost buttons the width of the card: the mark, muted until the
    /// pointer brings out its brand, the name, and what a click does. The list scrolls inside
    /// the card when the window is too short for it and fades at whichever end has more.
    fn rows(
        &self,
        providers: &[ProviderInfo],
        pending: bool,
        waiting: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let count = providers.len();
        let rows = providers.iter().enumerate().map(|(index, info)| {
            let slug = info.slug.clone();
            let busy = waiting && info.pending;
            let group = SharedString::from(format!("login-row-{slug}"));
            let brand = crate::shared::provider_brand(slug.as_ref(), cx);
            let mark = svg()
                .path(icons::path(crate::shared::provider_logo(slug.as_ref(), cx)))
                .size(MARK)
                .flex_none()
                .text_color(theme.muted_foreground)
                .group_hover(group.clone(), move |style| style.text_color(brand));
            let detail = match busy {
                true => t!("common-cancel"),
                false => t!("settings-provider-sign-in"),
            };
            Button::new(SharedString::from(format!("login-{slug}")))
                .ghost()
                .group(group)
                .leading(mark)
                .label(info.name.clone())
                .detail(detail)
                .trailing("icons/chevron-right.svg")
                .h(theme.metrics.list_row)
                .mx_neg_3()
                .px_3()
                .gap_3()
                .disabled(pending && !busy)
                .aria_position_in_set(index + 1)
                .aria_size_of_set(count)
                .on_hover({
                    let slug = slug.clone();
                    cx.listener(move |this, inside: &bool, window, cx| {
                        this.hover(slug.as_ref(), *inside, window, cx)
                    })
                })
                .on_click(cx.listener(move |this, _, _, cx| match busy {
                    true => this.abandon(cx),
                    false => this.pick(slug.as_ref(), cx),
                }))
                .into_any_element()
        });
        // The list stands as tall as its rows, a definite height the scroller can fill, and gives
        // way from there when the card runs out of room.
        let tall = theme.metrics.list_row * count as f32 + ROW_GAP * count.saturating_sub(1) as f32;
        div()
            .id("login-providers")
            .role(Role::List)
            .aria_label(t!("login-providers"))
            .flex()
            .flex_col()
            .h(tall)
            .flex_shrink(1.)
            .min_h_0()
            .child(
                Scroller::new("login-providers-scroll", &self.scrollbar)
                    .fade_ends(FADE)
                    .flex()
                    .flex_col()
                    .gap(ROW_GAP)
                    .px_3()
                    .children(rows),
            )
            .into_any_element()
    }

    /// A rule with "or" in its middle, between the providers and guest mode.
    fn divider(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .child(div().flex_1().child(Separator::horizontal()))
            .child(
                div()
                    .text_size(theme.text(Text::Tiny))
                    .text_color(theme.muted_foreground)
                    .child(t!("login-or")),
            )
            .child(div().flex_1().child(Separator::horizontal()))
    }

    fn guest_mode(&self, slug: &SharedString, pending: bool, cx: &mut Context<Self>) -> Button {
        Button::new("guest-mode")
            .label(t!("login-guest-use"))
            .icon("icons/hat-glasses.svg")
            .outline()
            .w_full()
            .disabled(pending)
            .on_click({
                let slug = slug.clone();
                cx.listener(move |this, _, _, cx| {
                    this.start(slug.as_ref(), SignIn::Anonymous, cx);
                })
            })
    }

    /// Plays only the music folders, without any streaming account. With no folder chosen yet
    /// it opens the music folders settings so one can be added.
    fn offline(&self, pending: bool, cx: &mut Context<Self>) -> Button {
        Button::new("login-offline")
            .label(t!("login-offline-use"))
            .icon(crate::shared::blend::LOCAL_MARK)
            .outline()
            .w_full()
            .disabled(pending)
            .on_click(cx.listener(|this, _, _, cx| {
                let empty = this.session.read(cx).local_paths().is_empty();
                this.session.update(cx, |session, cx| session.use_local(cx));
                if empty {
                    navigate(Destination::Settings(SettingsPage::MusicFolders), cx);
                }
            }))
    }

    fn url_prompt(&self, url: String) -> impl IntoElement {
        Button::new("copy-login-url")
            .icon("icons/copy.svg")
            .label(t!("menu-copy-link"))
            .outline()
            .small()
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
            })
    }

    /// The code a device sign-in is waiting on. Closing the dialog cancels the sign-in.
    fn code_prompt(&self, code: String, url: String, cx: &mut Context<Self>) -> impl IntoElement {
        Modal::new("login-code-prompt", t!("login-device-code", url = &url))
            .w(DIALOG)
            .close_button()
            .detail(t!("login-authorizing"))
            .child(device_code(code, cx))
            .action(
                Button::new("login-cancel-code")
                    .ghost()
                    .label(t!("common-cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.abandon(cx))),
            )
            .on_dismiss(cx.listener(|this, _, _, cx| this.abandon(cx)))
    }

    fn account_modal(&self, accounts: AccountChoices, cx: &mut Context<Self>) -> impl IntoElement {
        AccountPicker::new(accounts)
            .on_pick(cx.listener(|this, id: &SharedString, _, cx| {
                this.acted(cx);
                let id = id.to_string();
                this.session
                    .update(cx, |session, cx| session.submit_input(id, cx));
            }))
            .on_cancel(cx.listener(|this, _, _, cx| this.abandon(cx)))
    }

    fn verify_prompt(&self, kind: String, context: String, cx: &mut Context<Self>) -> AnyElement {
        let detail = verify_detail(&kind, &context);
        Modal::new("login-verify-prompt", t!("login-verify-title"))
            .w(DIALOG)
            .close_button()
            .detail(detail)
            .child(
                Form::new("login-verify-form")
                    .field(Field::new(&self.verify, cx).label(t!("login-verify-code")))
                    .on_submit(cx.listener(|this, _: &(), _, cx| {
                        this.submit_verify(cx);
                    })),
            )
            .action(
                Button::new("login-cancel-verify")
                    .ghost()
                    .label(t!("common-cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_prompt(cx))),
            )
            .action(
                Button::new("login-submit-verify")
                    .label(t!("login-verify-submit"))
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| this.submit_verify(cx))),
            )
            .on_dismiss(cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)))
            .into_any_element()
    }

    fn submit_verify(&mut self, cx: &mut Context<Self>) {
        let code = self.verify.read(cx).text().trim().to_owned();
        if code.is_empty() {
            return;
        }
        self.verify.update(cx, |input, cx| input.set_text("", cx));
        self.session
            .update(cx, |session, cx| session.submit_input(code, cx));
    }

    fn secret_prompt(&self, provider: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        CookiePrompt::new(provider.clone(), self.secret.clone())
            .on_submit(cx.listener(|this, _, _, cx| this.submit_secret(cx)))
            .on_cancel(cx.listener(|this, _, _, cx| this.abandon(cx)))
    }

    /// [wisp] 酷狗登录页：EchoMusic 式的一张卡，内容随底部方式切换，不列出其他服务。
    fn provider_page(
        &mut self,
        slug: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let _ = window;
        let theme = *cx.theme();
        let (name, options) = self
            .session
            .read(cx)
            .providers()
            .find(|info| info.slug.as_ref() == slug)
            .map(|info| (info.name, info.options))
            .unwrap_or_else(|| (slug.to_owned().into(), Vec::new()));
        let methods = card_methods(&options);
        let method = self
            .session
            .read(cx)
            .login_method()
            .cloned()
            .filter(|method| methods.contains(method))
            .unwrap_or_else(|| methods[0].clone());
        let state = self.session.read(cx).state().clone();
        let verify = match &state {
            SessionState::Authorizing(Some(SignInPrompt::Verify { kind, context })) => {
                Some((kind.clone(), context.clone()))
            }
            _ => None,
        };
        let content = self.provider_content(slug, name.as_ref(), &state, &method, cx);
        let active = methods
            .iter()
            .position(|candidate| candidate == &method)
            .unwrap_or(0);
        let from = match self.login_slide_seq {
            0 => active,
            _ => self.login_slide_from.min(methods.len().saturating_sub(1)),
        };
        let tabs = self.provider_tabs(slug, &methods, active, from, cx);

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let radius = crate::chrome::window_radius(Wisp::global(cx).settings.read(cx), cx, window);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let radius: Option<Pixels> = None;

        let card = div()
            .flex()
            .flex_col()
            .gap_5()
            .w(CARD)
            .p_8()
            .rounded(ui::corners(theme.radius))
            .border_1()
            .border_color(theme.border.opacity(0.7))
            .bg(theme.background.opacity(0.72))
            .backdrop_blur(FROST)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        svg()
                            .path(icons::path(crate::shared::provider_logo(slug, cx)))
                            .size(px(40.))
                            .text_color(crate::shared::provider_brand(slug, cx)),
                    )
                    .child(
                        div()
                            .text_size(theme.text(Text::Title))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(match slug {
                                "kugou" => t!("login-kugou-title"),
                                _ => t!("login-provider-title", provider = name.as_ref()),
                            }),
                    )
                    .child(
                        div()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(match slug {
                                "kugou" => t!("login-kugou-subtitle"),
                                _ => t!("login-provider-subtitle", provider = name.as_ref()),
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .w_full()
                    .min_h(px(312.))
                    .child(content),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .w_full()
                    .border_t_1()
                    .border_color(theme.border.opacity(0.6))
                    .pt_4()
                    .child(tabs),
            );

        let backdrop = Lattice::new(POOLS.map(|(h, s, l, a)| hsla(h, s, l, a)))
            .stage(Hsla {
                a: 1.,
                ..theme.background
            })
            .lamp(theme.foreground)
            .glow(None, 0.)
            .time(0.)
            .when_some(radius, |this, radius| this.corner_radius(radius));

        div()
            .id(SharedString::from(format!("provider-login-{slug}")))
            .relative()
            .size_full()
            .overflow_hidden()
            .when_some(radius, |this, radius| this.rounded(radius))
            .child(div().absolute().inset_0().child(backdrop))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .p(theme.metrics.pad)
                    .pt(theme.metrics.title_bar)
                    .child(card),
            )
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .top(theme.metrics.title_bar + px(8.))
                    .child(
                        Button::new("login-card-back")
                            .ghost()
                            .leading(
                                svg()
                                    .path(icons::path("icons/chevron-left.svg"))
                                    .size(px(16.))
                                    .text_color(theme.muted_foreground)
                                    .into_any_element(),
                            )
                            .label(t!("login-kugou-back"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.session
                                    .update(cx, |session, cx| session.back_to_login(cx));
                            })),
                    ),
            )
            .when_some(verify, |this, (kind, context)| {
                this.child(self.verify_prompt(kind, context, cx))
            })
            .into_any_element()
    }

    fn provider_tabs(
        &self,
        slug: &str,
        methods: &[SignIn],
        active: usize,
        from: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let count = methods.len().max(1) as f32;
        let seg_w =
            (LOGIN_METHOD_TRACK_W - 2. * LOGIN_METHOD_PAD - (count - 1.) * LOGIN_METHOD_GAP)
                / count;
        let from_left = LOGIN_METHOD_PAD + from as f32 * (seg_w + LOGIN_METHOD_GAP);
        let to_left = LOGIN_METHOD_PAD + active as f32 * (seg_w + LOGIN_METHOD_GAP);
        let accent = provider_method_accent(slug, methods.get(active).unwrap_or(&SignIn::Qr), cx);
        let bar = div()
            .absolute()
            .top(px(LOGIN_METHOD_PAD))
            .left(px(from_left))
            .w(px(seg_w))
            .h(px(LOGIN_METHOD_H - 2. * LOGIN_METHOD_PAD))
            .rounded(px(8.))
            .bg(accent.opacity(0.16))
            .border_1()
            .border_color(accent.opacity(0.42));
        let indicator = if ui::motion::animates(cx) && from != active {
            bar.with_animation(
                SharedString::from(format!(
                    "login-method-slide-{slug}-{}",
                    self.login_slide_seq
                )),
                ui::motion::Motion::Base.animation(),
                move |bar, t| bar.left(px(from_left + (to_left - from_left) * t)),
            )
            .into_any_element()
        } else {
            bar.left(px(to_left)).into_any_element()
        };
        div()
            .relative()
            .flex()
            .items_center()
            .gap(px(LOGIN_METHOD_GAP))
            .p(px(LOGIN_METHOD_PAD))
            .rounded(px(12.))
            .bg(theme.secondary.opacity(0.9))
            .border_1()
            .border_color(theme.border.opacity(0.8))
            .child(indicator)
            .children(
                methods.iter().enumerate().map(|(index, method)| {
                    self.provider_tab(slug, index, method, active, seg_w, cx)
                }),
            )
            .into_any_element()
    }

    fn provider_tab(
        &self,
        slug: &str,
        index: usize,
        method: &SignIn,
        active: usize,
        seg_w: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let selected = index == active;
        let (label, icon) = match method {
            SignIn::Qr if slug == "kugou" => {
                (t!("login-kugou-method-kugou"), "icons/login-qrcode.svg")
            }
            SignIn::Qr => (t!("login-sign-in-qr"), "icons/login-qrcode.svg"),
            SignIn::Phone => (t!("login-kugou-method-phone"), "icons/login-phone.svg"),
            SignIn::Qq => (t!("login-kugou-method-qq"), "icons/brand-qq.svg"),
            _ => (t!("login-kugou-method-wechat"), "icons/brand-wechat.svg"),
        };
        let accent = provider_method_accent(slug, method, cx);
        let chosen = method.clone();
        let owner = SharedString::from(slug);
        div()
            .id(SharedString::from(format!("{slug}-method-{index}")))
            .role(Role::Button)
            .aria_label(label.clone())
            .when(selected, |this| this.aria_selected(true))
            .w(px(seg_w))
            .h(px(LOGIN_METHOD_H - 2. * LOGIN_METHOD_PAD))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .text_size(theme.text(Text::Small))
            .font_weight(FontWeight::MEDIUM)
            .text_color(match selected {
                true => theme.foreground,
                false => theme.muted_foreground,
            })
            .cursor_pointer()
            .when(!selected, |this| {
                this.hover(move |style| style.bg(theme.foreground.opacity(0.05)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.provider_choose(owner.as_ref(), chosen.clone(), cx)
            }))
            .child(
                svg()
                    .path(icons::path(icon))
                    .size(px(LOGIN_METHOD_ICON))
                    .text_color(match selected {
                        true => accent,
                        false => theme.muted_foreground,
                    }),
            )
            .child(label)
            .into_any_element()
    }

    fn provider_choose(&mut self, slug: &str, method: SignIn, cx: &mut Context<Self>) {
        let current = self.session.read(cx).login_method().cloned();
        let same = current.as_ref() == Some(&method);
        let refreshes = matches!(method, SignIn::Qr | SignIn::Qq | SignIn::WeChat);
        if same && !refreshes {
            return;
        }
        let methods = self
            .session
            .read(cx)
            .providers()
            .find(|info| info.slug.as_ref() == slug)
            .map(|info| card_methods(&info.options))
            .unwrap_or_default();
        let from = current
            .as_ref()
            .and_then(|current| methods.iter().position(|method| method == current))
            .unwrap_or(0);
        let to = methods
            .iter()
            .position(|candidate| candidate == &method)
            .unwrap_or(from);
        if from != to {
            self.login_slide_from = from;
            self.login_slide_seq = self.login_slide_seq.wrapping_add(1);
        }
        self.phone_error = None;
        self.code_error = None;
        self.username_error = None;
        self.password_error = None;
        self.reset_phone(cx);
        self.reset_password(cx);
        let slug = slug.to_owned();
        self.session
            .update(cx, |session, cx| session.switch_sign_in(&slug, method, cx));
    }

    fn provider_content(
        &mut self,
        slug: &str,
        name: &str,
        state: &SessionState,
        method: &SignIn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        match state {
            SessionState::Authorizing(Some(SignInPrompt::Qr { url, hint }))
                if matches!(method, SignIn::Qr | SignIn::Qq | SignIn::WeChat) =>
            {
                self.provider_qr(slug, name, url.clone(), hint.clone(), method, cx)
            }
            SessionState::Authorizing(Some(SignInPrompt::Phone { notice })) => {
                self.provider_phone(slug, notice.clone(), cx)
            }
            SessionState::Failed(failure) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.danger)
                        .text_center()
                        .max_w(px(300.))
                        .child(failure.summary.clone()),
                )
                .child(
                    Button::new(SharedString::from(format!("{slug}-login-retry")))
                        .label(t!("login-kugou-retry"))
                        .secondary()
                        .on_click(cx.listener({
                            let owner = SharedString::from(slug);
                            let method = method.clone();
                            move |this, _, _, cx| {
                                this.provider_choose(owner.as_ref(), method.clone(), cx)
                            }
                        })),
                )
                .into_any_element(),
            _ => div()
                .text_size(theme.text(Text::Small))
                .text_color(theme.muted_foreground)
                .child(t!("login-provider-connecting", provider = name))
                .into_any_element(),
        }
    }

    fn provider_qr(
        &mut self,
        slug: &str,
        name: &str,
        url: String,
        hint: Option<String>,
        method: &SignIn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let hint = self.provider_hint(slug, name, method, hint);
        let owner = SharedString::from(slug);
        let chosen = method.clone();
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(match QrPicture::new(&url) {
                Some(picture) => picture.width(px(184.)).rounded(px(12.)).into_any_element(),
                None => Qr::new(url)
                    .size(px(184.))
                    .rounded(px(12.))
                    .into_any_element(),
            })
            .child(
                div()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .child(hint),
            )
            .child(
                Button::new(SharedString::from(format!("{slug}-login-refresh")))
                    .ghost()
                    .small()
                    .label(t!("login-kugou-refresh"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.provider_choose(owner.as_ref(), chosen.clone(), cx)
                    })),
            )
            .into_any_element()
    }

    fn provider_hint(
        &self,
        slug: &str,
        name: &str,
        method: &SignIn,
        hint: Option<String>,
    ) -> SharedString {
        let confirming = hint
            .as_deref()
            .is_some_and(|hint| hint.to_ascii_lowercase().contains("confirm"));
        match (confirming, method, slug) {
            (true, _, _) => t!("login-kugou-qr-confirm").to_string().into(),
            (false, SignIn::Qq, _) => t!("login-kugou-qr-qq").to_string().into(),
            (false, SignIn::WeChat, _) => t!("login-kugou-qr-wechat").to_string().into(),
            (false, _, "kugou") => t!("login-kugou-qr-kugou").to_string().into(),
            (false, _, "netease") => t!("login-qr-scan").to_string().into(),
            _ => t!("login-provider-qr-scan", provider = name)
                .to_string()
                .into(),
        }
    }

    fn provider_phone(
        &self,
        slug: &str,
        _notice: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let notice = match self.phone_sent_to.is_some() {
            true => t!("login-phone-sent"),
            false => t!("login-kugou-phone-hint"),
        };
        let seconds = self.phone_cooldown.unwrap_or(0);
        let send_label = match seconds {
            0 => t!("login-phone-send"),
            seconds => t!("login-phone-retry", seconds = seconds),
        };
        let send = Button::new(SharedString::from(format!("{slug}-send-phone-code")))
            .label(send_label)
            .secondary()
            .disabled(seconds > 0)
            .on_click(cx.listener(|this, _, _, cx| this.send_phone_code(cx)));
        let can_submit = self.phone_ready(cx);
        let form = Form::new(SharedString::from(format!("{slug}-phone-form")))
            .field(
                Field::new(&self.phone, cx)
                    .label(t!("login-phone-number"))
                    .error(self.phone_error.clone()),
            )
            .field(
                Field::new(&self.code, cx)
                    .label(t!("login-phone-code"))
                    .error(self.code_error.clone())
                    .end(send),
            )
            .on_submit(cx.listener(|this, _: &(), _, cx| this.submit_phone(cx)));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(
                div()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .child(notice),
            )
            .child(form)
            .child(
                Button::new(SharedString::from(format!("{slug}-submit-phone")))
                    .label(t!("login-phone-submit"))
                    .primary()
                    .large()
                    .w_full()
                    .disabled(!can_submit)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_phone(cx))),
            )
            .into_any_element()
    }
}

impl Render for LoginView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // [wisp] 扫码/手机号类的 provider 用同一张卡：底部滑块切方式，不铺服务列表，也不
        // 再弹方式选择框。其它登录方式仍走下面的通用页。
        let card = {
            let session = self.session.read(cx);
            session.login_slug().and_then(|slug| {
                let methods = session
                    .providers()
                    .find(|info| info.slug.as_ref() == slug)
                    .map(|info| card_methods(&info.options))
                    .unwrap_or_default();
                session
                    .login_method()
                    .filter(|method| methods.contains(method))
                    .map(|_| slug.to_owned())
            })
        };
        if let Some(slug) = card {
            return self.provider_page(&slug, window, cx).into_any_element();
        }
        let state = self.session.read(cx).state().clone();
        let pending = self.session.read(cx).is_pending();
        let providers: Vec<ProviderInfo> = self.session.read(cx).providers().collect();
        let guest = providers
            .iter()
            .filter(|info| {
                info.options
                    .iter()
                    .any(|option| matches!(option, SignIn::Anonymous))
            })
            .map(|info| info.slug.clone())
            .next();
        let manual_secret = self.manual_secret.clone().filter(|_| {
            matches!(
                &state,
                SessionState::Authorizing(Some(SignInPrompt::Secret))
            )
        });
        let waiting = match &state {
            SessionState::Authorizing(prompt) => {
                manual_secret.is_none() && !matches!(prompt, Some(SignInPrompt::Accounts(_)))
            }
            _ => false,
        };
        let status = match &state {
            SessionState::SignedOut
            | SessionState::Offline(_)
            | SessionState::Failed(_)
            | SessionState::Authorizing(Some(SignInPrompt::Accounts(_))) => {
                t!("login-welcome-detail")
            }
            SessionState::Restoring => t!("login-restoring"),
            SessionState::Authorizing(_) => t!("login-authorizing"),
            SessionState::SignedIn(profile) => t!("login-signed-in", name = &profile.display_name),
        };
        let prompt = match &state {
            SessionState::Authorizing(prompt) => prompt.clone(),
            _ => None,
        };
        let accounts = match &prompt {
            Some(SignInPrompt::Accounts(accounts)) => Some(accounts.clone()),
            _ => None,
        };
        let code = match &prompt {
            Some(SignInPrompt::Code { code, url }) => Some((code.clone(), url.clone())),
            _ => None,
        };
        let url = match prompt {
            Some(SignInPrompt::Url(url)) => Some(url),
            _ => None,
        };
        // [wisp] 扫码 / 手机号登录
        let qr = match &state {
            SessionState::Authorizing(Some(SignInPrompt::Qr { url, hint })) => {
                Some((url.clone(), hint.clone()))
            }
            _ => None,
        };
        let phone_prompt = match &state {
            SessionState::Authorizing(Some(SignInPrompt::Phone { notice })) => Some(notice.clone()),
            _ => None,
        };
        let password_prompt = match &state {
            SessionState::Authorizing(Some(SignInPrompt::Password { notice })) => {
                Some(notice.clone())
            }
            _ => None,
        };
        let verify_prompt = match &state {
            SessionState::Authorizing(Some(SignInPrompt::Verify { kind, context })) => {
                Some((kind.clone(), context.clone()))
            }
            _ => None,
        };

        let failure = providers.iter().find_map(|info| info.error.clone());

        let theme = *cx.theme();
        let lamp = match &self.hovered {
            Some(slug) => crate::shared::provider_brand(slug.as_ref(), cx),
            None => self.lamp,
        };
        let animates = ui::motion::animates(cx);
        match animates {
            true => self.pace(lamp, window, cx),
            false => {
                self.glow = f32::from(u8::from(self.hovered.is_some()));
                self.lamp = lamp;
            }
        }
        let time = match animates {
            true => self.started.elapsed().as_secs_f32(),
            false => 0.,
        };

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let radius = crate::chrome::window_radius(Wisp::global(cx).settings.read(cx), cx, window);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let radius: Option<Pixels> = None;

        let card = div()
            .flex()
            .flex_col()
            .gap_6()
            .w(CARD)
            .min_h_0()
            .max_h_full()
            .p_8()
            .rounded(ui::corners(theme.radius))
            .border_1()
            .border_color(theme.border.opacity(0.7))
            .bg(theme.background.opacity(0.62))
            .backdrop_blur(FROST)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .items_center()
                    .gap_3()
                    .child(self.welcome(status, cx))
                    .when_some(failure, |this, failure| {
                        this.child(crate::shared::trouble::trouble(failure, true))
                    })
                    .when_some(url, |this, url| this.child(self.url_prompt(url))),
            )
            .child(self.rows(&providers, pending, waiting, cx))
            .child(div().flex_none().child(self.divider(cx)))
            .when_some(guest, |this, slug| {
                this.child(div().flex_none().child(self.guest_mode(&slug, pending, cx)))
            })
            .child(div().flex_none().child(self.offline(pending, cx)));

        let backdrop = Lattice::new(POOLS.map(|(h, s, l, a)| hsla(h, s, l, a)))
            .stage(Hsla {
                a: 1.,
                ..theme.background
            })
            .lamp(self.lamp)
            .glow(self.glow_at, self.glow)
            .time(time)
            .when_some(radius, |this, radius| this.corner_radius(radius));

        div()
            .id("login")
            .relative()
            .size_full()
            .overflow_hidden()
            .when_some(radius, |this, radius| this.rounded(radius))
            .child(div().absolute().inset_0().child(backdrop))
            // The card never runs past the window: it shrinks to fit, and its provider list scrolls.
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .size_full()
                    .p(theme.metrics.pad)
                    .pt(theme.metrics.title_bar)
                    .child(card),
            )
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .top(theme.metrics.title_bar + px(8.))
                    .child(
                        Button::new("login-page-back")
                            .ghost()
                            .leading(
                                svg()
                                    .path(icons::path("icons/chevron-left.svg"))
                                    .size(px(16.))
                                    .text_color(theme.muted_foreground)
                                    .into_any_element(),
                            )
                            .label(t!("login-kugou-back"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.session
                                    .update(cx, |session, cx| session.close_login(cx));
                            })),
                    ),
            )
            .when_some(manual_secret, |this, (_, provider)| {
                this.child(self.secret_prompt(provider, cx).into_any_element())
            })
            .when_some(accounts, |this, accounts| {
                this.child(self.account_modal(accounts, cx).into_any_element())
            })
            .when_some(code, |this, (code, url)| {
                this.child(self.code_prompt(code, url, cx).into_any_element())
            })
            .when_some(qr, |this, (url, hint)| {
                this.child(
                    QrPrompt::new(url, hint.unwrap_or_else(|| t!("login-qr-scan").to_string()))
                        .on_cancel(cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)))
                        .into_any_element(),
                )
            })
            .when_some(phone_prompt, |this, notice| {
                let phone_error = self.phone_error.clone();
                let code_error = self.code_error.clone();
                let cooldown = self.phone_cooldown;
                let can_submit = self.phone_ready(cx);
                this.child(
                    PhonePrompt::new(self.phone.clone(), self.code.clone())
                        .notice(notice.map(SharedString::from))
                        .errors(phone_error, code_error)
                        .cooldown(cooldown)
                        .can_submit(can_submit)
                        .on_send(cx.listener(|this, _, _, cx| this.send_phone_code(cx)))
                        .on_submit(cx.listener(|this, _, _, cx| this.submit_phone(cx)))
                        .on_cancel(cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)))
                        .into_any_element(),
                )
            })
            .when_some(password_prompt, |this, notice| {
                this.child(
                    PasswordPrompt::new(self.username.clone(), self.password.clone())
                        .notice(notice.map(SharedString::from))
                        .errors(self.username_error.clone(), self.password_error.clone())
                        .on_submit(cx.listener(|this, _, _, cx| this.submit_password(cx)))
                        .on_cancel(cx.listener(|this, _, _, cx| this.abandon(cx)))
                        .into_any_element(),
                )
            })
            .when_some(verify_prompt, |this, (kind, context)| {
                this.child(self.verify_prompt(kind, context, cx))
            })
            .into_any_element()
    }
}

fn verify_detail(kind: &str, context: &str) -> SharedString {
    let hint = serde_json::from_str::<serde_json::Value>(context)
        .ok()
        .and_then(|value| {
            ["message", "url"]
                .into_iter()
                .filter_map(|key| {
                    value
                        .get(key)
                        .and_then(|value| value.as_str())
                        .filter(|hint| !hint.trim().is_empty())
                })
                .next()
                .map(str::to_owned)
        })
        .or_else(|| (!context.is_empty()).then(|| context.to_owned()));
    match hint {
        Some(hint) => t!("login-verify-detail", hint = hint).into(),
        None => t!("login-verify-detail-plain", kind = kind).into(),
    }
}

/// The methods a provider's card can switch between with one slider.
fn card_methods(options: &[SignIn]) -> Vec<SignIn> {
    [SignIn::Qr, SignIn::Phone, SignIn::Qq, SignIn::WeChat]
        .into_iter()
        .filter(|method| options.contains(method))
        .collect()
}

/// The colour a method's icon and highlight take. Scan and phone use the provider's own
/// accent, while QQ and WeChat keep their brand colours.
fn provider_method_accent(slug: &str, method: &SignIn, cx: &App) -> Hsla {
    match method {
        SignIn::Qr => crate::shared::provider_brand(slug, cx),
        SignIn::Phone => cx.theme().primary,
        SignIn::Qq => rgb(0x12B7F5).into(),
        _ => rgb(0x07C160).into(),
    }
}

fn valid_phone(phone: &str) -> bool {
    phone.len() == 11 && phone.starts_with('1') && phone.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_code(code: &str) -> bool {
    let code = code.trim();
    (4..=6).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_digit())
}

/// Which sign-in methods a provider offers on this screen. Guest mode has a button of its own,
/// and a local path is not a way to sign in.
fn offered(method: &SignIn) -> bool {
    matches!(
        method,
        SignIn::Default
            | SignIn::Secret
            | SignIn::Qr
            | SignIn::Phone
            | SignIn::Password
            | SignIn::Qq
            | SignIn::WeChat
    )
}
