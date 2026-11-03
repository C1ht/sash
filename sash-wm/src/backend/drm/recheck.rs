// SPDX-License-Identifier: Apache-2.0

//! When the connector is re-checked after udev change events. A re-check is a forced
//! probe of the connector (on NVIDIA about 190 ms, the display's EDID is read again),
//! so events are merged: the first one opens a window of WINDOW, and one re-check runs
//! when it ends. After a session resume, a re-check waits until the input devices are
//! back (the first libinput event), at most INPUT_WAIT after the resume. Times are
//! durations on one monotonic clock, so tests can use fake time.

use std::time::Duration;

/// Events within this window of the first one are one re-check.
pub const WINDOW: Duration = Duration::from_millis(200);
/// After a resume, a re-check waits for the input devices at most this long.
pub const INPUT_WAIT: Duration = Duration::from_secs(1);

/// What the caller does when the timer it armed fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fired {
    /// Re-check the connector now.
    Run,
    /// Waiting for the input devices: arm the timer again for this time.
    WaitUntil(Duration),
    /// Nothing is pending (already run).
    Nothing,
}

#[derive(Debug, Default)]
pub struct Recheck {
    /// The window of the pending re-check ends here.
    due: Option<Duration>,
    /// Resumed, and no libinput event since: the time of the resume.
    resumed_at: Option<Duration>,
}

impl Recheck {
    /// A udev change event of the DRM device at `now`. Returns the time to arm a timer
    /// for, or None if the event joins the pending re-check.
    pub fn changed(&mut self, now: Duration) -> Option<Duration> {
        if self.due.is_some() {
            return None;
        }
        let due = now + WINDOW;
        self.due = Some(due);
        Some(due)
    }

    /// The session resumed at `now`; input devices are not back yet.
    pub fn resumed(&mut self, now: Duration) {
        self.resumed_at = Some(now);
    }

    /// The timer armed by `changed` (or `WaitUntil`) fired at `now`.
    pub fn fired(&mut self, now: Duration) -> Fired {
        let Some(due) = self.due else {
            return Fired::Nothing;
        };
        if now < due {
            return Fired::WaitUntil(due);
        }
        if let Some(resumed_at) = self.resumed_at {
            let limit = resumed_at + INPUT_WAIT;
            if now < limit {
                return Fired::WaitUntil(limit);
            }
            self.resumed_at = None;
        }
        self.due = None;
        Fired::Run
    }

    /// A libinput event arrived (the devices are back). Returns true if a re-check was
    /// waiting for it and is due: run it now.
    pub fn input_back(&mut self, now: Duration) -> bool {
        if self.resumed_at.take().is_none() {
            return false;
        }
        match self.due {
            Some(due) if now >= due => {
                self.due = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn events_within_the_window_are_one_recheck() {
        let mut r = Recheck::default();
        assert_eq!(r.changed(ms(1000)), Some(ms(1200)));
        assert_eq!(r.changed(ms(1050)), None);
        assert_eq!(r.changed(ms(1199)), None);
        assert_eq!(r.fired(ms(1200)), Fired::Run);
        assert_eq!(r.fired(ms(1201)), Fired::Nothing, "run once");
        // After the window: a new re-check.
        assert_eq!(r.changed(ms(1250)), Some(ms(1450)));
        assert_eq!(r.fired(ms(1450)), Fired::Run);
    }

    #[test]
    fn a_timer_firing_early_waits_for_the_end_of_the_window() {
        let mut r = Recheck::default();
        r.changed(ms(0));
        assert_eq!(r.fired(ms(150)), Fired::WaitUntil(ms(200)));
        assert_eq!(r.fired(ms(200)), Fired::Run);
    }

    #[test]
    fn after_a_resume_the_recheck_waits_for_the_input_devices() {
        let mut r = Recheck::default();
        r.resumed(ms(1000));
        r.changed(ms(1010));
        // The window ends before libinput is back: wait.
        assert_eq!(r.fired(ms(1210)), Fired::WaitUntil(ms(2000)));
        assert_eq!(r.changed(ms(1300)), None, "joins the waiting re-check");
        // The devices are back: the re-check runs now, once.
        assert!(r.input_back(ms(1500)));
        assert_eq!(r.fired(ms(2000)), Fired::Nothing);
        assert!(
            !r.input_back(ms(1600)),
            "only the first input after a resume"
        );
    }

    #[test]
    fn input_back_before_the_window_ends_lets_the_timer_run_it() {
        let mut r = Recheck::default();
        r.resumed(ms(0));
        r.changed(ms(10));
        assert!(!r.input_back(ms(50)), "not due yet");
        assert_eq!(r.fired(ms(210)), Fired::Run);
    }

    #[test]
    fn without_input_devices_the_recheck_runs_after_the_input_wait() {
        let mut r = Recheck::default();
        r.resumed(ms(0));
        r.changed(ms(100));
        assert_eq!(r.fired(ms(300)), Fired::WaitUntil(INPUT_WAIT));
        assert_eq!(r.fired(INPUT_WAIT), Fired::Run);
    }

    #[test]
    fn without_a_resume_nothing_waits_for_input() {
        let mut r = Recheck::default();
        r.changed(ms(0));
        assert!(
            !r.input_back(ms(100)),
            "input during normal running changes nothing"
        );
        assert_eq!(r.fired(ms(200)), Fired::Run);
    }

    /// The owner's VT switch 1 (run 4, NVIDIA): a udev event while paused, the resume,
    /// then the event loop blocked by the first frame; two more udev events and the
    /// input devices arrive together when it is free. Before M9d: a re-check per event
    /// (two of 190 ms after the resume), input back 919 ms after the resume. Now: the
    /// re-checks wait for the input, and events arriving together are one re-check.
    #[test]
    fn the_owners_vt_switch_gives_input_first_and_one_recheck() {
        let mut r = Recheck::default();
        let t = |s: f64| Duration::from_secs_f64(s);
        // 45.7226 udev event (paused), 45.8009 resume.
        assert_eq!(r.changed(t(45.7226)), Some(t(45.9226)));
        r.resumed(t(45.8009));
        // The loop is busy until 46.339 (activation, first frame); then the timer, the
        // two udev events and libinput are all ready, in any order.
        assert_eq!(r.fired(t(46.339)), Fired::WaitUntil(t(46.8009)));
        assert_eq!(r.changed(t(46.3398)), None);
        assert_eq!(r.changed(t(46.3400)), None);
        let mut rechecks = 0;
        if r.input_back(t(46.3401)) {
            rechecks += 1;
        }
        if r.fired(t(46.8009)) == Fired::Run {
            rechecks += 1;
        }
        assert_eq!(
            rechecks, 1,
            "one re-check, after the input devices are back"
        );
    }
}
