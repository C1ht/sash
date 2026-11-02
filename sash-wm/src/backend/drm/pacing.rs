// SPDX-License-Identifier: Apache-2.0

//! When the DRM backend renders: only after the previous page flip has completed, only
//! while the session is active and the connector is connected, and only if a frame is
//! wanted. Frame callbacks go out when a flip completes (the frame is on screen), not
//! when it is queued.
//!
//! Flips are normally paced by vblank: they complete one refresh period apart at the
//! earliest. Some devices (virtio-gpu) complete them at once. Two flips that complete
//! less than half a refresh period apart mark the output as unpaced; from then on frames
//! follow a fixed schedule of one refresh period: each frame has a slot, and the next
//! may start at the earliest one period after that slot. A frame that starts late (a
//! timer fires late, or input arrives between slots) but within the next period takes
//! the next slot, so lateness does not add up and a steady stream runs at the nominal
//! refresh rate. After an idle gap the schedule restarts at the frame's start. Slots
//! are measured from the start of rendering, not its end. With vblank pacing nothing is
//! delayed.
//!
//! Times are monotonic clock readings passed in by the caller, so the tests use fake
//! ones.

use std::time::Duration;

/// What the backend should do about the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Render now.
    Render,
    /// Render at this time (a timer); unpaced output only.
    RenderAt(Duration),
    /// Nothing to do until something changes or a flip completes.
    Wait,
}

#[derive(Debug)]
pub struct Pacer {
    /// One refresh period at the output's mode.
    refresh: Duration,
    /// A queued frame waits for its page flip.
    flip_pending: bool,
    /// Something changed since the last frame.
    wanted: bool,
    /// The session (VT) is ours.
    active: bool,
    /// The connector has a display.
    connected: bool,
    /// Flips complete without vblank pacing.
    unpaced: bool,
    /// When the last flip completed (the kernel's timestamp when it has one).
    last_flip: Option<Duration>,
    /// The schedule slot of the last queued frame (unpaced output): when it was due.
    slot: Option<Duration>,
}

impl Pacer {
    pub fn new(refresh: Duration) -> Self {
        Self {
            refresh,
            flip_pending: false,
            wanted: true,
            active: true,
            connected: true,
            unpaced: false,
            last_flip: None,
            slot: None,
        }
    }

    /// A frame is wanted at `now`.
    pub fn want_frame(&mut self, now: Duration) -> Next {
        self.wanted = true;
        self.next(now)
    }

    /// The backend rendered a frame it started at `started`: `queued` if a frame went to
    /// the display (it will flip), false if there was nothing to show or rendering
    /// failed.
    pub fn rendered(&mut self, queued: bool, started: Duration) {
        self.wanted = false;
        self.flip_pending = queued;
        if queued {
            self.slot = Some(match self.slot {
                // On schedule, or late by less than a period: the next slot.
                Some(slot)
                    if started >= slot + self.refresh && started < slot + 2 * self.refresh =>
                {
                    slot + self.refresh
                }
                // The first frame, or after an idle gap: the schedule starts here.
                _ => started,
            });
        }
    }

    /// The page flip of the queued frame completed at `flipped` (the kernel's
    /// timestamp, or the time it was seen); `now` is the current time. A late flip
    /// event after a pause or a disconnect is ignored.
    pub fn flip_complete(&mut self, flipped: Duration, now: Duration) -> Next {
        if !self.active || !self.connected {
            return Next::Wait;
        }
        self.flip_pending = false;
        if let Some(previous) = self.last_flip
            && flipped.saturating_sub(previous) < self.refresh / 2
        {
            self.unpaced = true;
        }
        self.last_flip = Some(flipped);
        self.next(now)
    }

    /// The timer of a `RenderAt` fired.
    pub fn timer_fired(&mut self, now: Duration) -> Next {
        self.next(now)
    }

    /// The session was paused (VT switch away): no rendering, and a pending flip will
    /// not complete.
    pub fn pause(&mut self) {
        self.active = false;
        self.flip_pending = false;
    }

    /// The session is ours again: the whole output must be redrawn. Returns true if the
    /// backend should render now (false while the connector is disconnected).
    pub fn resume(&mut self) -> bool {
        self.active = true;
        self.restart()
    }

    /// The connector lost its display (false) or has one again (true). Returns true if
    /// the backend should render now (connected again while the session is active).
    pub fn set_connected(&mut self, connected: bool) -> bool {
        self.connected = connected;
        if connected {
            self.restart()
        } else {
            self.flip_pending = false;
            false
        }
    }

    pub fn is_active(&self) -> bool {
        self.active && self.connected
    }

    /// The mode changed (a reconnect at another mode).
    pub fn set_refresh(&mut self, refresh: Duration) {
        self.refresh = refresh;
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn is_unpaced(&self) -> bool {
        self.unpaced
    }

    /// A pending flip is forgotten and everything is redrawn; the flip history starts
    /// over (the device was away).
    fn restart(&mut self) -> bool {
        self.flip_pending = false;
        self.wanted = true;
        self.last_flip = None;
        self.is_active()
    }

    fn next(&self, now: Duration) -> Next {
        if !self.is_active() || !self.wanted || self.flip_pending {
            return Next::Wait;
        }
        match self.slot {
            Some(slot) if self.unpaced && now < slot + self.refresh => {
                Next::RenderAt(slot + self.refresh)
            }
            _ => Next::Render,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 60 Hz: 16.666 ms.
    const REFRESH: Duration = Duration::from_micros(16_666);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn pacer() -> Pacer {
        Pacer::new(REFRESH)
    }

    #[test]
    fn the_first_frame_renders_at_once() {
        assert_eq!(pacer().want_frame(ms(0)), Next::Render);
    }

    #[test]
    fn no_frame_while_a_flip_is_pending() {
        let mut pacer = pacer();
        assert_eq!(pacer.want_frame(ms(0)), Next::Render);
        pacer.rendered(true, ms(0));
        assert_eq!(pacer.want_frame(ms(1)), Next::Wait, "flip pending");
        assert_eq!(pacer.want_frame(ms(2)), Next::Wait, "still pending");
        assert_eq!(
            pacer.flip_complete(ms(16), ms(16)),
            Next::Render,
            "the wanted frame renders after the flip"
        );
        pacer.rendered(true, ms(16));
        assert_eq!(
            pacer.flip_complete(ms(33), ms(33)),
            Next::Wait,
            "nothing new wanted"
        );
    }

    #[test]
    fn an_empty_frame_does_not_wait_for_a_flip() {
        let mut pacer = pacer();
        assert_eq!(pacer.want_frame(ms(0)), Next::Render);
        pacer.rendered(false, ms(0));
        assert_eq!(pacer.want_frame(ms(1)), Next::Render);
    }

    #[test]
    fn nothing_renders_while_paused() {
        let mut pacer = pacer();
        pacer.pause();
        assert_eq!(pacer.want_frame(ms(0)), Next::Wait);
        assert_eq!(pacer.flip_complete(ms(1), ms(1)), Next::Wait);
        assert!(!pacer.is_active());
    }

    #[test]
    fn a_vt_switch_while_a_flip_is_pending() {
        let mut pacer = pacer();
        assert_eq!(pacer.want_frame(ms(0)), Next::Render);
        pacer.rendered(true, ms(0));
        pacer.pause();
        // The flip event arrives late, after the pause: ignored.
        assert_eq!(pacer.flip_complete(ms(16), ms(16)), Next::Wait);
        assert_eq!(pacer.want_frame(ms(20)), Next::Wait);
        // Back on our VT: the pending flip is forgotten, a full frame renders at once.
        assert!(pacer.resume());
        assert_eq!(pacer.want_frame(ms(5000)), Next::Render);
        pacer.rendered(true, ms(5000));
        assert_eq!(pacer.want_frame(ms(5001)), Next::Wait);
        assert_eq!(pacer.flip_complete(ms(5016), ms(5016)), Next::Render);
    }

    #[test]
    fn resume_always_redraws() {
        let mut pacer = pacer();
        assert_eq!(pacer.want_frame(ms(0)), Next::Render);
        pacer.rendered(false, ms(0));
        pacer.pause();
        pacer.pause(); // twice is harmless
        assert!(pacer.resume());
        assert!(pacer.is_active());
    }

    #[test]
    fn disconnect_stops_and_reconnect_redraws() {
        let mut pacer = pacer();
        assert_eq!(pacer.want_frame(ms(0)), Next::Render);
        pacer.rendered(true, ms(0));
        // Cable pulled with a flip pending: nothing renders, the flip may never come.
        assert!(!pacer.set_connected(false));
        assert!(!pacer.is_active());
        assert_eq!(pacer.want_frame(ms(5)), Next::Wait);
        assert_eq!(pacer.flip_complete(ms(16), ms(16)), Next::Wait);
        // A VT switch away and back while disconnected: still nothing.
        pacer.pause();
        assert!(!pacer.resume());
        assert_eq!(pacer.want_frame(ms(100)), Next::Wait);
        // Plugged in again: a full frame at once, not waiting for the lost flip.
        assert!(pacer.set_connected(true));
        assert_eq!(pacer.want_frame(ms(200)), Next::Render);
        // Reconnected while away on another VT: rendering waits for the resume.
        pacer.pause();
        assert!(!pacer.set_connected(false));
        assert!(!pacer.set_connected(true));
        assert!(pacer.resume());
    }

    /// Hardware: flips complete at vblank, one refresh apart. Input right after a flip
    /// renders at once; nothing is ever delayed by the cap.
    #[test]
    fn vblank_paced_flips_are_never_delayed() {
        let mut pacer = pacer();
        let vblank = |n: u32| REFRESH * n;
        assert_eq!(pacer.want_frame(ms(0)), Next::Render);
        pacer.rendered(true, ms(0));
        for n in 1..=120 {
            // Flip n completes at vblank n; something new is wanted every frame.
            assert_eq!(pacer.want_frame(vblank(n) - ms(2)), Next::Wait);
            assert_eq!(
                pacer.flip_complete(vblank(n), vblank(n) + ms(1)),
                Next::Render
            );
            pacer.rendered(true, vblank(n) + ms(1));
        }
        assert!(!pacer.is_unpaced());
        // Idle, then input 2 ms after a vblank: rendered at once.
        assert_eq!(pacer.flip_complete(vblank(121), vblank(121)), Next::Wait);
        assert_eq!(pacer.want_frame(vblank(121) + ms(2)), Next::Render);
    }

    /// virtio-gpu: a flip completes right after it is queued. Without the cap a stream
    /// of input renders far above the refresh rate; with it, frames are one refresh
    /// period apart.
    #[test]
    fn unpaced_flips_are_capped_at_the_refresh_rate() {
        let mut pacer = pacer();
        let mut now = ms(0);
        let mut rendered = Vec::new();
        let mut timer: Option<Duration> = None;
        // Input every 4 ms for one second; a flip completes 0.5 ms after each frame.
        let mut flip_at: Option<Duration> = None;
        let step = Duration::from_micros(500);
        while now < ms(1000) {
            if let Some(at) = flip_at
                && now >= at
            {
                flip_at = None;
                match pacer.flip_complete(at, now) {
                    Next::Render => {
                        pacer.rendered(true, now);
                        rendered.push(now);
                        flip_at = Some(now + step);
                    }
                    Next::RenderAt(at) => timer = Some(at),
                    Next::Wait => {}
                }
            }
            if let Some(at) = timer
                && now >= at
            {
                timer = None;
                if pacer.timer_fired(now) == Next::Render {
                    pacer.rendered(true, now);
                    rendered.push(now);
                    flip_at = Some(now + step);
                }
            }
            if now.as_micros().is_multiple_of(4000) {
                match pacer.want_frame(now) {
                    Next::Render => {
                        pacer.rendered(true, now);
                        rendered.push(now);
                        flip_at = Some(now + step);
                    }
                    Next::RenderAt(at) => timer = Some(at),
                    Next::Wait => {}
                }
            }
            now += step;
        }
        assert!(pacer.is_unpaced());
        // The first two frames come before the output is known to be unpaced; after
        // that, frames are one refresh period apart, give or take the 0.5 ms steps of
        // this simulation (a late frame keeps its slot, so the next may come early).
        let intervals: Vec<Duration> = rendered.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            intervals[1..]
                .iter()
                .all(|&i| i + step >= REFRESH && i <= REFRESH + step),
            "{intervals:?}"
        );
        // And the rate is 60 Hz.
        assert!(
            (59..=61).contains(&rendered.len()),
            "{} frames in one second",
            rendered.len()
        );
    }

    /// A full-rate stream on an unpaced output, as in the VM: input every millisecond,
    /// each frame takes 3 ms to render, the cap's timer fires 0.7 ms late, and the flip
    /// completes 0.5 ms after the frame is queued. Slots are kept from the start of
    /// rendering, so neither the render time nor the timer's lateness lowers the rate:
    /// 600 frames in 10 s, at the nominal 60 Hz. (Spacing from the end of the previous
    /// render, as before, gave one frame per 16.666 + 3 + 0.7 ms: about 49 Hz.)
    #[test]
    fn a_full_rate_stream_runs_at_the_nominal_refresh_rate() {
        let tick = Duration::from_micros(100);
        let render_time = ms(3);
        let timer_late = Duration::from_micros(700);
        let flip_delay = Duration::from_micros(500);
        let mut pacer = pacer();
        let mut starts: Vec<Duration> = Vec::new();
        let mut timer: Option<Duration> = None;
        let mut rendering_until: Option<Duration> = None;
        let mut flip_at: Option<Duration> = None;
        let mut now = Duration::ZERO;
        let end = Duration::from_secs(10);
        let start_frame =
            |now: Duration, starts: &mut Vec<Duration>, rendering: &mut Option<Duration>| {
                starts.push(now);
                *rendering = Some(now + render_time);
            };
        while now < end {
            // A frame being rendered is queued when its render time is over.
            if let Some(done) = rendering_until
                && now >= done
            {
                rendering_until = None;
                pacer.rendered(true, *starts.last().unwrap());
                flip_at = Some(now + flip_delay);
            }
            let mut next = Next::Wait;
            if let Some(at) = flip_at
                && now >= at
            {
                flip_at = None;
                next = pacer.flip_complete(at, now);
            }
            if let Some(at) = timer
                && now >= at + timer_late
            {
                timer = None;
                next = pacer.timer_fired(now);
            }
            if rendering_until.is_none() && now.as_micros().is_multiple_of(1000) {
                let wanted = pacer.want_frame(now);
                if next == Next::Wait {
                    next = wanted;
                }
            }
            if rendering_until.is_none() {
                match next {
                    Next::Render => start_frame(now, &mut starts, &mut rendering_until),
                    Next::RenderAt(at) => {
                        timer.get_or_insert(at);
                    }
                    Next::Wait => {}
                }
            }
            now += tick;
        }
        assert!(pacer.is_unpaced());
        // Frames after the first two (before the output was known to be unpaced).
        let steady = &starts[2..];
        let span = *steady.last().unwrap() - steady[0];
        let mean = span / (steady.len() as u32 - 1);
        assert!(
            (598..=601).contains(&starts.len()),
            "{} frames in 10 s",
            starts.len()
        );
        assert!(
            mean.abs_diff(REFRESH) < Duration::from_micros(20),
            "mean interval {mean:?}, refresh {REFRESH:?}"
        );
    }
}
