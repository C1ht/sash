// SPDX-License-Identifier: Apache-2.0

//! xdg-shell: toplevel lifecycle and the commit rules Smithay leaves to the compositor.
//!
//! A toplevel is created unmapped. Its first commit gets the initial configure; the
//! first commit with a buffer after the client acked a configure maps it. No layout
//! yet: mapped windows sit at the output origin at the size the client chose.

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
        self.unmapped.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
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

fn is_toplevel_of(window: &Window, surface: &WlSurface) -> bool {
    window
        .toplevel()
        .is_some_and(|toplevel| toplevel.wl_surface() == surface)
}

fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

/// True if a non-zero maximum is below the minimum on the same axis (0 means unset).
/// Negative values never get this far; see the xdg_toplevel dispatch above.
fn max_below_min(min: Size<i32, Logical>, max: Size<i32, Logical>) -> bool {
    (max.w > 0 && max.w < min.w) || (max.h > 0 && max.h < min.h)
}

impl Sash {
    fn toplevel_for(&self, surface: &WlSurface) -> Option<ToplevelSurface> {
        self.unmapped
            .iter()
            .chain(self.space.elements())
            .find(|window| is_toplevel_of(window, surface))
            .and_then(|window| window.toplevel().cloned())
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
    pub fn window_committed(&mut self, root: &WlSurface) {
        if let Some(index) = self.unmapped.iter().position(|w| is_toplevel_of(w, root)) {
            let window = self.unmapped[index].clone();
            window.on_commit();
            let toplevel = window.toplevel().expect("only toplevels are tracked");
            if !toplevel.is_initial_configure_sent() {
                toplevel.send_configure();
            } else if has_buffer(root) {
                self.unmapped.remove(index);
                self.map_window(window);
            }
            return;
        }

        let Some(window) = self
            .space
            .elements()
            .find(|w| is_toplevel_of(w, root))
            .cloned()
        else {
            return;
        };
        window.on_commit();
        if !has_buffer(root) {
            // A null buffer unmaps; the client starts over with a new initial configure.
            self.space.unmap_elem(&window);
            self.unmapped.push(window);
            self.focus_topmost_window();
        }
        self.request_redraw();
    }

    fn map_window(&mut self, window: Window) {
        self.space.map_element(window.clone(), (0, 0), true);
        self.focus_window(Some(&window));
        self.request_redraw();
    }

    fn remove_window(&mut self, surface: &WlSurface) {
        self.unmapped.retain(|w| !is_toplevel_of(w, surface));
        let mapped = self
            .space
            .elements()
            .find(|w| is_toplevel_of(w, surface))
            .cloned();
        if let Some(window) = mapped {
            self.space.unmap_elem(&window);
            self.focus_topmost_window();
            self.request_redraw();
        }
    }

    /// Asks the client to close the window (xdg_toplevel.close). The window goes away
    /// when the client destroys the toplevel.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no keybind until M3; used by tests")
    )]
    pub fn request_close(&self, window: &Window) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_close();
        }
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
