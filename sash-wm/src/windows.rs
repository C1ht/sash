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
//! windows are drawn at their tile origin and clipped to the tile; floating windows are
//! drawn unclipped at their rectangle's origin.

use std::{process::Command, time::Instant};

use smithay::{
    desktop::{Window, WindowSurfaceType},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Size},
    wayland::{compositor, shell::xdg::SurfaceCachedState},
};

use crate::{
    keybinds::{Action, Cycle},
    layout::{
        Direction, Edges, Mode, MonitorLayout, NewWindow, Rect, STEP, SizeLimits, WindowId,
        resize_rect, resize_zone,
    },
    state::Sash,
};

/// What was last sent to a client in a configure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sent {
    size: (i32, i32),
    tiled: bool,
    resizing: bool,
}

/// A toplevel known to the compositor.
pub struct Managed {
    pub id: WindowId,
    pub window: Window,
    /// Whether the client has committed a buffer since entering the layout.
    pub mapped: bool,
    sent: Option<Sent>,
    /// When a configure with a new size was sent and not yet answered by a buffer of
    /// that size (stale-content measurement).
    resize_sent_at: Option<Instant>,
}

/// Where the pointer was pressed to start a drag and where it is now.
#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub pressed_at: Point<f64, Logical>,
    pub now: Point<f64, Logical>,
}

/// An interactive move or resize in progress.
#[derive(Clone, Copy, Debug)]
pub enum DragKind {
    Move,
    Resize(Edges),
}

pub struct Windows {
    pub layout: MonitorLayout,
    list: Vec<Managed>,
    next_id: u64,
    /// The window being resized interactively and the edges that move.
    resizing: Option<(WindowId, Edges)>,
    /// The window a pointer grab is dragging.
    pub dragging: Option<WindowId>,
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
            resizing: None,
            dragging: None,
            swallowed_buttons: Vec::new(),
            input_pending_since: None,
        }
    }

    #[cfg(test)]
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

/// A window drawn on the output.
pub struct Visible<'a> {
    pub window: &'a Window,
    pub rect: Rect,
    pub position: Point<i32, Logical>,
    pub mode: Mode,
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
        let toplevel = window
            .toplevel()
            .expect("only toplevels are tracked")
            .clone();
        let (forced, parent, limits) = self.classify(&window);

        if !mapped && !toplevel.is_initial_configure_sent() {
            // Initial commit (also after an unmap): enter the layout.
            let new = NewWindow {
                forced,
                parent,
                limits,
            };
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
                let geometry = window.geometry().size;
                self.windows.layout.place(id, (geometry.w, geometry.h));
                changed = true;
            }
        } else if !has_buffer {
            // A null buffer unmaps: leave the layout like a close; the client starts
            // over with an initial commit and is inserted as a new window.
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
            && (size.w, size.h) == sent.size
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
        if self.windows.layout.mode(id) != Some(Mode::Floating) {
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
        if acked != Some(Size::from(sent.size)) {
            return false;
        }
        let actual = window.geometry().size;
        if (actual.w, actual.h) == (rect.w, rect.h) || actual.w <= 0 || actual.h <= 0 {
            return false;
        }
        let edges = match self.windows.resizing {
            Some((resizing, edges)) if resizing == id => edges,
            _ => Edges::default(),
        };
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
        self.leave_layout(id);
        self.windows.list.retain(|m| m.id != id);
    }

    fn leave_layout(&mut self, id: WindowId) {
        if self.dragged_window() == Some(id)
            && let Some(pointer) = self.seat.get_pointer()
        {
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
        }
        if self.windows.resizing.is_some_and(|(w, _)| w == id) {
            self.windows.resizing = None;
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
    pub fn set_layout_area(&mut self, size: Size<i32, Logical>) {
        self.windows
            .layout
            .set_area(Rect::new(0, 0, size.w, size.h));
        self.arrange();
    }

    /// Applies the layout: configures every window whose size or state changed (one
    /// batch), moves mapped windows into place and updates keyboard focus.
    pub fn arrange(&mut self) {
        let ids: Vec<WindowId> = self.windows.list.iter().map(|m| m.id).collect();
        for id in ids {
            let Some(mode) = self.windows.layout.mode(id) else {
                continue;
            };
            let rect = self.windows.layout.rect_of(id);
            let resizing = self.windows.resizing.is_some_and(|(w, _)| w == id);
            let want = Sent {
                // A zero size means "client chooses": used for floating windows that
                // have no rectangle yet. Tiles on tiny outputs can be 0; send 1.
                size: rect.map_or((0, 0), |r| (r.w.max(1), r.h.max(1))),
                tiled: mode == Mode::Tiled,
                resizing,
            };
            let managed = self.windows.by_id_mut(id).expect("listed");
            let toplevel = managed
                .window
                .toplevel()
                .expect("only toplevels are tracked")
                .clone();
            if managed.sent != Some(want) || !toplevel.is_initial_configure_sent() {
                if managed.mapped && managed.sent.map(|s| s.size) != Some(want.size) {
                    managed.resize_sent_at.get_or_insert_with(Instant::now);
                }
                managed.sent = Some(want);
                toplevel.with_pending_state(|state| {
                    state.size = (want.size != (0, 0)).then(|| want.size.into());
                    for tiled in TILED {
                        if want.tiled {
                            state.states.set(tiled);
                        } else {
                            state.states.unset(tiled);
                        }
                    }
                    if want.resizing {
                        state.states.set(xdg_toplevel::State::Resizing);
                    } else {
                        state.states.unset(xdg_toplevel::State::Resizing);
                    }
                });
                if toplevel.is_initial_configure_sent() {
                    toplevel.send_pending_configure();
                } else {
                    toplevel.send_configure();
                }
            }
            self.position(id);
        }
        self.sync_keyboard_focus();
        self.request_redraw();
    }

    /// Puts a mapped window at its rectangle in the space (output enter/leave, frame
    /// callbacks). Drawing order comes from the layout, not the space.
    fn position(&mut self, id: WindowId) {
        let Some(managed) = self.windows.by_id(id).filter(|m| m.mapped) else {
            return;
        };
        if let Some(rect) = self.windows.layout.rect_of(id) {
            let window = managed.window.clone();
            self.space
                .map_element(window.clone(), buffer_position(&window, rect), false);
        }
    }

    /// Gives keyboard focus to the layout's focused window if it is mapped, else to nothing.
    pub fn sync_keyboard_focus(&mut self) {
        let target = self
            .windows
            .layout
            .focused()
            .and_then(|id| self.windows.by_id(id))
            .filter(|m| m.mapped)
            .and_then(|m| m.window.toplevel().map(|t| t.wl_surface().clone()));
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if keyboard.current_focus() != target {
            keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        }
    }

    /// Focuses and raises a window (click or keyboard).
    pub fn focus_window(&mut self, id: WindowId) {
        if self.windows.layout.focus(id) {
            self.sync_keyboard_focus();
            self.request_redraw();
        }
    }

    /// The mapped window at a global position: floating windows above tiled ones.
    pub fn window_at(&self, location: Point<f64, Logical>) -> Option<WindowId> {
        let (x, y) = (location.x.floor() as i32, location.y.floor() as i32);
        let id = self.windows.layout.window_at(x, y)?;
        self.windows.by_id(id).filter(|m| m.mapped).map(|m| m.id)
    }

    /// The surface at a global position and the global position of its origin. Tiled
    /// windows can only be hit inside their tile.
    pub fn surface_at(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let id = self.window_at(location)?;
        let managed = self.windows.by_id(id)?;
        let rect = self.windows.layout.rect_of(id)?;
        let window_pos = buffer_position(&managed.window, rect).to_f64();
        let (surface, pos_in_window) = managed
            .window
            .surface_under(location - window_pos, WindowSurfaceType::ALL)?;
        Some((surface, window_pos + pos_in_window.to_f64()))
    }

    /// Mapped windows in drawing order, bottom first: tiled, then floating by stacking.
    pub fn visible_windows(&self) -> Vec<Visible<'_>> {
        let layout = &self.windows.layout;
        layout
            .tiled_rects()
            .into_iter()
            .chain(layout.floating_rects())
            .filter_map(|(id, rect)| {
                let managed = self.windows.by_id(id).filter(|m| m.mapped)?;
                Some(Visible {
                    window: &managed.window,
                    rect,
                    position: buffer_position(&managed.window, rect),
                    mode: layout.mode(id)?,
                })
            })
            .collect()
    }

    /// Starts moving or resizing `id` (mod+drag, or a client request). Moving a tiled
    /// window floats it where it is; resizing only applies to floating windows.
    /// Returns the rectangle the drag starts from.
    pub fn begin_drag(&mut self, id: WindowId, kind: DragKind) -> Option<Rect> {
        match kind {
            DragKind::Move => {
                self.windows.layout.float_in_place(id);
            }
            DragKind::Resize(edges) => {
                if self.windows.layout.mode(id) != Some(Mode::Floating) {
                    return None;
                }
                self.windows.resizing = Some((id, edges));
            }
        }
        self.windows.layout.focus(id);
        let rect = self.windows.layout.rect_of(id);
        self.arrange();
        rect
    }

    /// Applies a drag: `start` was the window's rectangle when the pointer was pressed,
    /// and the window follows the pointer's offset since then. Returns false if the
    /// window is gone (the drag must end).
    pub fn drag_to(&mut self, id: WindowId, kind: DragKind, start: Rect, pointer: Pointer) -> bool {
        if !self.windows.layout.contains(id) {
            return false;
        }
        let dx = (pointer.now.x - pointer.pressed_at.x).round() as i32;
        let dy = (pointer.now.y - pointer.pressed_at.y).round() as i32;
        let rect = match kind {
            DragKind::Move => Rect::new(start.x + dx, start.y + dy, start.w, start.h),
            DragKind::Resize(edges) => {
                resize_rect(start, edges, dx, dy, self.windows.layout.limits(id))
            }
        };
        self.windows.layout.set_floating_rect(id, rect);
        self.windows.note_input(Instant::now());
        self.arrange();
        true
    }

    /// A drag ended.
    pub fn end_drag(&mut self, id: WindowId) {
        if self.windows.resizing.is_some_and(|(w, _)| w == id) {
            self.windows.resizing = None;
            self.arrange();
        }
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
        let focused = self.windows.layout.focused();
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
