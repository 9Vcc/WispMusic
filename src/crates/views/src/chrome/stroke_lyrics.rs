//! 笔画歌词：把当前句逐字拆成汉字笔画，按演唱时刻逐笔飞入拼合，句尾四散。
//!
//! 笔画数据按字从 hanzi-writer-data 拉取（Make Me a Hanzi，Arphic Public
//! License），内存缓存；没有数据的字符退回静态字。几何与时序逻辑是纯函数，
//! 渲染用 GPUI 的 SVG 蒙版逐笔绘制。

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use anyhow::Result;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Element, ElementId, GlobalElementId, Hsla, IntoElement,
    InspectorElementId, LayoutId, Pixels, Render, ScaledPixels, SharedString, Size, Style,
    Styled, Task, TransformationMatrix, Window, canvas, div, point, px, radians,
};
use music::{Lyrics, LyricsLine};
use state::{AppSettings, Io, Playback};
use ui::ActiveTheme as _;

use super::aside::lyrics_frame_scale;

pub(crate) const STROKE_SCATTER_MS: f64 = 320.;
const CELL_MIN: f32 = 26.;
const CELL_MAX: f32 = 46.;
const COLUMN_HEIGHT_RATIO: f32 = 0.68;
const STAGE_SAFE_RATIO: f32 = 0.1;
const INSTANT_CHAR_BUDGET_MS: f64 = 250.;
const CATCHUP_INSTANT_MS: f64 = 600.;
const STROKE_CACHE_LIMIT: usize = 800;

const FLY_STAGGER_SPACE_MS: f64 = 300.;
const FLY_PATH_NATURAL_MS: f64 = 940.;
const FLY_TAIL_MS: f64 = 260.;
const FLY_TAIL_MIN_MS: f64 = 180.;
const FLY_SCALE_MIN: f64 = 0.35;

const STROKE_CHAR_RE: &[(u32, u32)] = &[
    (0x2E80, 0x2EFF),
    (0x31C0, 0x31EF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
];

pub(crate) fn is_strokable(ch: char) -> bool {
    STROKE_CHAR_RE
        .iter()
        .any(|(from, to)| (*from..=*to).contains(&(ch as u32)))
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Timing {
    pub(crate) text: char,
    pub(crate) trigger_ms: f64,
    pub(crate) span_ms: f64,
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.
}

pub(crate) fn flatten_line(line: &LyricsLine) -> Vec<Timing> {
    let mut timings = Vec::new();
    if let Some(words) = line.words.as_ref().filter(|words| !words.is_empty()) {
        for word in words {
            let chars = word
                .text
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<Vec<_>>();
            if chars.is_empty() {
                continue;
            }
            let span = millis(word.end.saturating_sub(word.start)) / chars.len() as f64;
            for (index, text) in chars.into_iter().enumerate() {
                timings.push(Timing {
                    text,
                    trigger_ms: millis(word.start) + span * index as f64,
                    span_ms: span,
                });
            }
        }
        return timings;
    }

    let chars = line
        .text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<Vec<_>>();
    if chars.is_empty() {
        return timings;
    }
    let line_start = millis(line.start);
    let duration = line
        .end
        .map(|end| millis(end.saturating_sub(line.start)))
        .unwrap_or(chars.len() as f64 * 320.);
    let stagger = (duration / chars.len() as f64).clamp(140., 420.);
    for (index, text) in chars.into_iter().enumerate() {
        timings.push(Timing {
            text,
            trigger_ms: line_start + stagger * index as f64,
            span_ms: stagger,
        });
    }
    timings
}

fn char_interval(timings: &[Timing]) -> f64 {
    match timings.len() {
        0 => f64::INFINITY,
        1 => match timings[0].span_ms {
            span if span > 0. => span,
            _ => f64::INFINITY,
        },
        _ => {
            let interval = (timings[timings.len() - 1].trigger_ms - timings[0].trigger_ms)
                / (timings.len() - 1) as f64;
            if interval > 0. {
                return interval;
            }
            let spans = timings
                .iter()
                .map(|timing| timing.span_ms)
                .filter(|span| *span > 0.)
                .collect::<Vec<_>>();
            match spans.is_empty() {
                true => f64::INFINITY,
                false => spans.iter().sum::<f64>() / spans.len() as f64,
            }
        }
    }
}

pub(crate) fn instant_line(timings: &[Timing]) -> bool {
    char_interval(timings) < INSTANT_CHAR_BUDGET_MS
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Start,
    Center,
    End,
}

struct Column {
    indices: Vec<usize>,
    align: Align,
}

fn split_columns(timings: &[Timing]) -> Vec<Column> {
    let count = match timings.len() {
        0 => 0,
        len if len <= 6 => 1,
        len if len <= 12 => 2,
        _ => 3,
    };
    if count == 0 {
        return Vec::new();
    }
    let per = timings.len().div_ceil(count);
    let mut columns = Vec::with_capacity(count);
    for column in 0..count {
        let from = column * per;
        let to = (from + per).min(timings.len());
        if from >= to {
            break;
        }
        columns.push(Column {
            indices: (from..to).collect(),
            align: match count {
                1 => Align::Center,
                _ if column % 2 == 0 => Align::Start,
                _ => Align::End,
            },
        });
    }
    columns
}

fn cell_size(stage_height: f32, column_length: usize, scale: f32) -> f32 {
    let scale = scale.clamp(0.5, 3.);
    if stage_height <= 0. || column_length == 0 {
        return CELL_MAX * scale;
    }
    let available = stage_height * COLUMN_HEIGHT_RATIO;
    (available / column_length as f32)
        .floor()
        .clamp(CELL_MIN * scale, CELL_MAX * scale)
}

#[derive(Clone, Copy)]
pub(crate) struct Timeline {
    pub(crate) scale: f64,
    pub(crate) tail_ms: f64,
}

pub(crate) fn fit_timeline(budget_ms: f64) -> Timeline {
    let fast_natural = FLY_STAGGER_SPACE_MS + FLY_PATH_NATURAL_MS;
    let natural = fast_natural + FLY_TAIL_MS;
    if budget_ms >= natural {
        return Timeline {
            scale: 1.,
            tail_ms: FLY_TAIL_MS,
        };
    }
    let uniform = budget_ms / natural;
    let tail_pinned = (budget_ms - FLY_TAIL_MIN_MS) / fast_natural;
    let floor = FLY_TAIL_MIN_MS / FLY_TAIL_MS;
    let scale = FLY_SCALE_MIN.max(match uniform >= floor {
        true => uniform,
        false => tail_pinned,
    });
    Timeline {
        scale,
        tail_ms: FLY_TAIL_MIN_MS.max(FLY_TAIL_MS * scale),
    }
}

pub(crate) fn ease_out_cubic(value: f64) -> f64 {
    let invert = 1. - value.clamp(0., 1.);
    1. - invert * invert * invert
}

pub(crate) fn next_random(seed: &mut u64) -> f64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed as f64) / (u64::MAX as f64)
}

// ---------------------------------------------------------------------------
// 笔画数据

pub(crate) struct Hanzi {
    strokes: Vec<HanziStroke>,
}

impl Hanzi {
    pub(crate) fn stroke_count(&self) -> usize {
        self.strokes.len()
    }
}

struct HanziStroke {
    key: SharedString,
    svg: Arc<str>,
}

static HANZI_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap_or_default()
});

pub(crate) async fn fetch_hanzi(ch: char) -> Result<Option<Arc<Hanzi>>> {
    const SOURCES: [&str; 2] = [
        "https://cdn.jsdelivr.net/npm/hanzi-writer-data@2.0.1/",
        "https://unpkg.com/hanzi-writer-data@2.0.1/",
    ];
    let client = &*HANZI_CLIENT;
    for source in SOURCES {
        let url = format!("{source}{}.json", ch);
        let Ok(answer) = client.get(url).send().await else {
            continue;
        };
        if !answer.status().is_success() {
            continue;
        }
        let Ok(payload) = answer.json::<serde_json::Value>().await else {
            continue;
        };
        let Some(entries) = payload.get("strokes").and_then(|value| value.as_array()) else {
            continue;
        };
        let mut strokes = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let path = match entry {
                serde_json::Value::String(path) => path.clone(),
                value => value
                    .get("path")
                    .and_then(|path| path.as_str())
                    .unwrap_or("")
                    .to_owned(),
            };
            if path.is_empty() {
                continue;
            }
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024"><g transform="translate(0,900) scale(1,-1)"><path d="{}" fill="black"/></g></svg>"#,
                path.replace('"', "&quot;").replace('&', "&amp;")
            );
            strokes.push(HanziStroke {
                key: SharedString::from(format!("hanzi-{ch}-{index}")),
                svg: Arc::from(svg.as_str()),
            });
        }
        if !strokes.is_empty() {
            return Ok(Some(Arc::new(Hanzi { strokes })));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// 渲染

#[derive(Clone, Copy)]
pub(crate) struct Pose {
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    pub(crate) rot: f32,
    pub(crate) scale: f32,
    pub(crate) alpha: f32,
}

pub(crate) struct StrokeCell {
    pub(crate) hanzi: Arc<Hanzi>,
    pub(crate) size: Pixels,
    pub(crate) poses: Vec<Pose>,
    pub(crate) color: Hsla,
}

impl IntoElement for StrokeCell {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for StrokeCell {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = self.size.into();
        style.size.height = self.size.into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let factor = window.scale_factor();
        let pivot = bounds.center().scale(factor);
        for (index, stroke) in self.hanzi.strokes.iter().enumerate() {
            let pose = self.poses.get(index).copied().unwrap_or(Pose {
                dx: 0.,
                dy: 0.,
                rot: 0.,
                scale: 1.,
                alpha: 1.,
            });
            if pose.alpha <= 0.002 {
                continue;
            }
            let offset = point(
                ScaledPixels(pose.dx * factor),
                ScaledPixels(pose.dy * factor),
            );
            let turn = TransformationMatrix::unit()
                .translate(offset)
                .translate(pivot)
                .rotate(radians(pose.rot))
                .scale(Size::new(pose.scale, pose.scale))
                .translate(pivot.map(|inset| ScaledPixels(-inset.0)));
            window
                .paint_svg(
                    bounds,
                    stroke.key.clone(),
                    Some(stroke.svg.as_bytes()),
                    turn,
                    self.color.opacity(pose.alpha),
                    cx,
                )
                .ok();
        }
    }
}

// ---------------------------------------------------------------------------
// 视图

pub(crate) struct FlyStroke {
    pub(crate) delay_ms: f64,
    pub(crate) duration_ms: f64,
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    pub(crate) rot: f32,
    pub(crate) scale: f32,
}

struct Fly {
    start: Instant,
    strokes: Vec<FlyStroke>,
    end: Instant,
}

struct Scatter {
    start: Instant,
    strokes: Vec<Pose>,
}

struct Char {
    timing: Timing,
    data: Option<Arc<Hanzi>>,
    no_data: bool,
    fallback_at: Option<Instant>,
    fly: Option<Fly>,
    assembled: bool,
    scatter: Option<Scatter>,
}

struct Scene {
    index: usize,
    line: LyricsLine,
    timings: Vec<Timing>,
    columns: Vec<Column>,
    instant: bool,
    fly_budget: f64,
    chars: Vec<Char>,
    cell: f32,
    safe: f32,
    phase: Phase,
    scatter_started: Option<Instant>,
    max_fly_end: Option<Instant>,
    translations: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    In,
    Out,
}

pub(crate) struct StrokeLyrics {
    lyrics: gpui::Entity<state::Lyrics>,
    playback: gpui::Entity<Playback>,
    settings: gpui::Entity<AppSettings>,
    stage: Rc<Cell<Bounds<Pixels>>>,
    stage_height: f32,
    titled: bool,
    scale: f32,
    revision: u64,
    cache: HashMap<char, Option<Arc<Hanzi>>>,
    fetching: HashSet<char>,
    tasks: Vec<Task<()>>,
    scene: Option<Scene>,
    pending: Option<usize>,
    switch_at: Option<Instant>,
    out_at: Option<Instant>,
}

impl StrokeLyrics {
    pub(crate) fn new(
        lyrics: gpui::Entity<state::Lyrics>,
        playback: gpui::Entity<Playback>,
        settings: gpui::Entity<AppSettings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let ticks = playback.read(cx).ticks().clone();
        cx.observe(&ticks, |_, _, cx| cx.notify()).detach();
        cx.observe(&lyrics, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            lyrics,
            playback,
            settings,
            stage: Rc::new(Cell::new(Bounds::default())),
            stage_height: 0.,
            titled: true,
            scale: 1.,
            revision: 0,
            cache: HashMap::new(),
            fetching: HashSet::new(),
            tasks: Vec::new(),
            scene: None,
            pending: None,
            switch_at: None,
            out_at: None,
        }
    }

    pub(super) fn set_titled(&mut self, titled: bool, cx: &mut Context<Self>) {
        self.titled = titled;
        cx.notify();
    }

    fn current_scale(&self, window: &Window, cx: &App) -> f32 {
        let settings = self.settings.read(cx);
        match self.titled {
            true => settings.panel_lyrics_scale(),
            false => settings.fullscreen_lyrics_scale() * lyrics_frame_scale(window.viewport_size()),
        }
    }

    fn position_ms(&self, cx: &App) -> f64 {
        let position = self.playback.read(cx).live_position();
        let offset = self.lyrics.read(cx).offset_ms();
        let position = match offset {
            0 => position,
            ms if ms > 0 => position + Duration::from_millis(ms as u64),
            _ => position.saturating_sub(Duration::from_millis((-offset) as u64)),
        };
        millis(position)
    }

    fn current_lines(&self, cx: &App) -> Option<Arc<[LyricsLine]>> {
        let lyrics = self.lyrics.read(cx);
        match &lyrics.current()?.lyrics {
            Lyrics::Synced { lines } => Some(lines.clone()),
            _ => None,
        }
    }

    fn request(&mut self, ch: char, cx: &mut Context<Self>) {
        if !is_strokable(ch) || self.cache.contains_key(&ch) || self.fetching.contains(&ch) {
            return;
        }
        self.fetching.insert(ch);
        let io = Io::global(cx);
        let task = io.load(
            async move { fetch_hanzi(ch).await },
            move |this: &mut Self, answer, cx| {
                this.fetching.remove(&ch);
                let data = answer.ok().flatten();
                this.cache.insert(ch, data.clone());
                if this.cache.len() > STROKE_CACHE_LIMIT {
                    if let Some(drop) = this.cache.keys().copied().find(|key| *key != ch) {
                        this.cache.remove(&drop);
                    }
                }
                if let Some(scene) = this.scene.as_mut() {
                    for item in &mut scene.chars {
                        if item.timing.text == ch && item.data.is_none() {
                            item.data = data.clone();
                            item.no_data = data.is_none();
                        }
                    }
                }
                this.arm_chars(cx);
                cx.notify();
            },
            cx,
        );
        self.tasks.push(task);
    }

    fn arm_chars(&mut self, cx: &mut Context<Self>) {
        let now_ms = self.position_ms(cx);
        let now = Instant::now();
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        if scene.phase != Phase::In {
            return;
        }
        for index in 0..scene.chars.len() {
            let char = &mut scene.chars[index];
            if char.fly.is_some() || char.assembled {
                continue;
            }
            let delay = char.timing.trigger_ms - now_ms;
            if char.data.is_some() {
                if scene.instant {
                    char.assembled = true;
                    continue;
                }
                if delay < -CATCHUP_INSTANT_MS {
                    char.assembled = true;
                    continue;
                }
                let elapsed = (-delay).max(0.);
                let timeline = fit_timeline((scene.fly_budget - elapsed).max(0.));
                let start = now + Duration::from_secs_f64(delay.max(0.) / 1000.);
                let data = char.data.as_ref().expect("the data was just checked");
                let mut seed = (char.timing.trigger_ms as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(index as u64);
                let (strokes, end) = fly_strokes(data, &mut seed, timeline, start);
                scene.max_fly_end = Some(match scene.max_fly_end {
                    Some(end_at) => end_at.max(end),
                    None => end,
                });
                char.fly = Some(Fly {
                    start,
                    strokes,
                    end,
                });
            } else if char.no_data && char.fallback_at.is_none() {
                let at = match scene.instant {
                    true => now,
                    false => now + Duration::from_secs_f64(delay.max(0.) / 1000.),
                };
                char.fallback_at = Some(at);
            }
        }
    }

    fn build(&mut self, index: usize, lines: &[LyricsLine], cx: &mut Context<Self>) {
        let Some(line) = lines.get(index) else {
            self.scene = None;
            return;
        };
        let timings = flatten_line(line);
        let columns = split_columns(&timings);
        let next_start = lines
            .get(index + 1)
            .map(|next| millis(next.start))
            .unwrap_or_else(|| millis(line.start) + 5000.);
        let line_window = (next_start - millis(line.start)).max(600.);
        let last_offset = timings
            .last()
            .map(|timing| timing.trigger_ms - millis(line.start))
            .unwrap_or(0.);
        let fly_budget = (line_window - 120. - last_offset).max(0.);
        let max_column = columns
            .iter()
            .map(|column| column.indices.len())
            .max()
            .unwrap_or(1);
        let cell = cell_size(self.stage_height, max_column, self.scale);
        let safe = self.stage_height * STAGE_SAFE_RATIO;
        let translations = line
            .secondary
            .first()
            .map(|lane| lane.text.clone())
            .into_iter()
            .collect::<Vec<_>>();
        let chars = timings
            .iter()
            .map(|timing| Char {
                timing: timing.clone(),
                data: self.cache.get(&timing.text).cloned().flatten(),
                no_data: !is_strokable(timing.text)
                    || self.cache.get(&timing.text).is_some_and(|data| data.is_none()),
                fallback_at: None,
                fly: None,
                assembled: false,
                scatter: None,
            })
            .collect::<Vec<_>>();
        self.scene = Some(Scene {
            index,
            line: line.clone(),
            timings,
            columns,
            instant: instant_line(&chars.iter().map(|char| char.timing.clone()).collect::<Vec<_>>()),
            fly_budget,
            chars,
            cell,
            safe,
            phase: Phase::In,
            scatter_started: None,
            max_fly_end: None,
            translations,
        });
        self.pending = None;
        self.switch_at = None;
        self.out_at = None;
        let chars = self
            .scene
            .as_ref()
            .map(|scene| {
                scene
                    .timings
                    .iter()
                    .map(|timing| timing.text)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for ch in chars {
            self.request(ch, cx);
        }
        if let Some(next) = lines.get(index + 1) {
            for ch in next.text.chars() {
                self.request(ch, cx);
            }
        }
        self.arm_chars(cx);
    }

    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(lines) = self.current_lines(cx) else {
            self.scene = None;
            return false;
        };
        let revision = self.lyrics.read(cx).revision();
        if revision != self.revision {
            self.revision = revision;
            self.scene = None;
        }
        if self.scene.is_none() {
            let position = self.position_ms(cx);
            let Some(index) = lines
                .iter()
                .rposition(|line| millis(line.start) <= position)
            else {
                return false;
            };
            self.build(index, &lines, cx);
        }
        let position = self.position_ms(cx);
        let Some(index) = lines
            .iter()
            .rposition(|line| millis(line.start) <= position)
        else {
            return false;
        };
        let now = Instant::now();
        if let Some(scene) = self.scene.as_mut()
            && index != scene.index
        {
            self.pending = Some(index);
        }
        let Some(scene) = self.scene.as_mut() else {
            return false;
        };
        match scene.phase {
            Phase::In => {
                if self.pending.is_some() {
                    let wait = scene
                        .max_fly_end
                        .map(|end| end.saturating_duration_since(now).as_secs_f64() * 1000.)
                        .unwrap_or(0.)
                        .min(2000.);
                    let switch_at = *self
                        .switch_at
                        .get_or_insert_with(|| now + Duration::from_secs_f64(wait / 1000.));
                    if now >= switch_at {
                        scene.phase = Phase::Out;
                        scene.scatter_started = Some(now);
                        self.out_at = Some(now + Duration::from_secs_f64(STROKE_SCATTER_MS / 1000.));
                        self.switch_at = None;
                        scatter_scene(scene, now);
                    }
                } else {
                    self.switch_at = None;
                }
            }
            Phase::Out => {
                if self.out_at.is_some_and(|at| now >= at) {
                    let next = self.pending.take().unwrap_or(index);
                    self.build(next, &lines, cx);
                }
            }
        }
        let Some(scene) = self.scene.as_mut() else {
            return false;
        };
        let animating = match scene.phase {
            Phase::Out => true,
            Phase::In => scene.chars.iter().any(|char| {
                char.fly.as_ref().is_some_and(|fly| fly.end > now)
                    || (char.fallback_at.is_some_and(|at| at > now))
            }),
        };
        animating
    }
}

pub(crate) fn fly_strokes(
    data: &Hanzi,
    seed: &mut u64,
    timeline: Timeline,
    start: Instant,
) -> (Vec<FlyStroke>, Instant) {
    let count = data.strokes.len();
    let step = match count {
        0 | 1 => 0.,
        _ => FLY_STAGGER_SPACE_MS / (count - 1) as f64,
    };
    let mut slots = (0..count).collect::<Vec<_>>();
    for index in (1..slots.len()).rev() {
        let swap = (next_random(seed) * (index + 1) as f64) as usize;
        slots.swap(index, swap.min(index));
    }
    let mut strokes = Vec::with_capacity(count);
    let mut end = start;
    for (slot, index) in slots.into_iter().enumerate() {
        let ang = next_random(seed) * std::f64::consts::TAU;
        let distance = 500. + next_random(seed) * 260.;
        let depth = next_random(seed);
        let rot = (next_random(seed) * 2. - 1.) * (24. + 48. * depth);
        let delay = (slot as f64 * step
            + next_random(seed) * step * 0.6)
            * timeline.scale;
        let duration = (FLY_PATH_NATURAL_MS
            * timeline.scale
            * (0.85 + next_random(seed) * 0.15))
            + timeline.tail_ms;
        let end_at = start + Duration::from_secs_f64((delay + duration) / 1000.);
        end = end.max(end_at);
        strokes.push(FlyStroke {
            delay_ms: delay,
            duration_ms: duration,
            dx: (ang.cos() * distance) as f32,
            dy: (ang.sin() * distance) as f32,
            rot: rot.to_radians() as f32,
            scale: (0.62 + 0.26 * depth) as f32,
        });
        let _ = index;
    }
    (strokes, end)
}

fn scatter_scene(scene: &mut Scene, now: Instant) {
    for (index, char) in scene.chars.iter_mut().enumerate() {
        if char.data.is_none() {
            continue;
        }
        let mut seed = (char.timing.trigger_ms as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(index as u64)
            .wrapping_add(0x5DEE_CE66);
        let mut poses = Vec::with_capacity(char.data.as_ref().map_or(0, |data| data.strokes.len()));
        if let Some(data) = char.data.as_ref() {
            for _ in 0..data.strokes.len() {
                let depth = next_random(&mut seed);
                let ang = next_random(&mut seed) * std::f64::consts::TAU;
                let radius = 120. + 360. * depth;
                let rot = (next_random(&mut seed) * 2. - 1.) * (24. + 48. * depth);
                poses.push(Pose {
                    dx: (ang.cos() * radius) as f32,
                    dy: (ang.sin() * radius) as f32,
                    rot: rot.to_radians() as f32,
                    scale: (0.78 + 0.36 * (1. - depth)) as f32,
                    alpha: 1.,
                });
            }
        }
        char.scatter = Some(Scatter {
            start: now,
            strokes: poses,
        });
    }
}

fn char_poses(char: &Char, phase: Phase, now: Instant, scene: &Scene) -> (Vec<Pose>, f32) {
    match phase {
        Phase::Out => {
            let Some(scatter) = char.scatter.as_ref() else {
                let progress = ((now
                    .saturating_duration_since(scene.scatter_started.unwrap_or(now))
                    .as_secs_f64()
                    * 1000.)
                    / STROKE_SCATTER_MS)
                    .clamp(0., 1.);
                return (Vec::new(), (1. - progress) as f32);
            };
            let progress = ((now.saturating_duration_since(scatter.start).as_secs_f64() * 1000.)
                / STROKE_SCATTER_MS)
                .clamp(0., 1.);
            let eased = progress * progress;
            let poses = scatter
                .strokes
                .iter()
                .map(|pose| Pose {
                    dx: pose.dx * eased as f32,
                    dy: pose.dy * eased as f32,
                    rot: pose.rot * eased as f32,
                    scale: 1. + (pose.scale - 1.) * eased as f32,
                    alpha: (1. - progress) as f32,
                })
                .collect();
            (poses, (1. - progress) as f32)
        }
        Phase::In => {
            if let Some(fly) = char.fly.as_ref() {
                let elapsed = now.saturating_duration_since(fly.start).as_secs_f64() * 1000.;
                let poses = fly
                    .strokes
                    .iter()
                    .map(|stroke| {
                        let progress = ((elapsed - stroke.delay_ms) / stroke.duration_ms)
                            .clamp(0., 1.);
                        let eased = ease_out_cubic(progress);
                        Pose {
                            dx: stroke.dx * (1. - eased as f32),
                            dy: stroke.dy * (1. - eased as f32),
                            rot: stroke.rot * (1. - eased as f32),
                            scale: stroke.scale + (1. - stroke.scale) * eased as f32,
                            alpha: eased as f32,
                        }
                    })
                    .collect();
                return (poses, 1.);
            }
            if char.assembled {
                let poses = char
                    .data
                    .as_ref()
                    .map(|data| {
                        data.strokes
                            .iter()
                            .map(|_| Pose {
                                dx: 0.,
                                dy: 0.,
                                rot: 0.,
                                scale: 1.,
                                alpha: 1.,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return (poses, 1.);
            }
            if let Some(at) = char.fallback_at {
                return (Vec::new(), if now >= at { 1. } else { 0. });
            }
            let _ = scene;
            (Vec::new(), 0.)
        }
    }
}

impl Render for StrokeLyrics {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.stage.get();
        if bounds.size.height > px(0.) && bounds.size.height.as_f32() != self.stage_height {
            self.stage_height = bounds.size.height.as_f32();
        }
        self.scale = self.current_scale(window, cx);
        if let Some(scene) = self.scene.as_mut() {
            let max_column = scene
                .columns
                .iter()
                .map(|column| column.indices.len())
                .max()
                .unwrap_or(1);
            let cell = cell_size(self.stage_height, max_column, self.scale);
            if (cell - scene.cell).abs() > 0.5 {
                scene.cell = cell;
                scene.safe = self.stage_height * STAGE_SAFE_RATIO;
            }
        }
        let animating = self.tick(cx);
        if animating {
            window.request_animation_frame();
        }
        let theme = *cx.theme();
        let font = SharedString::from(self.settings.read(cx).font().to_owned());
        let stage = self.stage.clone();
        let stage_height = Rc::new(Cell::new(self.stage_height));
        let mut stage_view = div().relative().size_full();
        let now = Instant::now();
        if let Some(scene) = self.scene.as_ref() {
            let cell = px(scene.cell);
            let cell_gap = px((scene.cell * 0.16).max(6.));
            let column_gap = px((scene.cell * 0.5).max(28.));
            let color = theme.foreground;
            let mut columns = div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .gap(column_gap);
            for column in &scene.columns {
                let mut body = div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .h_full()
                    .gap(cell_gap);
                body = match column.align {
                    Align::Start => body.justify_start().pt(px(scene.safe)),
                    Align::End => body.justify_end().pb(px(scene.safe)),
                    Align::Center => body.justify_center(),
                };
                for index in &column.indices {
                    let char = &scene.chars[*index];
                    let (poses, fallback_alpha) =
                        char_poses(char, scene.phase, now, scene);
                    let cell_element = match char.data.as_ref() {
                        Some(data) => StrokeCell {
                            hanzi: data.clone(),
                            size: cell,
                            poses,
                            color,
                        }
                        .into_any_element(),
                        None => div()
                            .w(cell)
                            .h(cell)
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(fallback_alpha)
                            .text_size(cell * 0.62)
                            .text_color(color)
                            .font_family(font.clone())
                            .child(SharedString::from(char.timing.text.to_string()))
                            .into_any_element(),
                    };
                    body = body.child(cell_element);
                }
                columns = columns.child(body);
            }
            let mut root = div()
                .relative().id("stroke-lyrics-stage")
                .size_full()
                .cursor_pointer()
                .child(columns)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(scene) = this.scene.as_ref() {
                        let start = scene.line.start;
                        this.playback
                            .update(cx, |playback, cx| playback.seek(start, cx));
                    }
                }));
            if !scene.translations.is_empty() {
                root = root.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(scene.safe.max(16.)))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .px_8()
                        .children(scene.translations.iter().map(|text| {
                            div()
                                .truncate()
                                .text_size(theme.text(ui::Text::Body))
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(text.clone()))
                        })),
                );
            }
            stage_view = stage_view.child(root);
        }
        stage_view = stage_view.child(
            canvas(
                move |bounds, window, _| {
                    if bounds.size.height != px(stage_height.get()) {
                        stage_height.set(bounds.size.height.as_f32());
                        stage.set(bounds);
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
        stage_view
    }
}

// ---------------------------------------------------------------------------
// 测试

#[cfg(test)]
mod tests {
    use super::*;
    use music::LyricsWord;

    fn line(start: u64, end: Option<u64>, text: &str, words: Option<Vec<LyricsWord>>) -> LyricsLine {
        LyricsLine {
            start: Duration::from_millis(start),
            end: end.map(Duration::from_millis),
            text: text.to_owned(),
            romanized: None,
            words,
            secondary: Vec::new(),
            voice: music::Voice::Lead,
        }
    }

    #[test]
    fn word_timings_split_evenly_within_a_word() {
        let words = vec![LyricsWord {
            start: Duration::from_millis(1000),
            end: Duration::from_millis(1600),
            text: "你好".to_owned(),
            real_timed: true,
        }];
        let timings = flatten_line(&line(1000, Some(2000), "你好", Some(words)));
        assert_eq!(timings.len(), 2);
        assert_eq!(timings[0].trigger_ms, 1000.);
        assert_eq!(timings[1].trigger_ms, 1300.);
    }

    #[test]
    fn lrc_timings_are_spread_over_the_line() {
        let timings = flatten_line(&line(0, Some(1200), "一二三四", None));
        assert_eq!(timings.len(), 4);
        assert_eq!(timings[1].trigger_ms, 300.);
        assert!(!instant_line(&timings));
    }

    #[test]
    fn fast_lines_skip_the_fly_in() {
        let words = (0..4)
            .map(|index| LyricsWord {
                start: Duration::from_millis(index * 100),
                end: Duration::from_millis(index * 100 + 100),
                text: "字".to_owned(),
                real_timed: true,
            })
            .collect();
        let timings = flatten_line(&line(0, Some(400), "字字字字", Some(words)));
        assert!(instant_line(&timings));
    }

    #[test]
    fn columns_split_and_stagger_the_alignment() {
        let timings = (0..12)
            .map(|index| Timing {
                text: '字',
                trigger_ms: index as f64 * 300.,
                span_ms: 300.,
            })
            .collect::<Vec<_>>();
        let columns = split_columns(&timings);
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].indices.len(), 6);
        assert!(matches!(columns[0].align, Align::Start));
        assert!(matches!(columns[1].align, Align::End));
    }

    #[test]
    fn cells_stay_readable_and_fit_the_stage() {
        assert_eq!(cell_size(560., 10, 1.), 38.);
        assert_eq!(cell_size(80., 10, 1.), CELL_MIN);
        assert_eq!(cell_size(560., 2, 1.), CELL_MAX);
        assert!(cell_size(560., 2, 1.2) > cell_size(560., 2, 1.));
    }

    #[test]
    fn the_timeline_keeps_a_tail_for_fast_lines() {
        let natural = fit_timeline(10_000.);
        assert_eq!(natural.scale, 1.);
        assert_eq!(natural.tail_ms, FLY_TAIL_MS);
        let tight = fit_timeline(300.);
        assert!(tight.scale < 1.);
        assert!(tight.tail_ms >= FLY_TAIL_MIN_MS);
    }
}