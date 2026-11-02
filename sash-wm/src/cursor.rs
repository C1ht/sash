// SPDX-License-Identifier: Apache-2.0

//! The pointer's cursor as clients ask for it: a named shape (wp_cursor_shape_v1), a
//! surface of their own (wl_pointer.set_cursor) or hidden. The backend shows it:
//! nested, named shapes and "hidden" go to the host's cursor and cursor surfaces are
//! drawn by sash-wm at the pointer (see `render::cursor_elements`).
//!
//! When the pointer moves onto another surface (or off every surface) the cursor goes
//! back to the default arrow until that client sets its own.

use smithay::{
    delegate_cursor_shape,
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point},
    wayland::{compositor, tablet_manager::TabletSeatHandler},
};

use crate::state::Sash;

/// Frames for a moving cursor sash-wm draws: one request until the next frame starts,
/// so however many motion events arrive, at most one frame per frame (the backend
/// renders at most once per refresh period).
#[derive(Debug, Default)]
pub struct CursorFrames {
    pending: bool,
    requested: u64,
}

impl CursorFrames {
    /// The drawn cursor moved. Returns true if a frame must be requested (none is
    /// pending yet).
    pub fn moved(&mut self) -> bool {
        if self.pending {
            return false;
        }
        self.pending = true;
        self.requested += 1;
        true
    }

    /// The backend started a frame: it shows the cursor where it is now.
    pub fn frame_started(&mut self) {
        self.pending = false;
    }

    /// Frames requested for cursor motion so far.
    #[cfg(test)]
    pub fn requested(&self) -> u64 {
        self.requested
    }
}

pub struct Cursor {
    image: CursorImageStatus,
    /// Bumped on every change, so the backend can apply a change once.
    generation: u64,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            image: CursorImageStatus::Named(CursorIcon::Default),
            generation: 0,
        }
    }
}

impl Cursor {
    /// The current image; a dead cursor surface counts as the default arrow.
    pub fn image(&self) -> CursorImageStatus {
        match &self.image {
            CursorImageStatus::Surface(surface) if !surface.alive() => {
                CursorImageStatus::Named(CursorIcon::Default)
            }
            image => image.clone(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether `surface` is the current cursor surface.
    pub fn is_surface(&self, surface: &WlSurface) -> bool {
        matches!(&self.image, CursorImageStatus::Surface(s) if s == surface)
    }
}

impl Sash {
    pub fn set_cursor(&mut self, image: CursorImageStatus) {
        if self.cursor.image == image {
            return;
        }
        tracing::trace!(?image, "cursor");
        self.cursor.image = image;
        self.cursor.generation += 1;
        self.request_redraw();
    }

    /// Pointer focus moved to another surface (or none): back to the default arrow.
    pub fn pointer_focus_changed(&mut self) {
        self.set_cursor(CursorImageStatus::Named(CursorIcon::Default));
    }

    /// The cursor surface to draw and the global position of its origin (pointer minus
    /// hotspot), if the cursor is a surface.
    pub fn cursor_surface(&self) -> Option<(WlSurface, Point<i32, Logical>)> {
        let CursorImageStatus::Surface(surface) = self.cursor.image() else {
            return None;
        };
        let hotspot = compositor::with_states(&surface, |states| {
            states
                .data_map
                .get::<CursorImageSurfaceData>()
                .and_then(|data| data.lock().ok().map(|attributes| attributes.hotspot))
                .unwrap_or_default()
        });
        let pointer = self.seat.get_pointer()?.current_location().to_i32_round();
        Some((surface, pointer - hotspot))
    }
}

impl TabletSeatHandler for Sash {}

delegate_cursor_shape!(Sash);

#[cfg(test)]
mod tests {
    use super::CursorFrames;

    /// 1000 motion events per second, frames every 16.667 ms (60 Hz): one frame
    /// request per frame at most, never more.
    #[test]
    fn a_fast_mouse_requests_at_most_one_frame_per_period() {
        let mut frames = CursorFrames::default();
        let period_us = 16_667;
        let mut pending = false;
        let mut rendered = 0;
        for t in 0..1_000_000u64 {
            if t % 1000 == 0 && frames.moved() {
                pending = true;
            }
            if t % period_us == 0 && pending {
                frames.frame_started();
                pending = false;
                rendered += 1;
            }
        }
        // Every request but a last pending one became a frame.
        assert_eq!(frames.requested(), rendered + u64::from(pending));
        assert!(
            (59..=61).contains(&frames.requested()),
            "{} requests in one second",
            frames.requested()
        );
    }

    #[test]
    fn no_motion_requests_no_frame() {
        let mut frames = CursorFrames::default();
        for _ in 0..100 {
            frames.frame_started();
        }
        assert_eq!(frames.requested(), 0);
        assert!(frames.moved(), "the first motion asks for a frame");
        assert!(!frames.moved(), "merged until the frame starts");
        frames.frame_started();
        assert!(frames.moved());
        assert_eq!(frames.requested(), 2);
    }
}
