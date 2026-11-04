// SPDX-License-Identifier: Apache-2.0

//! Renderer recovery (CLAUDE.md, "Renderer recovery (M11a, owner)"), the part both
//! backends share, with no Smithay types: the retry schedule, the flip watchdog and, in
//! debug builds, fault injection. Times are durations on one monotonic clock, so the
//! tests use fake time.

use std::time::Duration;

/// Retry delays after the first, second and third failure of an episode.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];
/// Frames requested but no flip (or presentation) for this long counts as a failure.
pub const WATCHDOG: Duration = Duration::from_secs(2);

/// Why a frame failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Rendering, swapping or queueing the frame failed.
    Frame,
    /// The GPU context is reported lost.
    ContextLost,
    /// The watchdog fired: no flip for WATCHDOG while frames were requested.
    Watchdog,
}

/// What the backend does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Try the frame again after this long.
    RetryIn(Duration),
    /// Rebuild the renderer, then try again after this long.
    RebuildThenRetryIn(Duration),
    /// Recovery failed: exit cleanly.
    GiveUp,
}

/// One failure episode: from the first failure to the next frame that succeeds.
#[derive(Debug, Default)]
pub struct Recovery {
    failures: usize,
    rebuilt: bool,
}

impl Recovery {
    /// A frame failed. Retries back off (0.5, 1, 2 s); the third failure or a lost
    /// context rebuilds the renderer once; any failure after the rebuild gives up.
    pub fn failed(&mut self, why: Failure) -> Next {
        self.failures += 1;
        if self.rebuilt {
            return Next::GiveUp;
        }
        let delay = BACKOFF[(self.failures - 1).min(BACKOFF.len() - 1)];
        if why == Failure::ContextLost || self.failures >= BACKOFF.len() {
            self.rebuilt = true;
            return Next::RebuildThenRetryIn(delay);
        }
        Next::RetryIn(delay)
    }

    /// A frame reached the screen: the episode is over.
    pub fn succeeded(&mut self) {
        *self = Self::default();
    }

    /// Whether an episode is running (a retry is due).
    pub fn active(&self) -> bool {
        self.failures > 0
    }
}

/// Watches for frames that never reach the screen.
#[derive(Debug, Default)]
pub struct Watchdog {
    /// When the oldest frame still waiting for its flip was requested.
    waiting_since: Option<Duration>,
}

impl Watchdog {
    /// A frame was queued (it waits for its flip from `now`).
    pub fn queued(&mut self, now: Duration) {
        self.waiting_since.get_or_insert(now);
    }

    /// A flip (presentation) came: nothing waits.
    pub fn flipped(&mut self) {
        self.waiting_since = None;
    }

    /// Whether a frame waits for its flip.
    pub fn waiting(&self) -> bool {
        self.waiting_since.is_some()
    }

    /// Forget the wait (the session paused, the output stopped presenting, the
    /// connector went away: no flip is expected).
    pub fn reset(&mut self) {
        self.waiting_since = None;
    }

    /// Checked at `now`: true (once) if a frame has waited WATCHDOG for its flip while
    /// flips are expected (`expected`: active session, output presenting, connector
    /// on).
    pub fn fired(&mut self, now: Duration, expected: bool) -> bool {
        if !expected {
            self.waiting_since = None;
            return false;
        }
        match self.waiting_since {
            Some(since) if now.saturating_sub(since) >= WATCHDOG => {
                self.waiting_since = None;
                true
            }
            _ => false,
        }
    }
}

/// Whether an error (or one of its sources) is EGL's "context lost".
pub fn context_lost(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(e) = current {
        if matches!(
            e.downcast_ref::<smithay::backend::egl::EGLError>(),
            Some(smithay::backend::egl::EGLError::ContextLost)
        ) {
            return true;
        }
        current = e.source();
    }
    false
}

/// Debug builds only: renderer faults on request (SASH_DEBUG_FAIL_SWAPS=N,
/// SASH_DEBUG_CONTEXT_LOST=K, SASH_DEBUG_STOP_FLIPS=K).
#[cfg(debug_assertions)]
#[derive(Debug, Default)]
pub struct Faults {
    fail_swaps: u64,
    context_lost_at: Option<u64>,
    stop_flips_after: Option<u64>,
    frames: u64,
}

/// What an injected fault does to a frame.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Injected {
    Fail,
    ContextLost,
}

#[cfg(debug_assertions)]
impl Faults {
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        let number = |name: &str| var(name).and_then(|v| v.trim().parse::<u64>().ok());
        let faults = Self {
            fail_swaps: number("SASH_DEBUG_FAIL_SWAPS").unwrap_or(0),
            context_lost_at: number("SASH_DEBUG_CONTEXT_LOST"),
            stop_flips_after: number("SASH_DEBUG_STOP_FLIPS"),
            frames: 0,
        };
        if faults.fail_swaps > 0
            || faults.context_lost_at.is_some()
            || faults.stop_flips_after.is_some()
        {
            tracing::warn!(?faults, "renderer fault injection (debug build)");
        }
        faults
    }

    /// Called for every frame about to be swapped (or queued): the fault, if any.
    pub fn swap(&mut self) -> Option<Injected> {
        self.frames += 1;
        if self.context_lost_at == Some(self.frames) {
            return Some(Injected::ContextLost);
        }
        if self.fail_swaps > 0 {
            self.fail_swaps -= 1;
            return Some(Injected::Fail);
        }
        None
    }

    /// Whether flips are swallowed (they never reach the backend).
    pub fn flips_stopped(&self) -> bool {
        self.stop_flips_after.is_some_and(|k| self.frames > k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(f64) -> Duration = Duration::from_secs_f64;

    #[test]
    fn retries_back_off_then_one_rebuild_then_give_up() {
        let mut r = Recovery::default();
        assert!(!r.active());
        assert_eq!(r.failed(Failure::Frame), Next::RetryIn(S(0.5)));
        assert!(r.active());
        assert_eq!(r.failed(Failure::Frame), Next::RetryIn(S(1.0)));
        assert_eq!(
            r.failed(Failure::Watchdog),
            Next::RebuildThenRetryIn(S(2.0))
        );
        assert_eq!(
            r.failed(Failure::Frame),
            Next::GiveUp,
            "failed again after the rebuild"
        );
    }

    #[test]
    fn a_lost_context_rebuilds_at_once_and_only_once() {
        let mut r = Recovery::default();
        assert_eq!(
            r.failed(Failure::ContextLost),
            Next::RebuildThenRetryIn(S(0.5))
        );
        assert_eq!(r.failed(Failure::ContextLost), Next::GiveUp);
        let mut r = Recovery::default();
        assert_eq!(r.failed(Failure::Frame), Next::RetryIn(S(0.5)));
        assert_eq!(
            r.failed(Failure::ContextLost),
            Next::RebuildThenRetryIn(S(1.0))
        );
    }

    #[test]
    fn a_frame_that_succeeds_ends_the_episode() {
        let mut r = Recovery::default();
        r.failed(Failure::Frame);
        r.failed(Failure::Frame);
        r.succeeded();
        assert!(!r.active());
        assert_eq!(
            r.failed(Failure::Frame),
            Next::RetryIn(S(0.5)),
            "starts over"
        );
        // After a rebuild that worked, a later episode may rebuild again.
        let mut r = Recovery::default();
        assert_eq!(
            r.failed(Failure::ContextLost),
            Next::RebuildThenRetryIn(S(0.5))
        );
        r.succeeded();
        assert_eq!(
            r.failed(Failure::ContextLost),
            Next::RebuildThenRetryIn(S(0.5))
        );
    }

    #[test]
    fn the_watchdog_fires_after_two_seconds_without_a_flip() {
        let mut w = Watchdog::default();
        w.queued(S(10.0));
        w.queued(S(10.5)); // the oldest wait counts
        assert!(!w.fired(S(11.9), true));
        assert!(w.fired(S(12.0), true));
        assert!(!w.fired(S(12.1), true), "once");
        // A flip clears the wait.
        w.queued(S(20.0));
        w.flipped();
        assert!(!w.fired(S(30.0), true));
    }

    /// No false trigger while flips are not expected: the session paused or switched
    /// away (VT switch), the output hidden, the connector off. The wait is forgotten
    /// then, so coming back starts a fresh one.
    #[test]
    fn the_watchdog_never_fires_while_no_flip_is_expected() {
        let mut w = Watchdog::default();
        w.queued(S(0.0));
        assert!(!w.fired(S(1.0), true));
        // Switched away at 1.5 s for a minute (pause): no trigger.
        for t in [1.5, 5.0, 30.0, 61.0] {
            assert!(!w.fired(S(t), false));
        }
        // Back: the old wait does not count.
        assert!(!w.fired(S(61.5), true));
        w.queued(S(62.0));
        assert!(!w.fired(S(63.9), true));
        assert!(w.fired(S(64.0), true));
        // reset() (hidden output, connector off) forgets the wait too.
        w.queued(S(100.0));
        w.reset();
        assert!(!w.fired(S(200.0), true));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn fault_injection_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.to_string())
            }
        };
        let mut none = Faults::from_env(env(&[]));
        assert!((0..100).all(|_| none.swap().is_none()));
        assert!(!none.flips_stopped());
        let mut f = Faults::from_env(env(&[("SASH_DEBUG_FAIL_SWAPS", "2")]));
        assert_eq!(f.swap(), Some(Injected::Fail));
        assert_eq!(f.swap(), Some(Injected::Fail));
        assert_eq!(f.swap(), None);
        let mut f = Faults::from_env(env(&[
            ("SASH_DEBUG_CONTEXT_LOST", "3"),
            ("SASH_DEBUG_STOP_FLIPS", "4"),
        ]));
        assert_eq!(f.swap(), None);
        assert_eq!(f.swap(), None);
        assert_eq!(f.swap(), Some(Injected::ContextLost));
        assert!(!f.flips_stopped());
        assert_eq!(f.swap(), None);
        assert!(!f.flips_stopped(), "frame 4 still flips");
        f.swap();
        assert!(f.flips_stopped());
    }
}
