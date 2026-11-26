use std::time::Duration;

use gpui::accesskit::Toggled;
use gpui::prelude::*;
use gpui::{App, Entity, Global, SharedString, div};
use i18n::t;
use state::{PlayMode, Playback, Wisp};
use ui::{ActiveTheme as _, Button, Side};

use crate::shared::ambient;

pub(crate) const NOTCH: f32 = state::VOLUME_STEP;
/// The name the playing cover is seen under, so the fullscreen player's cover flies in from the
/// player bar's.
pub(crate) const COVER_FLIGHT: &str = "playing-cover";

/// Whether the playing cover is in the air between the player bar and the fullscreen player,
/// so the player bar leaves its own undrawn until the flight lands.
pub(crate) struct CoverAway(pub(crate) bool);

impl Global for CoverAway {}

/// How much of the opening fullscreen player shows yet, from 0 to 1, which its cover fades in
/// by while it flies above the fade the rest of the player is under.
pub(crate) struct CoverFade(pub(crate) f32);

impl Global for CoverFade {}

/// How much room the player bar's cover takes beside the title, from 1 where it shows to 0 where
/// the title has slid over its place. Below `Room::Wide` the fullscreen player's artwork tab
/// tucks it away, since its big cover shows the same art, and the player bar under the player
/// tucks its own along while the player opens and closes, so the two never part.
pub(crate) struct CoverRoom(pub(crate) f32);

impl Global for CoverRoom {}

/// Which way the fullscreen player is moving between the shells, if it is. The player bar's
/// fullscreen button and the player's leave button share one spot, so while the player moves
/// the button of the shell it moves to paints above the fade and the other paints nothing.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ShellSwap {
    #[default]
    Settled,
    Opening,
    Closing,
}

impl Global for ShellSwap {}
const STEP: f32 = 0.004;

/// How far through `total` `elapsed` is, from 0 to 1.
pub(crate) fn fraction(elapsed: Duration, total: Duration) -> f32 {
    match total.is_zero() {
        true => 0.,
        false => (elapsed.as_secs_f32() / total.as_secs_f32()).clamp(0., 1.),
    }
}

pub(crate) fn volume_icon(level: f32) -> &'static str {
    match level {
        level if level <= 0.0001 => "icons/volume-x.svg",
        level if level < 0.25 => "icons/volume.svg",
        level if level < 0.5 => "icons/volume-1.svg",
        _ => "icons/volume-2.svg",
    }
}

pub(crate) fn percent(fraction: f32) -> SharedString {
    t!("player-percent", value = (fraction * 100.).round())
}

/// The heart that saves or drops `track`. A click toggles the track `playback` holds by then,
/// which is the one shown, so callers only lend the track to read its state.
pub(crate) fn like(track: Option<&music::Track>, playback: &Entity<Playback>, cx: &App) -> Button {
    let theme = *cx.theme();
    let library = Wisp::global(cx).library.clone();
    let playback = playback.clone();
    let id = track.and_then(|track| track.id.as_deref());
    let saved = id.is_some_and(|id| library.read(cx).saved(id));

    Button::new("toggle-liked-track")
        .ghost()
        .backgroundless()
        .small()
        .icon(match saved {
            true => "icons/heart-filled.svg",
            false => "icons/heart.svg",
        })
        .tooltip_above(match saved {
            true => "menu-remove-from-library",
            false => "menu-add-to-library",
        })
        .tint(match saved {
            true => theme.primary,
            false => theme.muted_foreground,
        })
        .disabled(id.is_none())
        .on_click(move |_, _, cx| {
            if let Some(track) = playback.read(cx).track().cloned() {
                library.update(cx, |library, cx| library.toggle(track, cx));
            }
        })
}

/// `big` is only true in fullscreen, where these buttons float over the ambient
/// background and so blur their hover fills. Without that background, and on
/// the flat player bar, the blur would show nothing.
pub(crate) fn transport(playback: &Entity<Playback>, big: bool, cx: &App) -> impl IntoElement {
    let frosted = big && ambient::shown(cx);

    controls(playback, big, frosted, cx).child(climax(playback, frosted, cx))
}

/// The transport without the climax button, for a bar whose climax key sits elsewhere.
pub(crate) fn transport_controls(playback: &Entity<Playback>, cx: &App) -> impl IntoElement {
    controls(playback, false, false, cx)
}

fn controls(playback: &Entity<Playback>, big: bool, frosted: bool, cx: &App) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(previous(playback, frosted, cx))
        .child(toggle(playback, big, frosted, cx))
        .child(next(playback, frosted, cx))
}

/// Jumps to the next climax start, or back to the first once the track has passed them all.
pub(crate) fn climax(playback: &Entity<Playback>, frosted: bool, cx: &App) -> Button {
    let enabled = !Wisp::global(cx).climax.read(cx).marks().is_empty();
    let playback = playback.clone();

    Button::new("jump-climax")
        .ghost()
        .when(frosted, Button::frosted)
        .small()
        .icon("icons/flame.svg")
        .tooltip_above("player-climax")
        .disabled(!enabled)
        .on_click(move |_, _, cx| {
            let target = {
                let (elapsed, total) = playback.read(cx).shown_time(cx);
                Wisp::global(cx).climax.read(cx).jump_target(elapsed, total)
            };
            if let Some(target) = target {
                playback.update(cx, |playback, cx| playback.seek(target, cx));
            }
        })
}

pub(crate) fn toggle(playback: &Entity<Playback>, big: bool, frosted: bool, cx: &App) -> Button {
    let held = playback.read(cx);
    let playing = held.wants_playing();
    let idle = held.track().is_none() && !held.can_restart_queue(cx);
    let playback = playback.clone();

    let (icon, tooltip) = match playing {
        true => ("icons/pause.svg", "play-pause"),
        false => ("icons/play.svg", "play-resume"),
    };

    // One id for both states, so the button keeps its press and swaps one icon for the other.
    Button::new("play-pause")
        .ghost()
        .when(frosted, Button::frosted)
        .when(!big, Button::small)
        .icon(icon)
        .tooltip_above(tooltip)
        .disabled(idle)
        .on_click(move |_, _, cx| {
            playback.update(cx, |playback, cx| playback.toggle_play(cx));
        })
}

pub(crate) fn play_mode(playback: &Entity<Playback>, frosted: bool, cx: &App) -> Button {
    let theme = *cx.theme();
    let held = playback.read(cx);

    // [wisp] 私人 FM 会话：循环键变成 ∞，点一下切到单曲循环（与旧版一致）。
    if held.fm_session() {
        let on = held.fm_repeat();
        let playback = playback.clone();
        return Button::new("play-mode")
            .ghost()
            .when(frosted, Button::frosted)
            .small()
            .icon(match on {
                true => "icons/repeat-one.svg",
                false => "icons/infinity.svg",
            })
            .blending()
            .indicator(on)
            .aria_toggled(Toggled::from(on))
            .tooltip_above(match on {
                true => "player-fm-repeat-one",
                false => "player-fm-infinite",
            })
            .tint(match on {
                true => theme.primary,
                false => theme.muted_foreground,
            })
            .on_click(move |_, _, cx| {
                playback.update(cx, |playback, cx| playback.toggle_fm_repeat(cx));
            });
    }

    // [wisp] 一个键循环 顺序 → 列表循环 → 随机 → 单曲循环，与旧版一致。
    let mode = held.play_mode(cx);
    let playback = playback.clone();
    let (icon, active) = match mode {
        PlayMode::Sequential => ("icons/repeat.svg", false),
        PlayMode::List => ("icons/repeat.svg", true),
        PlayMode::Random => ("icons/shuffle.svg", true),
        PlayMode::Single => ("icons/repeat-one.svg", true),
    };

    Button::new("play-mode")
        .ghost()
        .when(frosted, Button::frosted)
        .small()
        .icon(icon)
        .blending()
        .indicator(active)
        .aria_toggled(Toggled::from(active))
        .tooltip_above(mode.key())
        .tint(match active {
            true => theme.primary,
            false => theme.muted_foreground,
        })
        .on_click(move |_, _, cx| {
            playback.update(cx, |playback, cx| playback.cycle_play_mode(cx));
        })
}

pub(crate) fn previous(playback: &Entity<Playback>, frosted: bool, cx: &App) -> Button {
    let queue = playback.read(cx).shown_queue(cx);
    let enabled = !queue.editable() || queue.sections.current || queue.sections.past > 0;
    let playback = playback.clone();

    Button::new("previous")
        .ghost()
        .when(frosted, Button::frosted)
        .small()
        .icon("icons/skip-back.svg")
        .belt(Side::Left)
        .tooltip_above("player-previous")
        .disabled(!enabled)
        .on_click(move |_, _, cx| {
            playback.update(cx, |playback, cx| playback.previous(cx));
        })
}

pub(crate) fn next(playback: &Entity<Playback>, frosted: bool, cx: &App) -> Button {
    let queue = playback.read(cx).shown_queue(cx);
    // 最后一首上仍可点：列表循环/随机回绕，顺序模式由播放层处理。
    let enabled = !queue.editable() || queue.sections.current || queue.sections.upcoming > 0;
    let playback = playback.clone();

    Button::new("next")
        .ghost()
        .when(frosted, Button::frosted)
        .small()
        .icon("icons/skip-forward.svg")
        .belt(Side::Right)
        .tooltip_above("player-next")
        .disabled(!enabled)
        .on_click(move |_, _, cx| {
            playback.update(cx, |playback, cx| playback.next(cx));
        })
}

pub(crate) fn moved(before: Option<f32>, after: Option<f32>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => (before - after).abs() > STEP,
        (before, after) => before.is_some() != after.is_some(),
    }
}
