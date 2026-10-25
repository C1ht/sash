// SPDX-License-Identifier: Apache-2.0

//! When the DRM backend renders: only after the previous page flip has completed, only
//! while the session is active, and only if a frame is wanted. Frame callbacks go out
//! when a flip completes (the frame is on screen), not when it is queued.

#[derive(Debug)]
pub struct Pacer {
    /// A queued frame waits for its page flip.
    flip_pending: bool,
    /// Something changed since the last frame.
    wanted: bool,
    /// The session (VT) is ours.
    active: bool,
}

impl Default for Pacer {
    fn default() -> Self {
        Self {
            flip_pending: false,
            wanted: true,
            active: true,
        }
    }
}

impl Pacer {
    /// A frame is wanted. Returns true if the backend should render now.
    pub fn want_frame(&mut self) -> bool {
        self.wanted = true;
        self.can_render()
    }

    /// The backend rendered: `queued` if a frame went to the display (it will flip),
    /// false if there was nothing to show or rendering failed.
    pub fn rendered(&mut self, queued: bool) {
        self.wanted = false;
        self.flip_pending = queued;
    }

    /// The page flip of the queued frame completed. Returns true if the backend should
    /// render the next frame now. A late flip event after a pause is ignored.
    pub fn flip_complete(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.flip_pending = false;
        self.can_render()
    }

    /// The session was paused (VT switch away): no rendering, and a pending flip will
    /// not complete.
    pub fn pause(&mut self) {
        self.active = false;
        self.flip_pending = false;
    }

    /// The session is ours again: the whole output must be redrawn. Returns true if the
    /// backend should render now.
    pub fn resume(&mut self) -> bool {
        self.active = true;
        self.flip_pending = false;
        self.wanted = true;
        true
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn can_render(&self) -> bool {
        self.active && self.wanted && !self.flip_pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frame_renders_at_once() {
        let mut pacer = Pacer::default();
        assert!(pacer.want_frame());
    }

    #[test]
    fn no_frame_while_a_flip_is_pending() {
        let mut pacer = Pacer::default();
        assert!(pacer.want_frame());
        pacer.rendered(true);
        assert!(!pacer.want_frame(), "flip pending");
        assert!(!pacer.want_frame(), "still pending");
        assert!(
            pacer.flip_complete(),
            "the wanted frame renders after the flip"
        );
        pacer.rendered(true);
        assert!(!pacer.flip_complete(), "nothing new wanted");
    }

    #[test]
    fn an_empty_frame_does_not_wait_for_a_flip() {
        let mut pacer = Pacer::default();
        assert!(pacer.want_frame());
        pacer.rendered(false);
        assert!(pacer.want_frame());
    }

    #[test]
    fn nothing_renders_while_paused() {
        let mut pacer = Pacer::default();
        pacer.pause();
        assert!(!pacer.want_frame());
        assert!(!pacer.flip_complete());
        assert!(!pacer.is_active());
    }

    #[test]
    fn a_vt_switch_while_a_flip_is_pending() {
        let mut pacer = Pacer::default();
        assert!(pacer.want_frame());
        pacer.rendered(true);
        pacer.pause();
        // The flip event arrives late, after the pause: ignored.
        assert!(!pacer.flip_complete());
        assert!(!pacer.want_frame());
        // Back on our VT: the pending flip is forgotten, a full frame renders at once.
        assert!(pacer.resume());
        assert!(pacer.want_frame());
        pacer.rendered(true);
        assert!(!pacer.want_frame());
        assert!(pacer.flip_complete());
    }

    #[test]
    fn resume_always_redraws() {
        let mut pacer = Pacer::default();
        assert!(pacer.want_frame());
        pacer.rendered(false);
        pacer.pause();
        pacer.pause(); // twice is harmless
        assert!(pacer.resume());
        assert!(pacer.is_active());
    }
}
