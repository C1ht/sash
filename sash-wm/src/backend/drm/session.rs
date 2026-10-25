// SPDX-License-Identifier: Apache-2.0

//! What the DRM backend does when the session pauses (VT switch away) or resumes, as
//! an ordered list of steps the backend carries out. On pause: stop rendering, give up
//! input, then the DRM device. On resume: take the DRM device back, reset its state
//! (another compositor may have changed it), take input back, redraw everything.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    StopRendering,
    SuspendInput,
    PauseDrm,
    ActivateDrm,
    ResetDrmState,
    ResumeInput,
    RedrawAll,
}

#[derive(Debug, Default)]
pub struct SessionTracker {
    paused: bool,
}

impl SessionTracker {
    /// Steps for a pause; none if already paused.
    pub fn pause(&mut self) -> Vec<Step> {
        if std::mem::replace(&mut self.paused, true) {
            return Vec::new();
        }
        vec![Step::StopRendering, Step::SuspendInput, Step::PauseDrm]
    }

    /// Steps for a resume; none if not paused.
    pub fn resume(&mut self) -> Vec<Step> {
        if !std::mem::replace(&mut self.paused, false) {
            return Vec::new();
        }
        vec![
            Step::ActivateDrm,
            Step::ResetDrmState,
            Step::ResumeInput,
            Step::RedrawAll,
        ]
    }

    #[cfg(test)]
    pub fn is_paused(&self) -> bool {
        self.paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_then_resume() {
        let mut session = SessionTracker::default();
        assert_eq!(
            session.pause(),
            [Step::StopRendering, Step::SuspendInput, Step::PauseDrm]
        );
        assert!(session.is_paused());
        assert_eq!(
            session.resume(),
            [
                Step::ActivateDrm,
                Step::ResetDrmState,
                Step::ResumeInput,
                Step::RedrawAll
            ]
        );
        assert!(!session.is_paused());
    }

    #[test]
    fn repeated_events_do_nothing() {
        let mut session = SessionTracker::default();
        assert!(session.resume().is_empty(), "resume while active");
        assert!(!session.pause().is_empty());
        assert!(session.pause().is_empty(), "pause while paused");
        assert!(!session.resume().is_empty());
        assert!(session.resume().is_empty());
    }

    #[test]
    fn the_device_is_released_last_and_taken_back_first() {
        let mut session = SessionTracker::default();
        assert_eq!(session.pause().last(), Some(&Step::PauseDrm));
        assert_eq!(session.resume().first(), Some(&Step::ActivateDrm));
    }
}
