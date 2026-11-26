use gpui::prelude::*;
use gpui::{AnyElement, App, Context, SharedString, div};
use i18n::t;
use ui::Switch;

use super::{Group, SettingsView, Slot};

impl SettingsView {
    /// The switch for the active provider's own play record.
    pub(super) fn scrobbling_groups(&self, cx: &App) -> Vec<Group> {
        if self.session.read(cx).provider_slug().is_none() {
            return Vec::new();
        }
        vec![Group::untitled().rows([Slot::ScrobbleProvider])]
    }

    /// The switch's label and explanation, named after the active provider.
    pub(super) fn scrobble_provider_meta(&self, cx: &App) -> Option<(SharedString, SharedString)> {
        let provider = self.session.read(cx).provider_name()?;
        Some((
            t!("settings-scrobble-provider", provider = provider),
            t!("settings-scrobble-provider-detail", provider = provider),
        ))
    }

    /// Whether the active provider records the plays on its own server.
    pub(super) fn scrobble_provider_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let session = self.session.read(cx);
        let (Some(slug), Some(name)) = (session.provider_slug(), session.provider_name()) else {
            return div().into_any_element();
        };
        let on = self.settings.read(cx).scrobbles(slug);
        let label = t!("settings-scrobble-provider", provider = name);

        self.row(
            label.clone(),
            Switch::new("scrobble-provider", on)
                .name(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_scrobbles(slug, !on, cx));
                }))
                .into_any_element(),
        )
    }
}
