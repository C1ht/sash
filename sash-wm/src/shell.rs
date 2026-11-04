// SPDX-License-Identifier: Apache-2.0

//! xdg-shell: toplevel lifecycle and the commit rules Smithay leaves to the compositor.
//!
//! Window lifecycle and placement live in `windows.rs`; this module handles the
//! protocol side.

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::Window,
    input::Seat,
    reexports::{
        wayland_protocols::xdg::shell::server::{
            xdg_popup::XdgPopup,
            xdg_positioner::XdgPositioner,
            xdg_surface::XdgSurface,
            xdg_toplevel::{self, XdgToplevel},
            xdg_wm_base::{self, XdgWmBase},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource,
            backend::ClientId,
            delegate_dispatch,
            protocol::{wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface},
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

use crate::{
    clients::ClientState,
    layout::{Edges, Mode},
    state::Sash,
    windows::{DragKind, StateRequest},
};

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

    /// Only recorded (debug recording); Smithay tracks acknowledged configures.
    #[cfg_attr(not(feature = "debug-record"), expect(unused_variables))]
    fn ack_configure(
        &mut self,
        surface: WlSurface,
        configure: smithay::wayland::shell::xdg::Configure,
    ) {
        record!(
            self,
            "ack_configure",
            "window" => self.windows.by_surface(&surface).map(|m| m.id),
            "serial" => u32::from(match &configure {
                smithay::wayland::shell::xdg::Configure::Toplevel(c) => c.serial,
                smithay::wayland::shell::xdg::Configure::Popup(c) => c.serial,
            }),
            "popup" => matches!(configure, smithay::wayland::shell::xdg::Configure::Popup(_)),
        );
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        self.client_drag(&surface, &seat, serial, DragKind::Move);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        self.client_drag(&surface, &seat, serial, DragKind::Resize(edges_from(edges)));
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.client_state_request(surface.wl_surface(), StateRequest::Maximize(true));
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.client_state_request(surface.wl_surface(), StateRequest::Maximize(false));
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        // One output so far: fullscreen is always on it.
        self.client_state_request(surface.wl_surface(), StateRequest::Fullscreen(true));
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.client_state_request(surface.wl_surface(), StateRequest::Fullscreen(false));
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        Sash::new_popup(self, surface, positioner);
    }

    fn grab(&mut self, surface: PopupSurface, _seat: WlSeat, serial: Serial) {
        self.grab_popup(surface, serial);
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        self.reposition_popup(surface, positioner, token);
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        Sash::popup_destroyed(self, &surface);
    }
}

// Everything except xdg_wm_base and xdg_toplevel goes straight to Smithay (what
// `delegate_xdg_shell!` would do); those two are handled first, see below.
crate::limits::counted_global!(XdgWmBase: () => XdgShellState);
delegate_dispatch!(Sash: [XdgPositioner: XdgPositionerUserData] => XdgShellState);
delegate_dispatch!(Sash: [XdgPopup: XdgShellSurfaceUserData] => XdgShellState);
delegate_dispatch!(Sash: [XdgSurface: XdgSurfaceUserData] => XdgShellState);

/// Records the client's xdg_wm_base (Smithay 0.7 does not expose it; sash-wm needs
/// it to post not_the_topmost_popup), then lets Smithay handle the request.
impl Dispatch<XdgWmBase, XdgWmBaseUserData> for Sash {
    fn request(
        state: &mut Self,
        client: &Client,
        wm_base: &XdgWmBase,
        request: xdg_wm_base::Request,
        data: &XdgWmBaseUserData,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let Some(client_state) = client.get_data::<ClientState>() {
            client_state.note_xdg_wm_base(wm_base);
        }
        <XdgShellState as Dispatch<XdgWmBase, XdgWmBaseUserData, Self>>::request(
            state, client, wm_base, request, data, display, data_init,
        );
    }

    fn destroyed(
        state: &mut Self,
        client: ClientId,
        wm_base: &XdgWmBase,
        data: &XdgWmBaseUserData,
    ) {
        <XdgShellState as Dispatch<XdgWmBase, XdgWmBaseUserData, Self>>::destroyed(
            state, client, wm_base, data,
        );
    }
}

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

/// The edges an xdg_toplevel.resize request moves.
fn edges_from(edges: xdg_toplevel::ResizeEdge) -> Edges {
    use xdg_toplevel::ResizeEdge as E;
    Edges {
        left: matches!(edges, E::Left | E::TopLeft | E::BottomLeft),
        right: matches!(edges, E::Right | E::TopRight | E::BottomRight),
        top: matches!(edges, E::Top | E::TopLeft | E::TopRight),
        bottom: matches!(edges, E::Bottom | E::BottomLeft | E::BottomRight),
    }
}

impl Sash {
    /// A client asked to move or resize its window (e.g. a drag on its title bar). It
    /// must hold a pointer button pressed on that window, with the serial of the press.
    fn client_drag(
        &mut self,
        surface: &ToplevelSurface,
        seat: &WlSeat,
        serial: Serial,
        kind: DragKind,
    ) {
        let Some(pointer) = Seat::<Sash>::from_resource(seat).and_then(|s| s.get_pointer()) else {
            return;
        };
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start) = pointer.grab_start_data() else {
            return;
        };
        let pressed_on_window = start
            .focus
            .as_ref()
            .is_some_and(|(focus, _)| focus.id().same_client_as(&surface.wl_surface().id()));
        let Some(id) = self.windows.by_surface(surface.wl_surface()).map(|m| m.id) else {
            return;
        };
        // Client resizes of tiled windows stay ignored; tiled resize is mod+right-drag.
        let tiled = self.windows.layout.mode(id) == Some(Mode::Tiled);
        if matches!(kind, DragKind::Resize(_)) && tiled {
            return;
        }
        if pressed_on_window {
            self.start_pointer_drag(id, kind, start, serial);
        }
    }

    fn toplevel_for(&self, surface: &WlSurface) -> Option<ToplevelSurface> {
        self.windows
            .by_surface(surface)
            .and_then(|managed| managed.window.toplevel().cloned())
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
