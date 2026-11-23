use gpui::prelude::*;
use gpui::{AnyElement, App, ClickEvent, ElementId, SharedString, Window};
use i18n::t;
use music::SignInProblem;
use state::{Failure, Network};
use ui::{Button, Notice, Vacancy};

/// Shows a sign-in failure with recovery guidance for known account problems.
/// Connection failures keep their own title and icon because signing in again will not help.
pub(crate) fn trouble(failure: Failure, centered: bool) -> AnyElement {
    let offline = failure.offline();
    let Failure {
        problem,
        summary,
        detail,
    } = failure;

    let (title, message) = match (offline, problem) {
        (true, _) => (t!("trouble-offline"), t!("trouble-offline-detail")),
        (false, Some(problem)) => (
            t!("login-failed-title"),
            i18n::lookup(reason(problem), None),
        ),
        (false, None) => (
            t!("login-failed-title"),
            SharedString::from(sentence(summary, detail)),
        ),
    };

    Notice::new(title, message)
        .spoken("trouble-notice")
        .failed()
        .when(offline, |notice| notice.icon("icons/wifi-off.svg"))
        .when(centered, Notice::centered)
        .into_any_element()
}

/// Whether a page built from `id` cannot be read at all because the network is gone. A local
/// id never is, since nothing about it leaves the machine.
pub(crate) fn unreachable(id: &str, cx: &App) -> bool {
    !music::is_local_id(id) && Network::lost(cx)
}

/// The page-filling state for a screen that cannot load, with a button that tries again.
/// `reason` is what a load came back with, or `None` when the app already knows the network is
/// gone and the screen never tried. A reason that reads as a lost connection becomes the same
/// No connection state, a rate limit becomes a Service is busy state, and a provider refusing
/// the network says a VPN may be the cause. Anything else shows `label` over a plain sentence,
/// because the reason is an error chain meant for the log and never for the user.
pub(crate) fn lost(
    id: impl Into<ElementId>,
    label: SharedString,
    reason: Option<&str>,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    _cx: &App,
) -> Vacancy {
    let id: ElementId = id.into();
    let vacancy = match reason.filter(|reason| !music::trouble::offline(reason)) {
        None => Vacancy::new(t!("trouble-offline"))
            .detail(t!("trouble-offline-detail"))
            .icon("icons/wifi-off.svg"),
        Some(reason) if music::trouble::busy(reason) => Vacancy::new(t!("trouble-busy"))
            .detail(t!("trouble-busy-detail"))
            .icon("icons/gauge.svg"),
        Some(reason) if music::trouble::blocked(reason) => Vacancy::new(t!("trouble-blocked"))
            .detail(t!("trouble-blocked-detail"))
            .icon("icons/circle-alert.svg"),
        Some(_) => Vacancy::new(label)
            .detail(t!("trouble-failed-detail"))
            .icon("icons/circle-alert.svg"),
    };

    vacancy.spoken((id.clone(), "caption")).action(
        Button::new(id)
            .label(t!("trouble-retry"))
            .icon("icons/refresh-cw.svg")
            .outline()
            .on_click(retry),
    )
}

/// The failure's message alone, for the places that only have room for one line.
pub(crate) fn short(failure: &Failure) -> SharedString {
    match failure.problem {
        Some(problem) => i18n::lookup(reason(problem), None),
        None => SharedString::from(sentence(failure.summary.clone(), failure.detail.clone())),
    }
}

fn sentence(summary: String, detail: Option<String>) -> String {
    let summary = summary.trim_end_matches('.').to_owned();
    match detail.map(|detail| unwrapped_reason(&detail)) {
        Some(reason) => format!("{summary}: {reason}"),
        None => format!("{summary}."),
    }
}

fn unwrapped_reason(text: &str) -> String {
    let inner = text
        .split_once('{')
        .and_then(|(_, rest)| rest.rsplit_once('}'))
        .map(|(inner, _)| inner)
        .unwrap_or(text)
        .trim();
    let reason = inner
        .split_once("with reason:")
        .map(|(_, reason)| reason)
        .unwrap_or(inner)
        .trim()
        .trim_end_matches('.');
    let mut letters = reason.chars();
    match letters.next() {
        Some(first) => format!("{}{}.", first.to_lowercase(), letters.as_str()),
        None => text.to_owned(),
    }
}

fn reason(problem: SignInProblem) -> &'static str {
    match problem {
        SignInProblem::Region => "login-problem-region",
        SignInProblem::Credentials => "login-problem-credentials",
        SignInProblem::Password => "login-problem-password",
        SignInProblem::Network => "login-problem-network",
        SignInProblem::Blocked => "login-problem-blocked",
        SignInProblem::Cancelled => "login-problem-cancelled",
        SignInProblem::Refused => "login-problem-refused",
    }
}
