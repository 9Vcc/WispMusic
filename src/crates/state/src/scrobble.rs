//! Provider play reporting.
//!
//! [`Scrobbling`] follows [`Playback`] and tells the provider that served the track when it
//! starts, pauses, seeks, stops and completes a listen. The settings page exposes one switch
//! per active provider, so its own play counts and now-playing state can be kept in sync.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{App, Context, Entity};
use music::{MusicApi, Report, Track};
use tokio::sync::mpsc;

use crate::playback::PlaybackEvent;
use crate::{Io, Playback, PlaybackState, Session};

/// A track shorter than this never counts, and a play this long always does. The rule belongs
/// to the finished listen, not to any one provider.
const MIN_LENGTH: Duration = Duration::from_secs(30);
const FULL_PLAY: Duration = Duration::from_secs(240);

/// One provider report on its way to the server, applied in the order it was queued.
type Reported = (Arc<dyn MusicApi>, String, Report, Duration);

/// What a finished listen needs to know to decide whether it has been earned.
struct Listen {
    duration: Duration,
    at: SystemTime,
}

impl Listen {
    fn earned(&self, played: Duration) -> bool {
        self.duration >= MIN_LENGTH
            && (played >= FULL_PLAY || played.as_secs_f64() * 2. >= self.duration.as_secs_f64())
    }
}

pub struct Scrobbling {
    playback: Entity<Playback>,
    session: Entity<Session>,
    settings: Entity<crate::AppSettings>,
    io: Io,
    play: Option<Listen>,
    current: Option<String>,
    started: SystemTime,
    /// Whether the current play has already gone to the provider it came from.
    reported: bool,
    /// Where the current track was last heard, for the report that says it stopped.
    position: Duration,
    /// The queue that sends state reports one at a time. A segue puts a stop and the next start
    /// back to back, and the start has to land last or the server forgets the new track.
    reports: mpsc::UnboundedSender<Reported>,
}

impl Scrobbling {
    pub fn new(
        playback: Entity<Playback>,
        session: Entity<Session>,
        settings: Entity<crate::AppSettings>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&playback, |this, _, event, cx| match event {
            PlaybackEvent::StartedPlayback => this.begin(cx),
            PlaybackEvent::EndedPlayback => this.end(cx),
            PlaybackEvent::Paused | PlaybackEvent::Seeked => this.moved(cx),
        })
        .detach();
        cx.observe(&playback, |this, _, cx| this.tick(cx)).detach();
        // a play is earned by the time listened, which moves on position reports
        let ticks = playback.read(cx).ticks().clone();
        cx.observe(&ticks, |this, _, cx| this.tick(cx)).detach();

        Self {
            playback,
            session,
            settings,
            io: io.clone(),
            play: None,
            current: None,
            started: SystemTime::now(),
            reported: false,
            position: Duration::ZERO,
            reports: send_reports(&io),
        }
    }

    /// Starts a play and tells the provider it is playing now. A pause and resume of the same
    /// track keeps the original start time, so the finished listen is timed from when it began.
    fn begin(&mut self, cx: &mut Context<Self>) {
        let playback = self.playback.read(cx);
        let Some(track) = playback.track() else {
            return;
        };
        let resumed = track.id.is_some() && self.current == track.id;
        if !resumed {
            self.current = track.id.clone();
            self.started = SystemTime::now();
            self.reported = false;
        }
        self.play = listen(track, self.started);
        self.position = playback.position();
        self.report(Report::Playing, cx);
    }

    /// Reports a pause or a landed seek to the provider, for the track `begin` saw start. A
    /// track restored paused at launch was never reported, so it stays quiet until it plays.
    fn moved(&mut self, cx: &mut Context<Self>) {
        let playback = self.playback.read(cx);
        let id = playback.track().and_then(|track| track.id.as_ref());
        if self.current.is_none() || id != self.current.as_ref() {
            return;
        }
        let report = match playback.state() {
            PlaybackState::Playing => Report::Playing,
            _ => Report::Paused,
        };
        self.position = playback.position();
        self.report(report, cx);
    }

    /// Tells the provider the track stopped where it was last heard, since `Playback` has
    /// already reset its position by the time the end arrives.
    fn end(&mut self, cx: &mut Context<Self>) {
        self.report(Report::Stopped, cx);
        self.play = None;
        self.current = None;
        self.reported = false;
    }

    /// Submits the finished listen to the provider once it has run long enough.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(play) = self.play.as_ref() else {
            return;
        };
        let playback = self.playback.read(cx);
        if playback.track().and_then(|track| track.id.as_ref()) == self.current.as_ref() {
            self.position = playback.position();
        }
        if *playback.state() != PlaybackState::Playing || !play.earned(playback.position()) {
            return;
        }
        if !self.reported
            && self.records(cx)
            && let Some((client, id)) = self.provider(cx)
        {
            self.reported = true;
            let at = play.at;
            self.io.spawn(async move {
                if let Err(error) = client.played(&id, at).await {
                    log::warn!("scrobble: cannot submit the track to its provider: {error:#}");
                }
            });
        }
    }

    /// Whether the active provider asked to keep its own play record. Local files have no
    /// provider account to record against, so they stay silent either way.
    fn records(&self, cx: &App) -> bool {
        let Some(id) = self.current.as_deref() else {
            return false;
        };
        if music::is_local_id(id) {
            return false;
        }
        let Some(provider) = self.session.read(cx).provider_slug() else {
            return false;
        };
        self.settings.read(cx).scrobbles(provider)
    }

    /// Sends the provider the current track's state at the last position heard.
    fn report(&self, report: Report, cx: &App) {
        if !self.records(cx) {
            return;
        }
        let Some((client, id)) = self.provider(cx) else {
            return;
        };
        self.reports.send((client, id, report, self.position)).ok();
    }

    /// The client that serves the current track, and the track's id on it. Local files go to
    /// the local client, which keeps no record and ignores the report.
    fn provider(&self, cx: &App) -> Option<(Arc<dyn MusicApi>, String)> {
        let id = self.current.clone()?;
        let session = self.session.read(cx);
        let client = match music::is_local_id(&id) {
            true => session.local_client(),
            false => session.client(),
        }?;
        Some((client, id))
    }
}

/// Turns a track into a finished listen. A track with no duration cannot earn one.
fn listen(track: &Track, at: SystemTime) -> Option<Listen> {
    (!track.duration.is_zero()).then_some(Listen {
        duration: track.duration,
        at,
    })
}

/// Starts the task that sends state reports in the order they were queued. It ends when the
/// entity drops its sender.
fn send_reports(io: &Io) -> mpsc::UnboundedSender<Reported> {
    let (sender, mut queued) = mpsc::unbounded_channel::<Reported>();
    io.spawn(async move {
        while let Some((client, id, report, position)) = queued.recv().await {
            if let Err(error) = client.report(&id, report, position).await {
                log::warn!("scrobble: cannot report the track to its provider: {error:#}");
            }
        }
    });
    sender
}
