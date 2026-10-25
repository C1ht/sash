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
