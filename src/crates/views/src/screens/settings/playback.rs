use std::time::Duration;

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Div, MouseUpEvent, SharedString, Window, div, relative};
use i18n::t;
use music::{Capabilities, Encoding, Offer, Quality};
use state::{PREAMP_MAX, PREAMP_MIN, Sleep};
use ui::{ActiveTheme as _, OptionCard, Scrubber, Switch, Text};

use super::rows::{Announced as _, grid};
use super::{Group, SettingsView, Slot};

/// How many quality cards a row is laid out for, however few levels a service has.
const QUALITY_COLUMNS: usize = 4;
const STREAM_BITRATES: &str = "stream-bitrates";
/// How wide the sleep slider and the time beside it stand, in covers.
const SLEEP_WIDTH: f32 = 1.;
const SLEEP_LABEL: f32 = 0.7;
/// The shortest and longest timer the slider sets, in minutes.
const SLEEP_MIN_MINUTES: u64 = 1;
const SLEEP_MAX_MINUTES: u64 = 120;
const SLEEP_MAGNETS: [u64; 4] = [15, 30, 45, 60];
const SLEEP_MAGNET_WEIGHT: usize = 4;
/// The slot at the far right of the slider, end of track. Off is slot 0 and every minute
/// between takes one slot, or `SLEEP_MAGNET_WEIGHT` for a magnet.
const SLEEP_LAST: usize = (SLEEP_MAX_MINUTES - SLEEP_MIN_MINUTES + 1) as usize
    + SLEEP_MAGNETS.len() * (SLEEP_MAGNET_WEIGHT - 1)
    + 1;

impl SettingsView {
    pub(super) fn playback_groups(&self, cx: &App) -> Vec<Group> {
        let capabilities = self.session.read(cx).capabilities();
        let mut playing = vec![Slot::Normalisation, Slot::Gapless, Slot::Preamp];
        playing.extend([Slot::Sleep, Slot::StayAwake]);
        let mut groups = vec![Group::new("settings-group-playing", "icons/play.svg").rows(playing)];
        // The listener picks a level; a provider that cannot serve it falls back to its nearest.
        if !capabilities.qualities.is_empty() {
            groups.push(
                Group::new("settings-stream-quality", "icons/disc-3.svg")
                    .cards(Slot::StreamQuality),
            );
        }
        groups
    }

    /// [wisp] 前置放大：拖动即时生效（与音量同走 Gain 命令），保存走防抖。
    pub(super) fn preamp_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let current = self.playback.read(cx).preamp_db();
        let fraction = (current - PREAMP_MIN) / (PREAMP_MAX - PREAMP_MIN);
        let label = SharedString::from(format!("{current:+.1} dB"));
        let label_widths = [PREAMP_MIN, PREAMP_MAX]
            .into_iter()
            .map(|db| SharedString::from(format!("{db:+.1} dB")));

        let control = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                ui::spoken_steady("settings-preamp-value", label.clone(), label_widths)
                    .flex_none()
                    .w(theme.metrics.cover * SLEEP_LABEL)
                    .whitespace_nowrap()
                    .text_right(),
            )
            .child(
                div()
                    .id("settings-preamp-slider")
                    .role(Role::Group)
                    .aria_label(t!("settings-preamp"))
                    .w(theme.metrics.cover * SLEEP_WIDTH)
                    .child(
                        Scrubber::new(&self.preamp, fraction)
                            .name(t!("settings-preamp"))
                            .value(label)
                            .colors(
                                theme.progress_bar,
                                theme.muted_foreground.opacity(0.3),
                                theme.foreground,
                            )
                            .on_move(cx.listener(move |this, fraction: &f32, _, cx| {
                                let span = PREAMP_MAX - PREAMP_MIN;
                                let db = (PREAMP_MIN + fraction.clamp(0., 1.) * span) * 2.;
                                this.playback.update(cx, |playback, cx| {
                                    playback.set_preamp_db(db.round() / 2., cx)
                                });
                                cx.notify();
                            }))
                            .on_release(cx.listener(|_this, _: &MouseUpEvent, _, cx| {
                                cx.notify();
                            })),
                    ),
            );

        self.row(t!("settings-preamp"), control.into_any_element())
    }

    pub(super) fn playback_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.playback.read(cx).normalisation();

        self.row(
            t!("settings-normalisation"),
            Switch::new("normalisation", on)
                .name(t!("settings-normalisation"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.set_normalisation(!on, cx));
                }))
                .into_any_element(),
        )
    }

    pub(super) fn gapless_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.playback.read(cx).gapless();

        self.row(
            t!("settings-gapless"),
            Switch::new("gapless", on)
                .name(t!("settings-gapless"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.set_gapless(!on, cx));
                }))
                .into_any_element(),
        )
    }

    /// The switch that keeps played tracks on disk.
    pub(super) fn keep_tracks_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.playback.read(cx).keep_tracks();

        self.row(
            t!("settings-keep-tracks"),
            Switch::new("keep-tracks", on)
                .name(t!("settings-keep-tracks"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.set_keep_tracks(!on, cx));
                }))
                .into_any_element(),
        )
    }

    /// How many gigabytes the cache of played tracks may take, as a slider with the size before
    /// it. The label follows a drag at once, and the limit changes when the drag lets go.
    pub(super) fn cache_size_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let (low, high) = state::CACHE_GB_RANGE;
        let current = self
            .pending_cache
            .unwrap_or_else(|| self.playback.read(cx).cache_gb());
        let fraction = (current.saturating_sub(low)) as f32 / (high - low) as f32;

        let control = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                ui::spoken(
                    "settings-cache-size-value",
                    t!("settings-cache-size-value", count = current),
                )
                .flex_none()
                .w(theme.metrics.cover * SLEEP_LABEL)
                .whitespace_nowrap()
                .text_right(),
            )
            .child(
                div()
                    .id("settings-cache-slider")
                    .role(Role::Group)
                    .aria_label(t!("settings-cache-size"))
                    .w(theme.metrics.cover * SLEEP_WIDTH)
                    .child(
                        Scrubber::new(&self.cache, fraction)
                            .name(t!("settings-cache-size"))
                            .value(t!("settings-cache-size-value", count = current))
                            .colors(
                                theme.progress_bar,
                                theme.muted_foreground.opacity(0.3),
                                theme.foreground,
                            )
                            .on_move(cx.listener(move |this, fraction: &f32, _, cx| {
                                let span = (high - low) as f32;
                                this.pending_cache =
                                    Some(low + (fraction.clamp(0., 1.) * span).round() as u32);
                                cx.notify();
                            }))
                            .on_release(cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                let Some(gb) = this.pending_cache.take() else {
                                    return;
                                };
                                this.playback
                                    .update(cx, |playback, cx| playback.set_cache_gb(gb, cx));
                            })),
                    ),
            );

        self.row(t!("settings-cache-size"), control.into_any_element())
    }

    /// The stream level of a service that streams every track at one level, stated rather than
    /// offered since there is nothing to choose between.
    pub(super) fn stream_quality_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let offers = quality_ladder(self.session.read(cx).capabilities());
        let current = match offers.is_empty() {
            true => self.playback.read(cx).quality(),
            false => self.playback.read(cx).quality().within(&offers),
        };
        let encoding = offers
            .iter()
            .find(|offer| offer.quality == current)
            .map(encoding_label)
            .unwrap_or_default();

        self.row(
            i18n::lookup(quality_key(current), None),
            ui::spoken("settings-stream-quality-encoding", encoding)
                .text_color(theme.muted_foreground)
                .text_size(theme.text(Text::Label))
                .into_any_element(),
        )
    }

    /// One card per level the signed-in service streams at, lowest first, each saying what it
    /// streams as. A saved level the service lacks shows as the one it plays instead.
    pub(super) fn quality_cards(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ladder = quality_ladder(self.session.read(cx).capabilities());
        let current = match ladder.is_empty() {
            true => self.playback.read(cx).quality(),
            false => self.playback.read(cx).quality().within(&ladder),
        };
        let steps = ladder.len();

        // a short ladder keeps the width of a longer one rather than stretching across
        grid(steps.max(QUALITY_COLUMNS) as u16, window, cx)
            .children(ladder.into_iter().enumerate().map(|(rank, offer)| {
                let quality = offer.quality;
                OptionCard::new(quality.id())
                    .choice(
                        i18n::lookup(quality_key(quality), None),
                        quality == current,
                        rank,
                        steps,
                    )
                    .aria_description(encoding_label(&offer))
                    .thumb(signal(rank, steps, cx))
                    .label(i18n::lookup(quality_key(quality), None))
                    .detail(encoding_label(&offer))
                    .selected(quality == current)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.playback
                            .update(cx, |playback, cx| playback.set_quality(quality, cx));
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    pub(super) fn stay_awake_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).stay_awake();

        self.row(
            t!("settings-stay-awake"),
            Switch::new("stay-awake", on)
                .name(t!("settings-stay-awake"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_stay_awake(!on, cx));
                }))
                .into_any_element(),
        )
    }

    /// The sleep timer as a slider in its row with the time it is set to before it: off at the
    /// left, end of track at the right, minutes in between with the quarter hours widened. The
    /// label follows a drag at once, and the timer is armed when the drag lets go.
    pub(super) fn sleep_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let current = self
            .pending_sleep
            .unwrap_or_else(|| self.playback.read(cx).sleep());

        let control = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                ui::spoken("settings-sleep-value", sleep_label(current))
                    .flex_none()
                    .w(theme.metrics.cover * SLEEP_LABEL)
                    .whitespace_nowrap()
                    .text_right()
                    .text_color(match current {
                        Some(_) => theme.foreground,
                        None => theme.muted_foreground,
                    }),
            )
            .child(
                div()
                    .id("settings-sleep-slider")
                    .role(Role::Group)
                    .aria_label(t!("settings-sleep"))
                    .w(theme.metrics.cover * SLEEP_WIDTH)
                    .child(
                        Scrubber::new(&self.sleep, sleep_slot(current) as f32 / SLEEP_LAST as f32)
                            .name(t!("settings-sleep"))
                            .value(sleep_label(current))
                            .colors(
                                theme.progress_bar,
                                theme.muted_foreground.opacity(0.3),
                                theme.foreground,
                            )
                            .on_move(cx.listener(|this, fraction: &f32, _, cx| {
                                this.pending_sleep = Some(sleep_at_fraction(*fraction));
                                cx.notify();
                            }))
                            .on_release(cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                let Some(sleep) = this.pending_sleep.take() else {
                                    return;
                                };
                                this.playback
                                    .update(cx, |playback, cx| playback.set_sleep(sleep, cx));
                            })),
                    ),
            );

        self.row(t!("settings-sleep"), control.into_any_element())
    }
}

fn sleep_slot(sleep: Option<Sleep>) -> usize {
    match sleep {
        None => 0,
        Some(Sleep::EndOfTrack) => SLEEP_LAST,
        Some(Sleep::After(after)) => minute_slot(after.as_secs() / 60),
    }
}

fn sleep_at_fraction(fraction: f32) -> Option<Sleep> {
    let slot = (fraction.clamp(0., 1.) * SLEEP_LAST as f32).round() as usize;
    match slot {
        0 => None,
        SLEEP_LAST => Some(Sleep::EndOfTrack),
        slot => Some(Sleep::After(Duration::from_secs(slot_minute(slot) * 60))),
    }
}

fn minute_slot(minutes: u64) -> usize {
    let minutes = minutes.clamp(SLEEP_MIN_MINUTES, SLEEP_MAX_MINUTES);
    let earlier_magnets = SLEEP_MAGNETS
        .iter()
        .filter(|magnet| **magnet < minutes)
        .count();
    let width = match SLEEP_MAGNETS.contains(&minutes) {
        true => SLEEP_MAGNET_WEIGHT,
        false => 1,
    };
    (minutes - SLEEP_MIN_MINUTES + 1) as usize
        + earlier_magnets * (SLEEP_MAGNET_WEIGHT - 1)
        + (width - 1) / 2
}

fn slot_minute(slot: usize) -> u64 {
    let mut first = 1;
    for minute in SLEEP_MIN_MINUTES..=SLEEP_MAX_MINUTES {
        let width = match SLEEP_MAGNETS.contains(&minute) {
            true => SLEEP_MAGNET_WEIGHT,
            false => 1,
        };
        if slot < first + width {
            return minute;
        }
        first += width;
    }
    SLEEP_MAX_MINUTES
}

fn sleep_label(sleep: Option<Sleep>) -> SharedString {
    match sleep {
        Some(Sleep::EndOfTrack) => t!("settings-sleep-end-of-track"),
        Some(Sleep::After(after)) => t!("settings-sleep-minutes", count = after.as_secs() / 60),
        None => t!("settings-sleep-off"),
    }
}

/// The Fluent key naming a stream format, where `None` keeps the format the server sends.
pub(super) fn quality_key(quality: Quality) -> &'static str {
    match quality {
        Quality::ViperAtmos => "settings-stream-quality-viper-atmos",
        Quality::ViperTape => "settings-stream-quality-viper-tape",
        Quality::ViperClear => "settings-stream-quality-viper-clear",
        Quality::HiRes => "settings-stream-quality-hi-res",
        Quality::Lossless => "settings-stream-quality-lossless",
        Quality::High => "settings-stream-quality-high",
        Quality::Normal => "settings-stream-quality-normal",
        Quality::Low => "settings-stream-quality-low",
    }
}

/// The levels the signed-in service serves, lowest first for the cards. The provider's
/// declaration, best first, is the whole ladder: a level it does not name is not shown,
/// since there is nothing behind it to play.
fn quality_ladder(capabilities: Capabilities) -> Vec<Offer> {
    capabilities.qualities.iter().rev().copied().collect()
}

/// What a level streams as, such as "FLAC, up to 24-bit 192 kHz" or "AAC, 320 kbps".
pub(super) fn encoding_label(offer: &Offer) -> SharedString {
    let lossy = |codec: &str, kbps: u32| match offer.up_to {
        true => t!(
            "settings-stream-quality-lossy-up-to",
            codec = codec.to_owned(),
            kbps = i64::from(kbps)
        ),
        false => t!(
            "settings-stream-quality-lossy",
            codec = codec.to_owned(),
            kbps = i64::from(kbps)
        ),
    };
    match offer.encoding {
        Encoding::Flac { depth, rate } => {
            // 44100 reads as 44.1 and 192000 as 192
            let khz = match rate % 1000 {
                0 => (rate / 1000).to_string(),
                _ => format!("{:.1}", f64::from(rate) / 1000.0),
            };
            match offer.up_to {
                true => t!(
                    "settings-stream-quality-flac-up-to",
                    depth = i64::from(depth),
                    rate = khz
                ),
                false => t!(
                    "settings-stream-quality-flac",
                    depth = i64::from(depth),
                    rate = khz
                ),
            }
        }
        Encoding::Aac { kbps } => lossy("AAC", kbps),
        Encoding::Vorbis { kbps } => lossy("Ogg Vorbis", kbps),
        Encoding::Mp3 { kbps } => lossy("MP3", kbps),
        Encoding::Opus { kbps } => lossy("Opus", kbps),
        // No service offers these. They name a local file's encoding in the signal path.
        Encoding::Alac { .. } => t!("signal-path-codec-alac"),
        Encoding::Pcm { .. } => t!("signal-path-codec-pcm"),
        Encoding::Other => t!("signal-path-codec-other"),
    }
}

/// About how much data an hour at a bitrate ceiling takes, said under it in the picker.

/// A rising row of `steps` bars with the first `rank + 1` lit, the way a quality card shows how
/// far up the ladder its level stands.
fn signal(rank: usize, steps: usize, cx: &App) -> Div {
    let theme = cx.theme();

    div()
        .size_full()
        .flex()
        .items_end()
        .justify_center()
        .gap(theme.metrics.pad / 2.)
        .pb(theme.metrics.pad * 1.5)
        .children((0..steps).map(|step| {
            div()
                .w(theme.metrics.pad)
                .h(relative(0.2 + 0.5 * (step + 1) as f32 / steps as f32))
                .rounded(theme.metrics.pad / 4.)
                .bg(match step <= rank {
                    true => theme.foreground,
                    false => theme.muted,
                })
        }))
}
