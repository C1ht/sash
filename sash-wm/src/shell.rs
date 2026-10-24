// SPDX-License-Identifier: Apache-2.0

//! xdg-shell: toplevel lifecycle and the commit rules Smithay leaves to the compositor.
//!
//! Window lifecycle and placement live in `tiling.rs`; this module handles the
//! protocol side.

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::Window,
    reexports::{
        wayland_protocols::xdg::shell::server::{
            xdg_popup::XdgPopup,
            xdg_positioner::XdgPositioner,
            xdg_surface::XdgSurface,
            xdg_toplevel::{self, XdgToplevel},
            xdg_wm_base::XdgWmBase,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource,
            backend::ClientId,
            delegate_dispatch, delegate_global_dispatch,
            protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Serial, Size},
    wayland::{
        compositor::{self, BufferAssignment, SurfaceAttributes},
        shell::xdg::{
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface,
            XdgPositionerUserData, XdgShellHandler, XdgShellState, XdgShellSurfaceUserData,
            XdgSurfaceUserData, XdgWmBaseUserData,
        },
    },
};

use crate::state::Sash;

impl XdgShellHandler for Sash {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.protocols.xdg_shell
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.add_toplevel(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.forget_toplevel(surface.wl_surface());
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        tracing::debug!("popup ignored: popups are not supported yet");
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

// Everything except xdg_toplevel goes straight to Smithay (what `delegate_xdg_shell!`
// would do); xdg_toplevel requests are checked first, see below.
delegate_global_dispatch!(Sash: [XdgWmBase: ()] => XdgShellState);
delegate_dispatch!(Sash: [XdgWmBase: XdgWmBaseUserData] => XdgShellState);
delegate_dispatch!(Sash: [XdgPositioner: XdgPositionerUserData] => XdgShellState);
delegate_dispatch!(Sash: [XdgPopup: XdgShellSurfaceUserData] => XdgShellState);
delegate_dispatch!(Sash: [XdgSurface: XdgSurfaceUserData] => XdgShellState);

/// Rejects negative min/max sizes with `xdg_toplevel.invalid_size` before Smithay sees
/// them: Smithay 0.7 stores them in a `Size`, which panics on negative values in debug
/// builds, so a client could otherwise crash the compositor.
impl Dispatch<XdgToplevel, XdgShellSurfaceUserData> for Sash {
    fn request(
        state: &mut Self,
        client: &Client,
        toplevel: &XdgToplevel,
        request: xdg_toplevel::Request,
        data: &XdgShellSurfaceUserData,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_toplevel::Request::SetMinSize { width, height }
        | xdg_toplevel::Request::SetMaxSize { width, height } = &request
            && (*width < 0 || *height < 0)
        {
            toplevel.post_error(
                xdg_toplevel::Error::InvalidSize,
                format!("negative min/max size: {width}x{height}"),
            );
            tracing::debug!(width, height, "protocol error: negative min/max size");
            return;
        }
        <XdgShellState as Dispatch<XdgToplevel, XdgShellSurfaceUserData, Self>>::request(
            state, client, toplevel, request, data, display, data_init,
        );
    }

    fn destroyed(
        state: &mut Self,
        client: ClientId,
        toplevel: &XdgToplevel,
        data: &XdgShellSurfaceUserData,
    ) {
        <XdgShellState as Dispatch<XdgToplevel, XdgShellSurfaceUserData, Self>>::destroyed(
            state, client, toplevel, data,
        );
    }
}

/// True if a non-zero maximum is below the minimum on the same axis (0 means unset).
/// Negative values never get this far; see the xdg_toplevel dispatch above.
fn max_below_min(min: Size<i32, Logical>, max: Size<i32, Logical>) -> bool {
    (max.w > 0 && max.w < min.w) || (max.h > 0 && max.h < min.h)
}

impl Sash {
    fn toplevel_for(&self, surface: &WlSurface) -> Option<ToplevelSurface> {
        self.tiling
            .by_surface(surface)
            .and_then(|tile| tile.window.toplevel().cloned())
    }

    /// Checks a commit on a toplevel surface. Sends a protocol error and returns true
    /// if the client broke a rule; the commit must then be ignored.
    ///
    /// - A buffer before the first ack_configure: Smithay's `ensure_configured` sends
    ///   `xdg_surface.not_constructed` (Smithay 0.7 does not expose the xdg_surface
    ///   object, so `unconfigured_buffer` cannot be sent).
    /// - Maximum size below minimum size: `xdg_toplevel.invalid_size`.
    pub fn reject_invalid_commit(&self, surface: &WlSurface) -> bool {
        let Some(toplevel) = self.toplevel_for(surface) else {
            return false;
        };
        let (buffer_attached, min, max, geometry) = compositor::with_states(surface, |states| {
            let attached = matches!(
                states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer,
                Some(BufferAssignment::NewBuffer(_))
            );
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            (
                attached,
                current.min_size,
                current.max_size,
                current.geometry,
            )
        });

        if buffer_attached && !toplevel.ensure_configured() {
            tracing::debug!("protocol error: buffer committed before ack_configure");
            return true;
        }
        if max_below_min(min, max) {
            toplevel.xdg_toplevel().post_error(
                xdg_toplevel::Error::InvalidSize,
                format!(
                    "max size {}x{} is below min size {}x{}",
                    max.w, max.h, min.w, min.h
                ),
            );
            tracing::debug!(?min, ?max, "protocol error: max size below min size");
            return true;
        }
        if let Some(geometry) = geometry.filter(|g| g.size.w <= 0 || g.size.h <= 0) {
            // Should be xdg_surface.invalid_size, which Smithay 0.7 gives no way to send.
            tracing::warn!(?geometry, "ignoring invalid window geometry");
        }
        false
    }

    /// Handles a commit on a root surface (after the buffer was imported).
    pub fn root_committed(&mut self, root: &WlSurface) {
        tracing::trace!(surface = ?root.id(), "commit");
        let has_buffer =
            with_renderer_surface_state(root, |state| state.buffer().is_some()).unwrap_or(false);
        self.toplevel_committed(root, has_buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::max_below_min;

    #[test]
    fn max_below_min_per_axis() {
        let ok = [
            ((0, 0), (0, 0)),
            ((100, 50), (0, 0)),
            ((100, 50), (100, 50)),
            ((0, 0), (300, 200)),
        ];
        for (min, max) in ok {
            assert!(!max_below_min(min.into(), max.into()), "{min:?} {max:?}");
        }
        let bad = [
            ((200, 50), (100, 0)),
            ((0, 80), (0, 40)),
            ((200, 200), (100, 100)),
        ];
        for (min, max) in bad {
            assert!(max_below_min(min.into(), max.into()), "{min:?} {max:?}");
        }
    }
}
