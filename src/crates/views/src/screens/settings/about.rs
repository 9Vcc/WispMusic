use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, SharedString, div};
use i18n::t;
use state::{Channel, CheckFeedback, Wisp};
use ui::{ActiveTheme as _, Button, Spinner, Text};

use super::rows::{Row, outward, segmented};
use super::{Group, SettingsView, Slot};

/// How large the app icon stands, in covers.
const MARK_TILE: f32 = 0.5;
/// The GPL-3.0 project Wisp's player is built from, named on the page since Wisp ships its
/// source in binary form and the licence asks for the attribution to travel with it.
const UPSTREAM_URL: &str = "https://github.com/sonorahq/sonora";

impl SettingsView {
    pub(super) fn about_groups(&self) -> Vec<Group> {
        let check = Some(Slot::CheckUpdates);
        let channel = (state::updates_itself() && Channel::running() != Channel::Stable)
            .then_some(Slot::Channel);
        vec![
            Group::untitled().rows(
                [Slot::Version, Slot::Log]
                    .into_iter()
                    .chain(check)
                    .chain(channel)
                    .chain([Slot::Upstream]),
            ),
        ]
    }

    /// The version row, which for a nightly also names the commit it was built from.
    pub(super) fn version_row(&self) -> AnyElement {
        let version: SharedString = match state::commit() {
            Some(commit) => t!(
                "settings-version-commit",
                version = state::version(),
                commit = commit
            ),
            None => state::version().into(),
        };
        self.row(
            t!("settings-version"),
            ui::spoken("settings-version-value", version).into_any_element(),
        )
    }

    pub(super) fn check_updates_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let updates = Wisp::global(cx).updates.clone();
        let (checking, feedback) = {
            let updates = updates.read(cx);
            (updates.checking(), updates.feedback())
        };
        let button = Button::new("check-updates-now")
            .small()
            .outline()
            .label(match checking {
                true => t!("settings-check-updates-checking"),
                false => t!("settings-check-updates-now"),
            })
            .disabled(checking);
        let button = match checking {
            true => button.leading(Spinner::new("check-updates-spinner")),
            false => button.icon("icons/refresh-cw.svg"),
        };
        let button = button.on_click(cx.listener(move |_, _, _, cx| {
            updates.update(cx, |updates, cx| updates.check(cx));
        }));
        let detail = match feedback {
            Some(CheckFeedback::Latest) => {
                t!("settings-check-updates-latest", version = state::version())
            }
            Some(CheckFeedback::Updating) => t!("settings-check-updates-updating"),
            Some(CheckFeedback::Failed) => t!("settings-check-updates-failed"),
            None => t!("settings-check-updates-now-detail"),
        };
        Row::new(t!("settings-check-updates-now"), button.into_any_element())
            .detail(detail)
            .into_any_element()
    }

    pub(super) fn channel_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.settings.read(cx).update_channel();

        let choices = segmented(
            "update-channel",
            Channel::ALL.map(|channel| (channel, channel.label())),
            current,
            cx.listener(|this, channel: &Channel, _, cx| {
                let channel = *channel;
                this.settings
                    .update(cx, |settings, cx| settings.set_update_channel(channel, cx));
            }),
        );

        self.row(
            t!("settings-update-channel"),
            choices
                .name(t!("settings-update-channel"))
                .into_any_element(),
        )
    }

    /// The row that opens a copy of the current log file in whatever the system reads text
    /// with. It is disabled when the platform names no state or cache folder to log into.
    pub(super) fn log_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let path = state::log_file();

        outward("open-log", t!("settings-log-open"), None, cx)
            .disabled(path.is_none())
            .on_click(move |_, _, _| {
                let Some(path) = &path else { return };
                if let Err(error) = snapshot(path).and_then(open::that_detached) {
                    log::warn!(
                        "settings: cannot open a copy of {}: {error}",
                        path.display()
                    );
                }
            })
            .into_any_element()
    }

    /// The GPL-3.0 project the player comes from, which the licence asks Wisp to name wherever
    /// the binary goes. The version is the Sonora release the vendored crates were taken from.
    pub(super) fn upstream_row(&self, cx: &mut Context<Self>) -> AnyElement {
        outward(
            "upstream",
            t!("settings-upstream"),
            Some(t!("settings-upstream-detail")),
            cx,
        )
        .on_click(|_, _, cx| cx.open_url(UPSTREAM_URL))
        .into_any_element()
    }

    /// Opens About with the app icon beside its name, aligned with the settings below. The icon
    /// is the one this platform shows, in its own colours whatever the theme.
    pub(super) fn mark(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let tile = theme.metrics.cover * MARK_TILE;

        div()
            .flex()
            .items_center()
            .gap_5()
            .child(crate::brand::icon(tile))
            .child(
                div()
                    .id("settings-about-name")
                    .role(gpui::accesskit::Role::Heading)
                    .aria_level(2)
                    .aria_label(t!("settings-about-name"))
                    .text_size(theme.text(Text::Title))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("settings-about-name")),
            )
    }

    pub(super) fn notice(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();

        div()
            .id("settings-notice")
            .role(gpui::accesskit::Role::Paragraph)
            .aria_label(t!("settings-notice"))
            .text_color(theme.muted_foreground)
            .text_size(theme.text(Text::Small))
            .child(t!("settings-notice"))
    }
}

/// Notepad on Windows refuses a file that Wisp still writes to, so the button opens a copy.
/// `File::open` shares write access, and `fs::copy` documents no share mode.
fn snapshot(log: &Path) -> io::Result<PathBuf> {
    let copy = log.with_file_name("wisp.copy.log");
    io::copy(&mut File::open(log)?, &mut File::create(&copy)?)?;
    Ok(copy)
}
