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

use std::{
    process::Command,
    time::{Duration, Instant},
};

use smithay::{
    desktop::{Window, WindowSurfaceType, utils::under_from_surface_tree},
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Size},
    wayland::{compositor, shell::xdg::SurfaceCachedState},
};

use crate::{
    keybinds::{Action, Cycle},
    layout::{
        Direction, Edges, Layer, Mode, MonitorLayout, NewWindow, Presentation, Rect, STEP,
        SizeLimits, TiledResize, WindowId, ZoneId, resize_rect, resize_zone, snap_to_edges,
        stop_top_edge,
    },
    popups::popups_of,
    state::Sash,
};

/// How long drag configures wait for a frame before they are sent anyway.
const FRAME_FALLBACK: Duration = Duration::from_millis(16);

/// What was last sent to a client in a configure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sent {
    presentation: Presentation,
    resizing: bool,
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
}

impl Windows {
    pub fn new() -> Self {
        Self {
            layout: MonitorLayout::new(Rect::default()),
            list: Vec::new(),
            next_id: 1,
            drag: None,
            arrange_pending: false,
            fallback_armed: false,
            swallowed_buttons: Vec::new(),
            input_pending_since: None,
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

    pub fn by_id(&self, id: WindowId) -> Option<&Managed> {
        self.list.iter().find(|m| m.id == id)
    }

    fn by_id_mut(&mut self, id: WindowId) -> Option<&mut Managed> {
        self.list.iter_mut().find(|m| m.id == id)
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
type SurfaceHit = (WlSurface, Point<f64, Logical>);

/// A window drawn on the output.
pub struct Visible<'a> {
    pub id: WindowId,
    pub window: &'a Window,
    pub rect: Rect,
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
        let (id, window, mapped) = (managed.id, managed.window.clone(), managed.mapped);
        window.on_commit();
        record!(
            self,
            "commit",
            "window" => id,
            "buffer" => has_buffer,
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
                pointer,
            };
            record_context!(self, "open window");
            self.windows.layout.open(id, new);
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
        let actual = window.geometry().size;
        if (actual.w, actual.h) == (rect.w, rect.h) || actual.w <= 0 || actual.h <= 0 {
            return false;
        }
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

    /// Sets the area the layout covers (the output, logical pixels) and re-arranges.
    /// A real size change also re-constrains or dismisses popups.
    pub fn set_layout_area(&mut self, size: Size<i32, Logical>) {
        let area = Rect::new(0, 0, size.w, size.h);
        let changed = self.windows.layout.area() != area;
        record_context!(self, "output size");
        self.windows.layout.set_area(area);
        self.arrange();
        if changed {
            self.reconstrain_popups(true);
        }
    }

    /// Applies the layout: configures every window whose size or state changed (one
    /// batch), moves mapped windows into place and updates keyboard focus.
    pub fn arrange(&mut self) {
        let ids: Vec<WindowId> = self.windows.list.iter().map(|m| m.id).collect();
        for id in ids {
            // The size is None (client chooses) for floating windows without a
            // rectangle yet; tiles on tiny outputs can be 0 wide and are sent as 1.
            let Some(presentation) = self.windows.layout.presentation(id) else {
                continue;
            };
            let want = Sent {
                presentation,
                resizing: self.windows.resizing(id).is_some(),
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
                toplevel.with_pending_state(|state| {
                    state.size = presentation.size.map(Into::into);
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
                );
            }
            self.position(id);
        }
        let backdrop = self.windows.layout.fullscreen_backdrop();
        self.overlay.set_backdrop(backdrop);
        self.reconstrain_popups(false);
        self.sync_keyboard_focus();
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
            let window = managed.window.clone();
            self.space
                .map_element(window.clone(), buffer_position(&window, rect), false);
        }
    }

    /// Gives keyboard focus to the layout's focused window if it is mapped, else to
    /// nothing. While that window has a grabbed popup chain, its topmost popup gets
    /// it; a grab on any other window ends.
    pub fn sync_keyboard_focus(&mut self) {
        let focused = self
            .windows
            .layout
            .focused()
            .and_then(|id| self.windows.by_id(id))
            .filter(|m| m.mapped)
            .and_then(|m| m.window.toplevel().map(|t| t.wl_surface().clone()));
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
    }

    /// Focuses and raises a window (click or keyboard). Focusing another window ends
    /// fullscreen, which needs configures.
    pub fn focus_window(&mut self, id: WindowId) {
        if self.windows.layout.focus(id) {
            self.arrange();
        }
    }

    /// The mapped window at a global position, counting its popups: floating windows
    /// above tiled ones.
    pub fn window_at(&self, location: Point<f64, Logical>) -> Option<WindowId> {
        self.hit(location).map(|(id, _)| id)
    }

    /// The surface at a global position and the global position of its origin.
    pub fn surface_at(&self, location: Point<f64, Logical>) -> Option<SurfaceHit> {
        self.hit(location).and_then(|(_, surface)| surface)
    }

    /// What is at a global position, top first: a window's popups (anywhere, drawn
    /// directly above it; those of tiled windows above every tiled window), then the
    /// window itself (tiled windows only inside their tile). Returns the window and
    /// the surface under the point with its origin.
    fn hit(&self, location: Point<f64, Logical>) -> Option<(WindowId, Option<SurfaceHit>)> {
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

    /// Mapped windows in drawing order, bottom first (see `MonitorLayout::stacking`).
    pub fn visible_windows(&self) -> Vec<Visible<'_>> {
        self.windows
            .layout
            .stacking()
            .into_iter()
            .filter_map(|(id, rect, layer)| {
                let managed = self.windows.by_id(id).filter(|m| m.mapped)?;
                Some(Visible {
                    id,
                    window: &managed.window,
                    rect,
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
                    .then(|| layout.zones().at(area, px, py).map(|z| z.id))
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
        let preview = drag.zone.and_then(|z| layout.zone_rect(z));
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
    pub fn overlay_preview_for_bench(&self) -> Option<Rect> {
        self.windows
            .drag
            .as_ref()
            .and_then(|d| d.zone)
            .and_then(|z| self.windows.layout.zone_rect(z))
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

    fn spawn_terminal(&self) {
        let spawned = Command::new("foot")
            .env("WAYLAND_DISPLAY", &self.keybinds.display_name)
            .spawn();
        match spawned {
            // Reap the child when it exits so it does not linger as a zombie.
            Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
            Err(err) => tracing::warn!(%err, "could not start foot"),
        }
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
