// SPDX-License-Identifier: Apache-2.0

//! Animation: the drawn value of something moves toward its logical value over
//! [`ANIMATION_MS`]; the logical value itself changes at once (owner, M16: animation
//! never delays logical state). Time is passed in, so tests run on a fake clock.

use std::time::Duration;

use crate::{geometry::Rect, theme::ANIMATION_MS};

pub const ANIMATION: Duration = Duration::from_millis(ANIMATION_MS);

pub trait Lerp: Copy + PartialEq {
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }
}

impl Lerp for Rect {
    fn lerp(a: Rect, b: Rect, t: f32) -> Rect {
        let mix = |x: i32, y: i32| (x as f32 + (y - x) as f32 * t).round() as i32;
        Rect::new(mix(a.x, b.x), mix(a.y, b.y), mix(a.w, b.w), mix(a.h, b.h))
    }
}

/// A value on its way from `from` to `to`, started at `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween<T> {
    from: T,
    to: T,
    start: Duration,
}

impl<T: Lerp> Tween<T> {
    /// Already at `value`.
    pub fn at(value: T) -> Self {
        Self {
            from: value,
            to: value,
            start: Duration::ZERO,
        }
    }

    /// Where it is drawn at `now` (ease-out cubic).
    pub fn value(&self, now: Duration) -> T {
        let t = self.progress(now);
        if t >= 1.0 {
            return self.to;
        }
        let eased = 1.0 - (1.0 - t).powi(3);
        T::lerp(self.from, self.to, eased)
    }

    fn progress(&self, now: Duration) -> f32 {
        if self.from == self.to {
            return 1.0;
        }
        let elapsed = now.saturating_sub(self.start);
        (elapsed.as_secs_f32() / ANIMATION.as_secs_f32()).min(1.0)
    }

    pub fn target(&self) -> T {
        self.to
    }

    pub fn running(&self, now: Duration) -> bool {
        self.progress(now) < 1.0
    }

    /// Heads for `to` from wherever it is drawn now (an interrupted animation starts
    /// from its current value, never from its old start).
    pub fn retarget(&mut self, to: T, now: Duration) {
        if to == self.to {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.start = now;
    }

    /// At `to` at once, without animating.
    pub fn jump(&mut self, to: T) {
        *self = Self::at(to);
    }
}
