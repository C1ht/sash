// SPDX-License-Identifier: Apache-2.0

//! Window management: connects the monitor layout to Wayland windows. Window ids,
//! configures, positions, keyboard focus, keybind actions and drags.
//!
//! A toplevel enters the layout on its initial commit. A tiled window's initial
//! configure carries its tile size; a floating one gets 0x0 (the client chooses) and is
//! placed when its first buffer arrives. It leaves the layout when it unmaps (null
//! buffer) or is destroyed.
//!
//! Resize synchronisation: every changed window is configured in the same batch. Tiled
//! windows are drawn at their tile origin and clipped to the tile; floating, maximized
//! and fullscreen windows are drawn unclipped.
//!
//! Drags: a move follows the pointer, snaps window edges to nearby edges (not with
//! Shift held) and, when the pointer is in a snap zone, shows a preview and snaps the
//! window on release. Pulling a snapped window out restores its pre-snap size.

#[cfg(not(test))]
use std::process::Command;
use std::time::{Duration, Instant};

use smithay::{
    desktop::{Window, WindowSurfaceType, utils::under_from_surface_tree},
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Size},
    wayland::{compositor, shell::xdg::SurfaceCachedState},
};

use crate::{
    keybinds::{Action, Cycle},
    layout::{
        Direction, Edges, Layer, Mode, MonitorLayout, NewWindow, Presentation, Rect, STEP,
        SizeLimits, TiledResize, WindowId, ZoneId,
        decor::{Decoration, Part},
        layers::{self, LayerSurface},
        resize_rect, resize_zone, snap_to_edges, stop_top_edge,
    },
    popups::popups_of,
    state::Sash,
};

/// A debug-only override of a geometry constant (0 to 64 px), to compare by eye; no
/// setting. Absent from release builds (tools/check-release.sh).
#[cfg(debug_assertions)]
fn debug_px(var: &str) -> Option<i32> {
    let px = std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|px| (0..=64).contains(px))?;
    tracing::info!(px, var, "debug override");
    Some(px)
}

/// The gaps (layout::gaps, outer 13 and inner 8 px; owner, M18a). Debug builds only:
/// SASH_DEBUG_OUTER_GAP_PX and SASH_DEBUG_INNER_GAP_PX override them.
fn gaps() -> crate::layout::gaps::Gaps {
    use crate::layout::gaps::Gaps;
    #[cfg(debug_assertions)]
    return Gaps::new(
        debug_px("SASH_DEBUG_OUTER_GAP_PX").unwrap_or(Gaps::DEFAULT.outer),
        debug_px("SASH_DEBUG_INNER_GAP_PX").unwrap_or(Gaps::DEFAULT.inner),
    );
    #[cfg(not(debug_assertions))]
    Gaps::DEFAULT
}

/// The corner radius (layout::shape::RADIUS, 13 px; owner, M18a). Debug builds only:
/// SASH_DEBUG_RADIUS_PX overrides it.
fn radius_px() -> i32 {
    #[cfg(debug_assertions)]
    if let Some(px) = debug_px("SASH_DEBUG_RADIUS_PX") {
        return px;
    }
    crate::layout::shape::RADIUS
}

/// How long drag configures wait for a frame before they are sent anyway.
const FRAME_FALLBACK: Duration = Duration::from_millis(16);

/// What was last sent to a client in a configure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sent {
    presentation: Presentation,
    resizing: bool,
    /// The xdg activated state: the window has keyboard focus (owner, M19a).
    activated: bool,
}

impl Sent {
    fn size(&self) -> (i32, i32) {
        self.presentation.size.unwrap_or((0, 0))
    }
}

/// A toplevel known to the compositor.
pub struct Managed {
    pub id: WindowId,
    pub window: Window,
    /// Whether the client has committed a buffer since entering the layout.
    pub mapped: bool,
    /// The client negotiated xdg-decoration (always answered server-side).
    pub server_side: bool,
    sent: Option<Sent>,
    /// Send a configure even if nothing changed (a client request needs an answer).
    reply_due: bool,
    /// When a configure with a new size was sent and not yet answered by a buffer of
    /// that size (stale-content measurement).
    resize_sent_at: Option<Instant>,
}

/// An interactive move or resize.
#[derive(Clone, Copy, Debug)]
pub enum DragKind {
    Move,
    Resize(Edges),
}

/// A drag in progress.
#[derive(Clone, Debug)]
pub struct Drag {
    pub window: WindowId,
    pub kind: DragKind,
    /// A resize of a tiled window: the splits being moved.
    tiled: Option<TiledResize>,
    /// The window's rectangle and the pointer position the drag is measured from
    /// (moved when a snapped window is pulled out of its zone).
    from: Rect,
    pressed_at: Point<f64, Logical>,
    /// The snap zone the pointer is in (moves only).
    zone: Option<ZoneId>,
    /// The last motion handled, with Shift's state: a motion that changes neither
    /// (the pointer pinned at a screen edge) changes nothing and draws no frame.
    last: Option<(Point<f64, Logical>, bool)>,
}

pub struct Windows {
    pub layout: MonitorLayout,
    list: Vec<Managed>,
    next_id: u64,
    /// The drag in progress.
    pub drag: Option<Drag>,
    /// A drag changed the layout; its configures are due with the next frame.
    arrange_pending: bool,
    /// The FRAME_FALLBACK timer is running.
    fallback_armed: bool,
    /// Buttons whose press started a drag or was mod+click; their release is not
    /// delivered either.
    pub swallowed_buttons: Vec<u32>,
    /// When the oldest input whose effect is not on screen yet arrived (latency logging).
    pub input_pending_since: Option<Instant>,
    /// The output in logical pixels.
    output: Rect,
    /// The output's layer surfaces (M13b step 3 fills this from the protocol).
    layer_surfaces: Vec<LayerSurface>,
    /// Their arrangement on the output.
    pub arrangement: layers::Arrangement,
}

impl Windows {
    pub fn new() -> Self {
        let mut layout = MonitorLayout::new(Rect::default());
        layout.set_gaps(gaps());
        layout.set_radius(radius_px());
        Self {
            layout,
            list: Vec::new(),
            next_id: 1,
            drag: None,
            arrange_pending: false,
            fallback_armed: false,
            swallowed_buttons: Vec::new(),
            input_pending_since: None,
            output: Rect::default(),
            layer_surfaces: Vec::new(),
            arrangement: layers::Arrangement::default(),
        }
    }

    pub fn all(&self) -> impl Iterator<Item = &Managed> {
        self.list.iter()
    }

    pub fn by_surface(&self, surface: &WlSurface) -> Option<&Managed> {
        self.list
            .iter()
            .find(|m| is_toplevel_of(&m.window, surface))
    }

    /// The windows whose last configure had the activated state (tests).
    #[cfg(test)]
    pub fn activated_for_tests(&self) -> Vec<WindowId> {
        self.list
            .iter()
            .filter(|m| m.sent.is_some_and(|s| s.activated))
            .map(|m| m.id)
            .collect()
    }

    pub fn by_id(&self, id: WindowId) -> Option<&Managed> {
        self.list.iter().find(|m| m.id == id)
    }

    fn by_id_mut(&mut self, id: WindowId) -> Option<&mut Managed> {
        self.list.iter_mut().find(|m| m.id == id)
    }

    /// Notes whether the client negotiated server-side decorations.
    pub fn set_server_side(&mut self, id: WindowId, server_side: bool) {
        if let Some(managed) = self.by_id_mut(id) {
            managed.server_side = server_side;
        }
    }

    /// The client gets a configure with the next arrange even if nothing changed.
    pub fn reply_due(&mut self, id: WindowId) {
        if let Some(managed) = self.by_id_mut(id) {
            managed.reply_due = true;
        }
    }

    /// The edges an interactive resize of `id` moves, if one is in progress.
    fn resizing(&self, id: WindowId) -> Option<Edges> {
        match &self.drag {
            Some(Drag {
                window,
                kind: DragKind::Resize(edges),
                tiled: None,
                ..
            }) if *window == id => Some(*edges),
            _ => None,
        }
    }

    /// Marks that an input changed what should be on screen.
    pub fn note_input(&mut self, at: Instant) {
        self.input_pending_since.get_or_insert(at);
    }
}

fn is_toplevel_of(window: &Window, surface: &WlSurface) -> bool {
    window
        .toplevel()
        .is_some_and(|toplevel| toplevel.wl_surface() == surface)
}

const TILED: [xdg_toplevel::State; 4] = [
    xdg_toplevel::State::TiledLeft,
    xdg_toplevel::State::TiledRight,
    xdg_toplevel::State::TiledTop,
    xdg_toplevel::State::TiledBottom,
];

/// Where the window's buffer goes so that its visible geometry starts at `rect`'s origin
/// (clients may draw shadows outside their window geometry).
fn buffer_position(window: &Window, rect: Rect) -> Point<i32, Logical> {
    Point::from((rect.x, rect.y)) - window.geometry().loc
}

/// The client's committed min/max size.
fn size_limits(surface: &WlSurface) -> SizeLimits {
    compositor::with_states(surface, |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let current = cached.current();
        SizeLimits {
            min: (current.min_size.w, current.min_size.h),
            max: (current.max_size.w, current.max_size.h),
        }
    })
}

/// A surface under the pointer and the global position of its origin.
pub type SurfaceHit = (WlSurface, Point<f64, Logical>);

/// A window drawn on the output.
pub struct Visible<'a> {
    pub id: WindowId,
    pub window: &'a Window,
    /// The client's rectangle (inside the decorations).
    pub rect: Rect,
    /// The whole window, decorations included.
    pub outer: Rect,
    pub decoration: Decoration,
    pub position: Point<i32, Logical>,
    pub layer: Layer,
}

impl Sash {
    /// A new toplevel: known, but not in the layout until its initial commit.
    pub fn add_toplevel(&mut self, window: Window) {
        let id = WindowId(self.windows.next_id);
        self.windows.next_id += 1;
        self.windows.list.push(Managed {
            id,
            window,
            mapped: false,
            server_side: false,
            sent: None,
            reply_due: false,
            resize_sent_at: None,
        });
    }

    /// Forced floating: a parent (dialog) or a fixed size. Returns (forced, parent, limits).
    fn classify(&self, window: &Window) -> (bool, Option<WindowId>, SizeLimits) {
        let toplevel = window.toplevel().expect("only toplevels are tracked");
        let limits = size_limits(toplevel.wl_surface());
        let parent = toplevel
            .parent()
            .and_then(|p| self.windows.by_surface(&p))
            .map(|m| m.id);
        (parent.is_some() || limits.is_fixed(), parent, limits)
    }

    /// Handles a commit on a toplevel's root surface (after the buffer was imported).
    pub fn toplevel_committed(&mut self, surface: &WlSurface, has_buffer: bool) {
        let Some(managed) = self.windows.by_surface(surface) else {
            return;
        };
        let (id, window, mapped, server_side) = (
            managed.id,
            managed.window.clone(),
            managed.mapped,
            managed.server_side,
        );
        window.on_commit();
        record!(
            self,
            "commit",
            "window" => id,
            "buffer" => has_buffer,
            "server_side" => server_side,
            "geometry" => {
                let g = window.geometry();
                vec![g.loc.x.into(), g.loc.y.into(), g.size.w.into(), g.size.h.into()]
            },
        );
        let toplevel = window
            .toplevel()
            .expect("only toplevels are tracked")
            .clone();
        let (forced, parent, limits) = self.classify(&window);

        if !mapped && !toplevel.is_initial_configure_sent() {
            // Initial commit (also after an unmap): enter the layout.
            // Rule A: a new tiled window splits the tile under the pointer; a pointer
            // that has not moved since startup counts as none.
            let pointer = self
                .seat
                .get_pointer()
                .filter(|_| self.pointer_has_moved)
                .map(|p| {
                    let at = p.current_location();
                    (at.x.floor() as i32, at.y.floor() as i32)
                });
            let new = NewWindow {
                forced,
                parent,
                limits,
                server_side,
                pointer,
            };
            record_context!(self, "open window");
            // Focus stays where it is until the window maps.
            self.windows.layout.open_unfocused(id, new);
            self.arrange();
            return;
        }
        let mut changed = false;
        if self.windows.layout.contains(id) {
            self.windows.layout.set_limits(id, limits);
            changed |= self.windows.layout.set_forced(id, forced, parent);
        }
        if !mapped {
            if has_buffer {
                self.windows.by_id_mut(id).expect("managed").mapped = true;
                record!(self, "map", "window" => id);
                record_context!(self, "map");
                let geometry = window.geometry().size;
                self.windows.layout.place(id, (geometry.w, geometry.h));
                // A new window gets focus when it maps (the last one to map wins),
                // unless keyboard input came in the last TYPING_GUARD (interim, until
                // xdg-activation tokens, E5): then it maps unfocused in its place.
                if self.focus.guard_on_map() {
                    record!(self, "map_unfocused", "window" => id, "reason" => "typing");
                    tracing::debug!(window = id.0, "mapped unfocused: keyboard input just now");
                } else {
                    self.windows.layout.focus(id);
                }
                changed = true;
            }
        } else if !has_buffer {
            // A null buffer unmaps: leave the layout like a close; the client starts
            // over with an initial commit and is inserted as a new window.
            record!(self, "unmap", "window" => id);
            self.leave_layout(id);
            return;
        } else {
            self.note_client_caught_up(id, &window);
            changed |= self.adopt_client_size(id, &window);
        }
        if changed {
            self.arrange();
        } else {
            // The window geometry may have changed with this commit.
            self.position(id);
            self.schedule_pointer_refresh();
            self.request_redraw();
        }
    }

    /// Logs how long a client showed content of the old size after a resize: from the
    /// configure with the new size to its first buffer of that size.
    fn note_client_caught_up(&mut self, id: WindowId, window: &Window) {
        let size = window.geometry().size;
        let Some(managed) = self.windows.by_id_mut(id) else {
            return;
        };
        if let (Some(sent), Some(at)) = (managed.sent, managed.resize_sent_at)
            && (size.w, size.h) == sent.size()
        {
            managed.resize_sent_at = None;
            tracing::debug!(
                window = id.0,
                configure_to_commit_us = at.elapsed().as_micros() as u64,
                "client caught up"
            );
        }
    }

    /// A floating client may settle on a different size than configured (its own
    /// constraints). Once it has acked our latest configure, its committed geometry
    /// becomes the rectangle; during a resize the edges that do not move stay put.
    fn adopt_client_size(&mut self, id: WindowId, window: &Window) -> bool {
        let layout = &self.windows.layout;
        // Only plain floating windows: snapped, maximized and fullscreen sizes are ours.
        if layout.mode(id) != Some(Mode::Floating)
            || layout.snapped_zone(id).is_some()
            || layout.is_maximized(id)
            || layout.fullscreen() == Some(id)
        {
            return false;
        }
        let (Some(rect), Some(sent)) = (
            self.windows.layout.rect_of(id),
            self.windows.by_id(id).and_then(|m| m.sent),
        ) else {
            return false;
        };
        let toplevel = window.toplevel().expect("only toplevels are tracked");
        let acked = toplevel.current_state().size;
        if acked != Some(Size::from(sent.size())) {
            return false;
        }
        let client = window.geometry().size;
        let decoration = self.windows.layout.decoration(id);
        if (client.w, client.h) == decoration.client_size((rect.w, rect.h))
            || client.w <= 0
            || client.h <= 0
        {
            return false;
        }
        let (w, h) = decoration.outer_size((client.w, client.h));
        let actual = Size::<i32, Logical>::from((w, h));
        let edges = self.windows.resizing(id).unwrap_or_default();
        let x = if edges.left {
            rect.right() - actual.w
        } else {
            rect.x
        };
        let y = if edges.top {
            rect.bottom() - actual.h
        } else {
            rect.y
        };
        self.windows
            .layout
            .set_floating_rect(id, Rect::new(x, y, actual.w, actual.h))
            .is_some()
    }

    /// The toplevel was destroyed (or its client disconnected).
    pub fn forget_toplevel(&mut self, surface: &WlSurface) {
        let Some(id) = self.windows.by_surface(surface).map(|m| m.id) else {
            return;
        };
        record!(self, "destroy", "window" => id);
        self.leave_layout(id);
        self.windows.list.retain(|m| m.id != id);
        self.titles.forget(id);
    }

    fn leave_layout(&mut self, id: WindowId) {
        record_context!(self, "window left the layout");
        self.dismiss_popups(id);
        if self.dragged_window() == Some(id) {
            // The window is going away: no snap on the way out.
            if let Some(drag) = &mut self.windows.drag {
                drag.zone = None;
            }
            if let Some(pointer) = self.seat.get_pointer() {
                pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
            }
            self.windows.drag = None;
        }
        let Some(managed) = self.windows.by_id_mut(id) else {
            return;
        };
        managed.mapped = false;
        managed.sent = None;
        let window = managed.window.clone();
        self.space.unmap_elem(&window);
        self.windows.layout.close(id);
        self.arrange();
    }

    /// Sets the output size (logical pixels) and re-arranges.
    pub fn set_layout_area(&mut self, size: Size<i32, Logical>) {
        let surfaces = self.windows.layer_surfaces.clone();
        self.set_output_and_layers(size, surfaces);
    }

    /// Sets the output's layer surfaces and re-arranges.
    pub fn set_layer_surfaces(&mut self, surfaces: Vec<LayerSurface>) {
        let size = (self.windows.output.w, self.windows.output.h).into();
        self.set_output_and_layers(size, surfaces);
    }

    /// The output size and its layer surfaces together, as one layout update and one
    /// configure batch: a panel change in the same frame as an output resize gives one
    /// result. A real change of the output or the usable area also re-constrains or
    /// dismisses popups.
    pub fn set_output_and_layers(&mut self, size: Size<i32, Logical>, surfaces: Vec<LayerSurface>) {
        let output = Rect::new(0, 0, size.w, size.h);
        let arrangement = layers::arrange(output, &surfaces);
        for id in &arrangement.ignored {
            if !self.windows.arrangement.ignored.contains(id) {
                tracing::warn!(
                    layer_surface = id.0,
                    "exclusive zone ignored: it would reserve more than half the output \
                     or leave less than 640x360"
                );
            }
        }
        let layout = &self.windows.layout;
        let changed = layout.output() != output || layout.area() != arrangement.usable;
        record_context!(self, "output size");
        self.windows.layout.set_geometry(output, arrangement.usable);
        self.windows.output = output;
        self.windows.layer_surfaces = surfaces;
        self.windows.arrangement = arrangement;
        self.arrange();
        if changed {
            self.reconstrain_popups(true);
        }
    }

    /// Applies the layout: configures every window whose size or state changed (one
    /// batch), moves mapped windows into place and updates keyboard focus.
    pub fn arrange(&mut self) {
        let ids: Vec<WindowId> = self.windows.list.iter().map(|m| m.id).collect();
        let active = self.activated_window();
        for id in ids {
            // The size is None (client chooses) for floating windows without a
            // rectangle yet; tiles on tiny outputs can be 0 wide and are sent as 1.
            let Some(presentation) = self.windows.layout.presentation(id) else {
                continue;
            };
            let want = Sent {
                presentation,
                resizing: self.windows.resizing(id).is_some(),
                activated: active == Some(id),
            };
            let managed = self.windows.by_id_mut(id).expect("listed");
            let toplevel = managed
                .window
                .toplevel()
                .expect("only toplevels are tracked")
                .clone();
            let reply = std::mem::take(&mut managed.reply_due);
            if managed.sent != Some(want) || reply || !toplevel.is_initial_configure_sent() {
                if managed.mapped && managed.sent.map(|s| s.size()) != Some(want.size()) {
                    managed.resize_sent_at.get_or_insert_with(Instant::now);
                }
                managed.sent = Some(want);
                let server_side = managed.server_side;
                toplevel.with_pending_state(|state| {
                    state.size = presentation.size.map(Into::into);
                    // Every client that negotiates gets server-side decorations; the
                    // mode goes out with the size in the same configure.
                    state.decoration_mode = server_side.then_some(DecorationMode::ServerSide);
                    let tiled = presentation.tiled;
                    for (on, tiled_state) in [tiled.left, tiled.right, tiled.top, tiled.bottom]
                        .into_iter()
                        .zip(TILED)
                    {
                        set_state(&mut state.states, tiled_state, on);
                    }
                    set_state(
                        &mut state.states,
                        xdg_toplevel::State::Maximized,
                        presentation.maximized,
                    );
                    set_state(
                        &mut state.states,
                        xdg_toplevel::State::Fullscreen,
                        presentation.fullscreen,
                    );
                    set_state(
                        &mut state.states,
                        xdg_toplevel::State::Resizing,
                        want.resizing,
                    );
                    set_state(
                        &mut state.states,
                        xdg_toplevel::State::Activated,
                        want.activated,
                    );
                });
                #[cfg_attr(not(feature = "debug-record"), expect(unused_variables))]
                let serial = if toplevel.is_initial_configure_sent() {
                    toplevel.send_pending_configure()
                } else {
                    Some(toplevel.send_configure())
                };
                record!(
                    self,
                    "configure",
                    "window" => id,
                    "serial" => serial.map(u32::from),
                    "size" => presentation.size.map(|(w, h)| crate::record::Json::from(vec![w.into(), h.into()])),
                    "maximized" => presentation.maximized,
                    "fullscreen" => presentation.fullscreen,
                    "resizing" => want.resizing,
                    "activated" => want.activated,
                );
            }
            self.position(id);
        }
        let backdrop = self.windows.layout.fullscreen_backdrop();
        self.overlay.set_backdrop(backdrop);
        self.reconstrain_popups(false);
        self.sync_keyboard_focus();
        // What is under a still pointer may have changed (it never changes focus).
        self.schedule_pointer_refresh();
        self.request_redraw();
        #[cfg(feature = "debug-record")]
        if self.recorder.is_some() {
            let now = crate::record::layout_json(self);
            if let Some(recorder) = self.recorder.as_mut() {
                recorder.layout_after_arrange(now);
            }
        }
    }

    /// Puts a mapped window at its rectangle in the space (output enter/leave, frame
    /// callbacks). Drawing order comes from the layout, not the space.
    fn position(&mut self, id: WindowId) {
        let Some(managed) = self.windows.by_id(id).filter(|m| m.mapped) else {
            return;
        };
        if let Some(rect) = self.windows.layout.shown_rect(id) {
            let rect = self.windows.layout.decoration(id).inner(rect);
            let window = managed.window.clone();
            self.space
                .map_element(window.clone(), buffer_position(&window, rect), false);
        }
    }

    /// Gives keyboard focus to the layout's focused window if it is mapped, else to
    /// nothing. While that window has a grabbed popup chain, its topmost popup gets
    /// it; a grab on any other window ends.
    pub fn sync_keyboard_focus(&mut self) {
        // A layer surface holding the keyboard comes first (layer_shell.rs).
        let focused = self.layer_keyboard_target().or_else(|| {
            self.windows
                .layout
                .focused()
                .and_then(|id| self.windows.by_id(id))
                .filter(|m| m.mapped)
                .and_then(|m| m.window.toplevel().map(|t| t.wl_surface().clone()))
        });
        let popup_target = self.popup_focus(focused.as_ref());
        #[cfg(feature = "debug-record")]
        let popup = popup_target.is_some();
        let target = popup_target.or(focused);
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if keyboard.current_focus() != target {
            #[cfg(feature = "debug-record")]
            if self.recorder.is_some() {
                let window_of = |s: Option<&WlSurface>| {
                    s.and_then(|s| self.windows.by_surface(s).map(|m| m.id))
                };
                let from = window_of(keyboard.current_focus().as_ref());
                let to = window_of(target.as_ref());
                let reason = self.recorder.as_ref().map(|r| r.context());
                record!(
                    self,
                    "focus",
                    "from" => from,
                    "to" => to,
                    "to_popup" => popup,
                    "reason" => reason,
                );
            }
            keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        }
        self.sync_activated();
    }

    /// Sends the activated state where it changed outside `arrange` (a layer surface
    /// took or gave back the keyboard, a mod+Tab step): set on the window with keyboard
    /// focus, cleared on every other one.
    fn sync_activated(&mut self) {
        let active = self.activated_window();
        for managed in &mut self.windows.list {
            let Some(sent) = managed.sent.as_mut() else {
                continue;
            };
            let want = active == Some(managed.id);
            if sent.activated == want {
                continue;
            }
            sent.activated = want;
            let Some(toplevel) = managed.window.toplevel() else {
                continue;
            };
            if !toplevel.is_initial_configure_sent() {
                continue;
            }
            toplevel.with_pending_state(|state| {
                set_state(&mut state.states, xdg_toplevel::State::Activated, want);
            });
            toplevel.send_pending_configure();
        }
    }

    /// Focuses and raises a window (click or keyboard). Focusing another window ends
    /// fullscreen, which needs configures.
    pub fn focus_window(&mut self, id: WindowId) {
        self.layer_focus_lost();
        if self.windows.layout.focus(id) {
            self.arrange();
        }
    }

    /// The mapped window at a global position, counting its popups: floating windows
    /// above tiled ones.
    /// None where a layer surface above the windows is.
    pub fn window_at(&self, location: Point<f64, Logical>) -> Option<WindowId> {
        if self.layer_above(location).is_some() {
            return None;
        }
        self.hit(location).map(|(id, _)| id)
    }

    /// The surface at a global position and the global position of its origin:
    /// layer surfaces above the windows, the windows, then layer surfaces below.
    pub fn surface_at(&self, location: Point<f64, Logical>) -> Option<SurfaceHit> {
        if let Some((_, surface)) = self.layer_above(location) {
            return Some(surface);
        }
        match self.hit(location) {
            Some((_, surface)) => surface,
            None => self.layer_below(location).map(|(_, surface)| surface),
        }
    }

    /// The surface a drag and drop targets at a global position: the surface there,
    /// or, in a gap or on a window's own decorations (its border on a screen edge,
    /// owner M18a), that window, at the nearest point of its surface (owner, after
    /// M15b step 1: a drop never lands on nothing because of a gap).
    pub fn drop_target_at(&self, location: Point<f64, Logical>) -> Option<SurfaceHit> {
        if let Some(hit) = self.surface_at(location) {
            return Some(hit);
        }
        let (x, y) = (location.x.floor() as i32, location.y.floor() as i32);
        let decorated = || match self.hit(location) {
            Some((id, None)) if self.layer_above(location).is_none() => Some(id),
            _ => None,
        };
        let id = self.windows.layout.gap_owner(x, y).or_else(decorated)?;
        let visible = self.visible_windows().into_iter().find(|v| v.id == id)?;
        let surface = visible.window.toplevel()?.wl_surface().clone();
        let r = visible.rect;
        let clamped = Point::<f64, Logical>::from((
            location
                .x
                .clamp(f64::from(r.x), f64::from(r.right() - 1).max(f64::from(r.x))),
            location.y.clamp(
                f64::from(r.y),
                f64::from(r.bottom() - 1).max(f64::from(r.y)),
            ),
        ));
        // The origin is shifted so that `location - origin` is the clamped point.
        let origin = visible.position.to_f64() + (location - clamped);
        Some((surface, origin))
    }

    /// The layer surface at a global position, if one is the topmost thing there.
    pub fn layer_at(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<crate::layout::layers::LayerId> {
        if let Some((id, _)) = self.layer_above(location) {
            return Some(id);
        }
        match self.hit(location) {
            Some(_) => None,
            None => self.layer_below(location).map(|(id, _)| id),
        }
    }

    /// What is at a global position, top first: a window's popups (anywhere, drawn
    /// directly above it; those of tiled windows above every tiled window), then the
    /// window itself (tiled windows only inside their tile). Returns the window and
    /// the surface under the point with its origin.
    pub fn hit(&self, location: Point<f64, Logical>) -> Option<(WindowId, Option<SurfaceHit>)> {
        let (x, y) = (location.x.floor() as i32, location.y.floor() as i32);
        let layout = &self.windows.layout;
        let target = layout.window_at(x, y);
        let backdrop = layout
            .fullscreen_backdrop()
            .is_some_and(|b| b.contains(x, y));
        let visible_windows = self.visible_windows();
        let popup_hit = |visible: &Visible<'_>| {
            popups_of(visible.window, visible.rect)
                .into_iter()
                .find_map(|(popup, origin)| {
                    under_from_surface_tree(
                        popup.wl_surface(),
                        location,
                        origin,
                        WindowSurfaceType::ALL,
                    )
                    .map(|(surface, at)| (visible.id, Some((surface, at.to_f64()))))
                })
        };
        // Popups of tiled windows are above every tiled window (see render.rs).
        let mut tiled_popups_checked = false;
        for visible in visible_windows.iter().rev() {
            let popups = if visible.layer != Layer::Tiled {
                popup_hit(visible)
            } else if !tiled_popups_checked {
                tiled_popups_checked = true;
                visible_windows
                    .iter()
                    .rev()
                    .filter(|v| v.layer == Layer::Tiled)
                    .find_map(popup_hit)
            } else {
                None
            };
            if popups.is_some() {
                return popups;
            }
            if Some(visible.id) == target {
                // The bar, border and resize zones are the compositor's.
                if self.decoration_part(visible, x, y).is_some() {
                    return Some((visible.id, None));
                }
                let origin = visible.position.to_f64();
                let surface = visible
                    .window
                    .surface_under(
                        location - origin,
                        WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
                    )
                    .map(|(surface, at)| (surface, origin + at.to_f64()));
                return Some((visible.id, surface));
            }
            // The backdrop of a fixed-size fullscreen window hides everything below.
            if backdrop && visible.layer == Layer::Fullscreen {
                return None;
            }
        }
        None
    }

    /// What (x, y) is on a window's decorations; None in the client's area.
    pub fn decoration_part(&self, visible: &Visible<'_>, x: i32, y: i32) -> Option<Part> {
        let layout = &self.windows.layout;
        visible.decoration.hit(
            visible.outer,
            layout.resize_zones(visible.id),
            layout.output(),
            x,
            y,
        )
    }

    /// The decoration under a global position, if the topmost thing there is one
    /// (popups and windows above it win).
    pub fn decoration_at(&self, location: Point<f64, Logical>) -> Option<(WindowId, Part)> {
        if self.layer_above(location).is_some() {
            return None;
        }
        let (id, surface) = self.hit(location)?;
        if surface.is_some() {
            return None;
        }
        let (x, y) = (location.x.floor() as i32, location.y.floor() as i32);
        let visible = self.visible_windows().into_iter().find(|v| v.id == id)?;
        self.decoration_part(&visible, x, y).map(|part| (id, part))
    }

    /// Mapped windows in drawing order, bottom first (see `MonitorLayout::stacking`).
    pub fn visible_windows(&self) -> Vec<Visible<'_>> {
        self.windows
            .layout
            .stacking()
            .into_iter()
            .filter_map(|(id, outer, layer)| {
                let managed = self.windows.by_id(id).filter(|m| m.mapped)?;
                let decoration = self.windows.layout.decoration(id);
                let rect = decoration.inner(outer);
                Some(Visible {
                    id,
                    window: &managed.window,
                    rect,
                    outer,
                    decoration,
                    position: buffer_position(&managed.window, rect),
                    layer,
                })
            })
            .collect()
    }

    /// Starts moving or resizing `id` with the pointer pressed at `pressed_at` (mod+drag
    /// or a client request). Moving a tiled window floats it where it is; resizing a
    /// tiled window moves the splits on the dragged edges (none on the output boundary).
    /// Returns false if the drag does not apply.
    pub fn begin_drag(
        &mut self,
        id: WindowId,
        kind: DragKind,
        pressed_at: Point<f64, Logical>,
    ) -> bool {
        let mut tiled = None;
        match kind {
            DragKind::Move => {
                self.windows.layout.float_in_place(id);
            }
            DragKind::Resize(edges) => {
                if self.windows.layout.mode(id) == Some(Mode::Tiled) {
                    match self.windows.layout.begin_tiled_resize(id, edges) {
                        Some(resize) => tiled = Some(resize),
                        None => return false,
                    }
                }
            }
        }
        self.windows.layout.focus(id);
        let Some(from) = self.windows.layout.rect_of(id) else {
            return false;
        };
        self.windows.drag = Some(Drag {
            window: id,
            kind,
            tiled,
            from,
            pressed_at,
            zone: None,
            last: None,
        });
        record!(self, "grab_start", "grab" => format!("{kind:?}"), "window" => id);
        record_context!(self, format!("{kind:?} drag start"));
        self.arrange();
        // The rest of the drag is one layout operation, recorded when it ends.
        #[cfg(feature = "debug-record")]
        if let Some(recorder) = self.recorder.as_mut() {
            recorder.set_in_drag(true);
        }
        true
    }

    /// The pointer moved to `now` during a drag. Returns false if the dragged window is
    /// gone (the drag must end). `free` (Shift held) disables edge snapping.
    pub fn drag_motion(&mut self, now: Point<f64, Logical>, free: bool) -> bool {
        let Windows { layout, drag, .. } = &mut self.windows;
        let Some(drag) = drag.as_mut() else {
            return false;
        };
        let id = drag.window;
        if !layout.contains(id) {
            return false;
        }
        if drag.last.replace((now, free)) == Some((now, free)) {
            return true;
        }
        let (px, py) = (now.x.floor() as i32, now.y.floor() as i32);
        let grabbed = (
            drag.pressed_at.x.floor() as i32,
            drag.pressed_at.y.floor() as i32,
        );
        // Pulling a snapped window out of its zone: pre-snap size, with the grab point
        // under the pointer.
        if matches!(drag.kind, DragKind::Move)
            && now != drag.pressed_at
            && let Some(rect) = layout.unsnap_for_drag(id, grabbed, (px, py))
        {
            drag.from = rect;
            drag.pressed_at = now;
        }
        let dx = (now.x - drag.pressed_at.x).round() as i32;
        let dy = (now.y - drag.pressed_at.y).round() as i32;
        let from = drag.from;
        let area = layout.area();
        let others: Vec<Rect> = layout
            .stacking()
            .into_iter()
            .filter(|(w, _, l)| *w != id && *l != Layer::Fullscreen)
            .map(|(_, r, _)| r)
            .collect();
        if let Some(resize) = &drag.tiled {
            // A tiled resize: the splits follow the pointer; no edge snapping or zones.
            layout.drag_tiled_resize(resize, dx, dy);
            self.windows.note_input(Instant::now());
            self.schedule_arrange();
            return true;
        }
        let rect = match drag.kind {
            DragKind::Move => {
                // The current size: a client may have settled on its own size since
                // the press (e.g. a terminal's cell grid after floating).
                let (w, h) = layout.rect_of(id).map_or((from.w, from.h), |r| (r.w, r.h));
                let moved = Rect::new(from.x + dx, from.y + dy, w, h);
                let fixed = layout.limits(id).is_fixed();
                drag.zone = (!fixed)
                    // Triggers at the true output edge; the zone's rectangle is in
                    // the usable area.
                    .then(|| layout.zones().at(layout.output(), px, py).map(|z| z.id))
                    .flatten();
                if free {
                    moved
                } else {
                    snap_to_edges(moved, Edges::ALL, area, &others)
                }
            }
            DragKind::Resize(edges) => {
                let limits = layout.limits(id);
                let resized = resize_rect(from, edges, dx, dy, limits);
                let resized = if free {
                    resized
                } else {
                    snap_to_edges(resized, edges, area, &others)
                };
                stop_top_edge(resized, edges, area, limits)
            }
        };
        layout.set_floating_rect(id, rect);
        // The rectangle the window will take: the zone inset by the gap.
        let preview = drag.zone.and_then(|z| layout.zone_gapped(z));
        self.overlay.set_preview(preview);
        self.windows.note_input(Instant::now());
        self.schedule_arrange();
        true
    }

    /// Pointer drags change the layout on every motion; their configures go out at most
    /// once per frame: right before the next frame is drawn, or after FRAME_FALLBACK if
    /// no frame comes (the output is not presenting). Rendering already uses the new
    /// rectangles.
    fn schedule_arrange(&mut self) {
        self.windows.arrange_pending = true;
        self.request_redraw();
        if self.windows.fallback_armed {
            return;
        }
        self.windows.fallback_armed = true;
        let timer = Timer::from_duration(FRAME_FALLBACK);
        let inserted = self.loop_handle.insert_source(timer, |_, _, server| {
            server.state.windows.fallback_armed = false;
            server.state.flush_pending_arrange();
            server.flush();
            TimeoutAction::Drop
        });
        if inserted.is_err() {
            self.windows.fallback_armed = false;
            self.arrange();
        }
    }

    /// Sends the configures a drag scheduled, if any. Called before drawing a frame.
    pub fn flush_pending_arrange(&mut self) {
        if std::mem::take(&mut self.windows.arrange_pending) {
            self.arrange();
        }
    }

    /// A drag ended: a move released in a zone snaps the window there.
    pub fn end_drag(&mut self) {
        let Some(drag) = self.windows.drag.take() else {
            return;
        };
        if let (DragKind::Move, Some(zone)) = (drag.kind, drag.zone) {
            self.windows.layout.snap(drag.window, zone);
        }
        record!(
            self,
            "grab_end",
            "grab" => format!("{:?}", drag.kind),
            "window" => drag.window,
            "snap_zone" => drag.zone.map(|z| format!("{z:?}")),
        );
        #[cfg(feature = "debug-record")]
        if let Some(recorder) = self.recorder.as_mut() {
            recorder.set_in_drag(false);
            recorder.set_context(format!("{:?} drag", drag.kind));
        }
        self.overlay.set_preview(None);
        self.windows.arrange_pending = false;
        self.arrange();
    }

    /// The preview rectangle of the zone the current drag targets (benchmarks).
    #[cfg(test)]
    pub fn overlay_preview_for_bench(&self) -> Option<crate::layout::gaps::Gapped> {
        self.windows
            .drag
            .as_ref()
            .and_then(|d| d.zone)
            .and_then(|z| self.windows.layout.zone_gapped(z))
    }

    /// The window being dragged, if any.
    pub fn dragged_window(&self) -> Option<WindowId> {
        self.windows.drag.as_ref().map(|d| d.window)
    }

    /// A client's set/unset_maximized or set/unset_fullscreen. The client always gets a
    /// configure in reply, even if the request is refused.
    pub fn client_state_request(&mut self, surface: &WlSurface, request: StateRequest) {
        let Some(id) = self.windows.by_surface(surface).map(|m| m.id) else {
            return;
        };
        record_context!(self, format!("client request {request:?}"));
        let layout = &mut self.windows.layout;
        match request {
            StateRequest::Maximize(on) => layout.set_maximized(id, on),
            StateRequest::Fullscreen(on) => layout.set_fullscreen(id, on),
        };
        if let Some(managed) = self.windows.by_id_mut(id) {
            managed.reply_due = true;
        }
        self.arrange();
    }

    /// The edges a mod+right-drag starting at `location` resizes.
    pub fn resize_edges_at(&self, id: WindowId, location: Point<f64, Logical>) -> Option<Edges> {
        let rect = self.windows.layout.rect_of(id)?;
        Some(resize_zone(
            rect,
            location.x.floor() as i32,
            location.y.floor() as i32,
        ))
    }

    /// Runs a keybind action.
    pub fn run_action(&mut self, action: Action) {
        let start = Instant::now();
        record_context!(self, format!("keybind {action:?}"));
        let focused = self.windows.layout.focused();
        // Every action first dismisses the focused window's popups, except a resize
        // (they follow it) and the default-mode toggle (nothing on screen changes).
        if !matches!(action, Action::Resize(_) | Action::ToggleDefaultMode)
            && let Some(id) = focused
        {
            self.dismiss_popups(id);
        }
        match action {
            Action::SpawnTerminal => self.spawn_terminal(),
            Action::CloseFocused => {
                if let Some(managed) = focused.and_then(|id| self.windows.by_id(id)) {
                    self.request_close(&managed.window.clone());
                }
            }
            Action::Focus(direction) => {
                if let Some(next) =
                    focused.and_then(|id| self.windows.layout.neighbour(id, direction))
                {
                    self.focus_window(next);
                }
            }
            Action::Swap(direction) => {
                if self.windows.layout.swap_focused(direction) {
                    self.arrange();
                }
            }
            Action::Quit => self.loop_signal.stop(),
            Action::ToggleFloating => {
                if let Some(id) = focused
                    && self.windows.layout.toggle(id)
                {
                    self.arrange();
                }
            }
            Action::FloatAll => {
                self.windows.layout.float_all();
                self.arrange();
            }
            Action::TileAll => {
                self.windows.layout.tile_all();
                self.arrange();
            }
            Action::Move(direction) => {
                if let Some(id) = focused {
                    let (dx, dy) = step(direction);
                    self.windows.layout.move_by(id, dx, dy);
                    self.arrange();
                }
            }
            Action::Resize(direction) => {
                if let Some(id) = focused {
                    let (dw, dh) = step(direction);
                    self.windows.layout.resize_by(id, dw, dh);
                    self.arrange();
                }
            }
            Action::Snap(direction) => {
                if let Some(id) = focused
                    && self.windows.layout.snap_key(id, direction)
                {
                    self.arrange();
                }
            }
            Action::SnapQuarter(zone) => {
                if let Some(id) = focused
                    && self.windows.layout.snap_quarter(id, zone)
                {
                    self.arrange();
                }
            }
            Action::CycleNext => self.cycle(1),
            Action::CyclePrevious => self.cycle(-1),
            Action::ToggleDefaultMode => {
                let layout = &mut self.windows.layout;
                let mode = match layout.default_mode() {
                    Mode::Tiled => Mode::Floating,
                    Mode::Floating => Mode::Tiled,
                };
                layout.set_default_mode(mode);
                tracing::info!(?mode, "default mode");
            }
            // Pointer rows of the table; seat.rs handles them on button presses.
            Action::MoveDrag | Action::ResizeDrag | Action::ClickFocus => return,
        }
        self.windows.note_input(start);
        tracing::debug!(
            ?action,
            layout_us = start.elapsed().as_micros() as u64,
            "keybind"
        );
    }

    /// One step of mod+Tab: walks a most-recently-used snapshot taken at the first
    /// press, focusing and raising without recording; `commit_cycle` ends it.
    fn cycle(&mut self, step: isize) {
        let cycle = self.keybinds.cycle.get_or_insert_with(|| Cycle {
            order: self.windows.layout.recent_first(),
            index: 0,
        });
        cycle.order.retain(|id| self.windows.layout.contains(*id));
        if cycle.order.is_empty() {
            self.keybinds.cycle = None;
            return;
        }
        let len = cycle.order.len() as isize;
        cycle.index = ((cycle.index as isize + step).rem_euclid(len)) as usize;
        let id = cycle.order[cycle.index];
        self.windows.layout.preview_focus(id);
        self.sync_keyboard_focus();
        self.request_redraw();
    }

    /// Ends a mod+Tab cycle when the modifier is released: the chosen window becomes
    /// the most recent.
    pub fn commit_cycle(&mut self) {
        if self.keybinds.cycle.take().is_some() {
            // The pointer stays held until its first real motion (owner, M19a).
            self.focus.cycle_ended();
            self.windows.layout.commit_focus();
            self.arrange();
        }
    }

    /// Asks the client to close the window (xdg_toplevel.close). The window leaves the
    /// layout when the client destroys the toplevel.
    pub fn request_close(&self, window: &Window) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_close();
        }
    }

    #[cfg(not(test))]
    fn spawn_terminal(&mut self) {
        let spawned = Command::new("foot")
            .env("WAYLAND_DISPLAY", &self.keybinds.display_name)
            .spawn();
        match spawned {
            // Reap the child when it exits so it does not linger as a zombie.
            Ok(mut child) => {
                // Its first window may take focus although keys were typed just
                // before (the typing guard's exemption).
                self.focus.note_spawn();
                drop(std::thread::spawn(move || child.wait()));
            }
            Err(err) => tracing::warn!(%err, "could not start foot"),
        }
    }

    /// Tests start no foot: the display it would connect to is recorded.
    #[cfg(test)]
    fn spawn_terminal(&mut self) {
        self.terminals_spawned
            .borrow_mut()
            .push(self.keybinds.display_name.clone());
        self.focus.note_spawn();
    }
}

/// A client request to change its window state.
#[derive(Clone, Copy, Debug)]
pub enum StateRequest {
    Maximize(bool),
    Fullscreen(bool),
}

fn set_state(
    states: &mut smithay::wayland::shell::xdg::ToplevelStateSet,
    state: xdg_toplevel::State,
    on: bool,
) {
    if on {
        states.set(state);
    } else {
        states.unset(state);
    }
}

/// Keyboard move/resize step for a direction (resize: Right wider, Left narrower,
/// Down taller, Up shorter).
fn step(direction: Direction) -> (i32, i32) {
    match direction {
        Direction::Left => (-STEP, 0),
        Direction::Right => (STEP, 0),
        Direction::Up => (0, -STEP),
        Direction::Down => (0, STEP),
    }
}
