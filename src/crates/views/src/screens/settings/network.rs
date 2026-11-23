use gpui::prelude::*;
use gpui::{AnyElement, App, Context};
use i18n::t;
use ui::{Field, Form, MenuItem, Picker, Switch};

use super::{Group, SettingsView, Slot};

impl SettingsView {
    pub(super) fn unblock_available(&self, cx: &App) -> bool {
        self.session.read(cx).provider_slug() == Some("netease")
    }

    /// The externals the app reaches out to when a greyed-out track has no stream of its own:
    /// the switch that allows it, the source it prefers, and the cookies of the two services
    /// that need an account to answer.
    pub(super) fn network_groups(&self, cx: &App) -> Vec<Group> {
        if !self.unblock_available(cx) {
            return Vec::new();
        }
        vec![
            Group::new("settings-group-network", "icons/link.svg").rows([
                Slot::UnblockEnabled,
                Slot::UnblockSource,
                Slot::UnblockOrder,
                Slot::UnblockBudget,
                Slot::UnblockFlac,
                Slot::UnblockQqCookie,
                Slot::UnblockJooxCookie,
            ]),
        ]
    }

    /// [wisp] 解灰开关：灰歌拿不到直链时去外部音源找。
    pub(super) fn unblock_enabled_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).unblock_enabled();

        self.row(
            t!("settings-unblock"),
            Switch::new("unblock-enabled", on)
                .name(t!("settings-unblock"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_unblock_enabled(!on, cx));
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// [wisp] 解灰音源：自动（全部引擎）或指定一个。
    pub(super) fn unblock_source_row(&self, cx: &mut Context<Self>) -> AnyElement {
        const SOURCES: [(&str, &str); 8] = [
            ("", "settings-unblock-source-auto"),
            ("kugou", "settings-unblock-source-kugou"),
            ("kuwo", "settings-unblock-source-kuwo"),
            ("bodian", "settings-unblock-source-bodian"),
            ("migu", "settings-unblock-source-migu"),
            ("bilibili", "settings-unblock-source-bilibili"),
            ("qq", "settings-unblock-source-qq"),
            ("joox", "settings-unblock-source-joox"),
        ];
        let chosen = self.settings.read(cx).unblock_source().to_owned();
        let label = SOURCES
            .iter()
            .find(|(id, _)| *id == chosen)
            .map(|(_, key)| i18n::lookup(key, None))
            .unwrap_or_else(|| i18n::lookup("settings-unblock-source-auto", None));

        let picker = Picker::new("unblock-source", &self.popovers, label)
            .width(Picker::NARROW)
            .items(SOURCES.map(|(id, key)| {
                let selected = id == chosen.as_str();
                MenuItem::new(key, i18n::lookup(key, None))
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings
                            .update(cx, |settings, cx| settings.set_unblock_source(id, cx));
                        cx.notify();
                    }))
            }));

        self.row(
            t!("settings-unblock-source"),
            picker
                .name(t!("settings-unblock-source"))
                .into_any_element(),
        )
    }

    /// [wisp] 解灰顺序优先：关掉后所有音源并发，谁先给出直链用谁。
    pub(super) fn unblock_order_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).unblock_order_first();

        self.row(
            t!("settings-unblock-order"),
            Switch::new("unblock-order", on)
                .name(t!("settings-unblock-order"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_unblock_order_first(!on, cx));
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// [wisp] 解灰总预算：一次解灰最多等多久，到点就放弃。
    pub(super) fn unblock_budget_row(&self, cx: &mut Context<Self>) -> AnyElement {
        const BUDGETS: [u32; 6] = [5_000, 10_000, 15_000, 20_000, 25_000, 30_000];
        let chosen = self.settings.read(cx).unblock_budget_ms();
        let label = t!("settings-unblock-budget-value", seconds = chosen / 1_000);

        let picker = Picker::new("unblock-budget", &self.popovers, label)
            .width(Picker::NARROW)
            .items(BUDGETS.map(|ms| {
                let selected = ms == chosen;
                MenuItem::new(
                    ("unblock-budget", ms as usize),
                    t!("settings-unblock-budget-value", seconds = ms / 1_000),
                )
                .selected(selected)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_unblock_budget_ms(ms, cx));
                    cx.notify();
                }))
            }));

        self.row(
            t!("settings-unblock-budget"),
            picker
                .name(t!("settings-unblock-budget"))
                .into_any_element(),
        )
    }

    /// [wisp] 解灰优先无损。
    pub(super) fn unblock_flac_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).unblock_flac();

        self.row(
            t!("settings-unblock-flac"),
            Switch::new("unblock-flac", on)
                .name(t!("settings-unblock-flac"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_unblock_flac(!on, cx));
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// [wisp] QQ 音乐 Cookie（格式 `uin=...; qqmusic_key=...`）。
    pub(super) fn unblock_qq_cookie_row(&self, cx: &App) -> AnyElement {
        self.yielding_row(
            t!("settings-unblock-qq-cookie"),
            Form::new("settings-unblock-qq-cookie-form")
                .w(Picker::WIDE)
                .field(Field::new(&self.unblock_qq_cookie, cx))
                .into_any_element(),
        )
    }

    /// [wisp] Joox Cookie（格式 `wmid=...; session_key=...`）。
    pub(super) fn unblock_joox_cookie_row(&self, cx: &App) -> AnyElement {
        self.yielding_row(
            t!("settings-unblock-joox-cookie"),
            Form::new("settings-unblock-joox-cookie-form")
                .w(Picker::WIDE)
                .field(Field::new(&self.unblock_joox_cookie, cx))
                .into_any_element(),
        )
    }
}
