//! 侧栏展开动画：参考 FluxDown 的 SidebarMotion，200ms 内同时插值宽度与透明度，
//! 反向切换从当前帧继续，不跳到端点。

use std::time::{Duration, Instant};

use ui::motion::ease_in_out_cubic;

const DURATION: Duration = Duration::from_millis(200);

#[derive(Clone, Copy)]
pub(crate) struct SidebarMotion {
    from: f32,
    to: f32,
    started_at: Option<Instant>,
}

impl SidebarMotion {
    pub(crate) fn settled(open: bool) -> Self {
        let amount = if open { 1. } else { 0. };
        Self {
            from: amount,
            to: amount,
            started_at: None,
        }
    }

    pub(crate) fn needs_retarget(&self, open: bool, animate: bool) -> bool {
        let target = if open { 1. } else { 0. };
        if animate {
            self.to != target
        } else {
            self.from != target || self.to != target || self.started_at.is_some()
        }
    }

    pub(crate) fn retarget(&mut self, open: bool, now: Instant, animate: bool) {
        if !self.needs_retarget(open, animate) {
            return;
        }
        if !animate {
            *self = Self::settled(open);
            return;
        }
        let target = if open { 1. } else { 0. };
        self.from = self.amount(now);
        self.to = target;
        self.started_at = Some(now);
    }

    pub(crate) fn amount(&self, now: Instant) -> f32 {
        let progress = self.started_at.map_or(1., |started| {
            (now.saturating_duration_since(started).as_secs_f32() / DURATION.as_secs_f32())
                .clamp(0., 1.)
        });
        self.from + (self.to - self.from) * ease_in_out_cubic(progress)
    }

    pub(crate) fn is_animating(&self, now: Instant) -> bool {
        self.from != self.to && self.progress(now) < 1.
    }

    fn progress(&self, now: Instant) -> f32 {
        self.started_at.map_or(1., |started| {
            (now.saturating_duration_since(started).as_secs_f32() / DURATION.as_secs_f32())
                .clamp(0., 1.)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverses_from_the_current_frame() {
        let start = Instant::now();
        let mut motion = SidebarMotion::settled(true);
        motion.retarget(false, start, true);
        let halfway = start + DURATION / 2;
        let amount = motion.amount(halfway);
        assert!((amount - 0.5).abs() < 0.01);
        motion.retarget(true, halfway, true);
        assert!((motion.amount(halfway) - amount).abs() < f32::EPSILON);
        assert_eq!(motion.amount(halfway + DURATION), 1.);
    }
}
