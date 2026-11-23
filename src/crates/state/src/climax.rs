//! Climax marks for the track the player shows.
//!
//! Marks are fetched once per provider and track, kept in a small in-memory cache, and dropped
//! as soon as another track takes the player. A late answer for a track that has since gone is
//! ignored, so marks never land on the wrong song.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{App, Context, Entity, Task};
use music::ClimaxMark;

use crate::{Io, Playback, Session, join, settled};

/// How many tracks' marks are kept before the cache starts over.
const KEPT: usize = 64;

type Key = (String, String);

pub struct Climax {
    playback: Entity<Playback>,
    session: Entity<Session>,
    io: Io,
    current: Option<Key>,
    /// The key whose request is in flight or has been attempted, so a provider that gains its
    /// client later is asked again while a failed one is not retried on every notification.
    requested: Option<Key>,
    marks: Vec<ClimaxMark>,
    cache: HashMap<Key, Vec<ClimaxMark>>,
    seq: u64,
    task: Option<Task<()>>,
}

impl Climax {
    pub fn new(
        playback: Entity<Playback>,
        session: Entity<Session>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&playback, |this, _, cx| this.follow(cx)).detach();
        cx.observe(&session, |this, _, cx| this.follow(cx)).detach();
        let mut climax = Self {
            playback,
            session,
            io,
            current: None,
            requested: None,
            marks: Vec::new(),
            cache: HashMap::new(),
            seq: 0,
            task: None,
        };
        climax.follow(cx);
        climax
    }

    pub fn marks(&self) -> &[ClimaxMark] {
        &self.marks
    }

    /// The normalized start and end fractions of every climax segment, ready for a seek bar.
    pub fn ticks(&self, total: Duration) -> Vec<f32> {
        let total_ms = total.as_millis() as f32;
        if total_ms <= 0. {
            return Vec::new();
        }
        let mut ticks = Vec::new();
        for mark in &self.marks {
            if mark.start_ms as f32 >= total_ms {
                continue;
            }
            let start = mark.start_ms as f32 / total_ms;
            let end = (mark.end_ms as f32 / total_ms).min(1.);
            ticks.push(start.clamp(0., 1.));
            if end > start {
                ticks.push(end);
            }
        }
        ticks
    }

    /// Where a jump-to-climax control should seek: the next climax start after `elapsed`, or the
    /// first one once the track has passed them all.
    pub fn jump_target(&self, elapsed: Duration, total: Duration) -> Option<Duration> {
        let total_ms = total.as_millis() as u64;
        if total_ms == 0 {
            return None;
        }
        let now_ms = elapsed.as_millis() as u64;
        let starts = || {
            self.marks
                .iter()
                .map(|mark| mark.start_ms)
                .filter(|start| *start > 0 && *start < total_ms)
        };
        starts()
            .filter(|start| *start > now_ms)
            .min()
            .or_else(|| starts().min())
            .map(Duration::from_millis)
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let key = self.key(cx);
        if self.current != key {
            self.seq += 1;
            self.task = None;
            self.requested = None;
            self.current = key.clone();
            self.marks = key
                .as_ref()
                .and_then(|key| self.cache.get(key))
                .cloned()
                .unwrap_or_default();
            cx.notify();
        }

        let Some(key) = key else {
            return;
        };
        if self.cache.contains_key(&key) || self.requested.as_ref() == Some(&key) {
            return;
        }
        let Some(client) = self.session.read(cx).client() else {
            return;
        };

        let seq = self.seq;
        let io = self.io.clone();
        let track = key.1.clone();
        self.requested = Some(key.clone());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(io.spawn(async move { client.climax(&track).await })).await;
            this.update(cx, |this, cx| {
                if this.seq != seq || this.current.as_ref() != Some(&key) {
                    return;
                }
                this.task = None;
                match settled(loaded, cx) {
                    Ok(mut marks) => {
                        marks.retain(|mark| mark.start_ms > 0 && mark.end_ms >= mark.start_ms);
                        marks.sort_by_key(|mark| mark.start_ms);
                        marks.dedup_by_key(|mark| mark.start_ms);
                        if this.cache.len() >= KEPT {
                            this.cache.clear();
                        }
                        this.cache.insert(key, marks.clone());
                        this.marks = marks;
                    }
                    Err(reason) => {
                        log::warn!("climax: cannot load the marks: {reason}");
                        this.marks.clear();
                        this.cache.remove(&key);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn key(&self, cx: &App) -> Option<Key> {
        let track = self.playback.read(cx).shown_track(cx)?;
        let id = track.id.as_deref().filter(|id| !music::is_local_id(id))?;
        let slug = self.session.read(cx).provider_slug()?;
        Some((slug.to_owned(), id.to_owned()))
    }
}