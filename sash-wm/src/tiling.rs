// SPDX-License-Identifier: Apache-2.0

//! Connects the BSP layout to Wayland windows: window ids, configures for layout
//! changes, window positions, keyboard focus and the layout keybind actions.
//!
//! A toplevel enters the layout on its initial commit, so the initial configure
//! already carries its tile size. It is drawn once it commits a buffer. It leaves the
//! layout when it unmaps (null buffer) or is destroyed.
//!
//! Resize synchronisation: every window whose tile changed gets its configure in the
//! same batch. Each window is always drawn at its tile's origin and clipped to its
//! tile, so a window that has not yet redrawn at the new size shows its last buffer
//! cut to the new tile, never over a neighbour.

use std::{process::Command, time::Instant};

use smithay::{
    desktop::{Window, WindowSurfaceType},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Size},
};

use crate::{
    keybinds::Action,
    layout::{BspTree, Layout, Rect, WindowId},
    state::Sash,
};

/// A toplevel known to the compositor.
pub struct Tile {
    pub id: WindowId,
    pub window: Window,
    /// The rectangle last sent to the client; `None` while not in the layout.
    pub rect: Option<Rect>,
    /// Whether the client has committed a buffer since entering the layout.
    pub mapped: bool,
}

pub struct Tiling {
    pub layout: BspTree,
    tiles: Vec<Tile>,
    next_id: u64,
}

impl Tiling {
    pub fn new() -> Self {
        Self {
            layout: BspTree::new(Rect::default()),
            tiles: Vec::new(),
            next_id: 1,
        }
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile> {
        self.tiles.iter()
    }

    pub fn by_surface(&self, surface: &WlSurface) -> Option<&Tile> {
        self.tiles
            .iter()
            .find(|t| is_toplevel_of(&t.window, surface))
    }

    fn by_id(&self, id: WindowId) -> Option<&Tile> {
        self.tiles.iter().find(|t| t.id == id)
    }

    fn by_id_mut(&mut self, id: WindowId) -> Option<&mut Tile> {
        self.tiles.iter_mut().find(|t| t.id == id)
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

/// Where the window's buffer goes so that its visible geometry starts at the tile's
/// origin (clients may draw shadows outside their window geometry).
fn window_position(window: &Window, tile: Rect) -> Point<i32, Logical> {
    Point::from((tile.x, tile.y)) - window.geometry().loc
}

impl Sash {
    /// A new toplevel: known, but not in the layout until its initial commit.
    pub fn add_toplevel(&mut self, window: Window) {
        let id = WindowId(self.tiling.next_id);
        self.tiling.next_id += 1;
        self.tiling.tiles.push(Tile {
            id,
            window,
            rect: None,
            mapped: false,
        });
    }

    /// Handles a commit on a toplevel's root surface (after the buffer was imported).
    pub fn toplevel_committed(&mut self, surface: &WlSurface, has_buffer: bool) {
        let Some(tile) = self.tiling.by_surface(surface) else {
            return;
        };
        let (id, window, mapped) = (tile.id, tile.window.clone(), tile.mapped);
        window.on_commit();
        let toplevel = window.toplevel().expect("only toplevels are tracked");

        if !mapped {
            if !toplevel.is_initial_configure_sent() {
                // Initial commit (also after an unmap): enter the layout. The initial
                // configure carries the tile size.
                self.tiling.layout.insert(id);
                self.arrange();
            } else if has_buffer {
                self.tiling.by_id_mut(id).expect("tile exists").mapped = true;
                if let Some(rect) = self.tiling.by_id(id).and_then(|t| t.rect) {
                    self.space
                        .map_element(window.clone(), window_position(&window, rect), false);
                }
                self.sync_keyboard_focus();
                self.request_redraw();
            }
        } else if !has_buffer {
            // A null buffer unmaps: leave the layout like a close; the client starts
            // over with an initial commit and is inserted as a new window.
            self.leave_layout(id);
        } else {
            // The window geometry may have changed with this commit.
            if let Some(rect) = self.tiling.by_id(id).and_then(|t| t.rect) {
                self.space
                    .map_element(window.clone(), window_position(&window, rect), false);
            }
            self.request_redraw();
        }
    }

    /// The toplevel was destroyed (or its client disconnected).
    pub fn forget_toplevel(&mut self, surface: &WlSurface) {
        let Some(id) = self.tiling.by_surface(surface).map(|t| t.id) else {
            return;
        };
        self.leave_layout(id);
        self.tiling.tiles.retain(|t| t.id != id);
    }

    fn leave_layout(&mut self, id: WindowId) {
        let Some(tile) = self.tiling.by_id_mut(id) else {
            return;
        };
        tile.mapped = false;
        tile.rect = None;
        let window = tile.window.clone();
        self.space.unmap_elem(&window);
        self.tiling.layout.remove(id);
        self.arrange();
    }

    /// Sets the area the layout covers (the output, logical pixels) and re-arranges.
    pub fn set_layout_area(&mut self, size: Size<i32, Logical>) {
        self.tiling.layout.set_area(Rect::new(0, 0, size.w, size.h));
        self.arrange();
    }

    /// Applies the layout: configures every window whose tile changed (one batch),
    /// moves mapped windows to their tiles and updates keyboard focus.
    pub fn arrange(&mut self) {
        let rects = self.tiling.layout.rects();
        for (id, rect) in rects {
            let Some(tile) = self.tiling.by_id_mut(id) else {
                continue;
            };
            let changed = tile.rect != Some(rect);
            tile.rect = Some(rect);
            let (window, mapped) = (tile.window.clone(), tile.mapped);
            let toplevel = window.toplevel().expect("only toplevels are tracked");
            if changed || !toplevel.is_initial_configure_sent() {
                toplevel.with_pending_state(|state| {
                    // A zero size would mean "client chooses"; tiles on tiny outputs
                    // can be zero, so send at least 1.
                    state.size = Some((rect.w.max(1), rect.h.max(1)).into());
                    for tiled in TILED {
                        state.states.set(tiled);
                    }
                });
                if toplevel.is_initial_configure_sent() {
                    toplevel.send_pending_configure();
                } else {
                    toplevel.send_configure();
                }
            }
            if mapped {
                self.space
                    .map_element(window.clone(), window_position(&window, rect), false);
            }
        }
        self.sync_keyboard_focus();
        self.request_redraw();
    }

    /// Gives keyboard focus to the layout's focused window if it is mapped, else to nothing.
    pub fn sync_keyboard_focus(&mut self) {
        let target = self
            .tiling
            .layout
            .focused()
            .and_then(|id| self.tiling.by_id(id))
            .filter(|tile| tile.mapped)
            .and_then(|tile| tile.window.toplevel().map(|t| t.wl_surface().clone()));
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if keyboard.current_focus() != target {
            keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        }
    }

    /// The surface at a global position and the global position of its origin. Only
    /// the part of a window inside its tile can be hit.
    pub fn surface_at(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (x, y) = (location.x.floor() as i32, location.y.floor() as i32);
        let tile = self
            .tiling
            .tiles()
            .find(|t| t.mapped && t.rect.is_some_and(|r| r.contains(x, y)))?;
        let window_pos = window_position(&tile.window, tile.rect?).to_f64();
        let (surface, pos_in_window) = tile
            .window
            .surface_under(location - window_pos, WindowSurfaceType::ALL)?;
        Some((surface, window_pos + pos_in_window.to_f64()))
    }

    /// Mapped windows with the tile they are drawn in.
    pub fn visible_tiles(&self) -> impl Iterator<Item = (&Window, Rect, Point<i32, Logical>)> {
        self.tiling.tiles().filter(|t| t.mapped).filter_map(|t| {
            let rect = t.rect?;
            Some((&t.window, rect, window_position(&t.window, rect)))
        })
    }

    /// Runs a keybind action.
    pub fn run_action(&mut self, action: Action) {
        let start = Instant::now();
        let focused = self.tiling.layout.focused();
        match action {
            Action::SpawnTerminal => self.spawn_terminal(),
            Action::CloseFocused => {
                if let Some(tile) = focused.and_then(|id| self.tiling.by_id(id)) {
                    self.request_close(&tile.window.clone());
                }
            }
            Action::Focus(direction) => {
                if let Some(next) =
                    focused.and_then(|id| self.tiling.layout.neighbour(id, direction))
                {
                    self.tiling.layout.focus(next);
                    self.sync_keyboard_focus();
                }
            }
            Action::Swap(direction) => {
                if let Some(id) = focused
                    && let Some(other) = self.tiling.layout.neighbour(id, direction)
                {
                    // Focus stays on `id`, which now sits in the neighbour's leaf.
                    self.tiling.layout.swap(id, other);
                    self.arrange();
                }
            }
            Action::Quit => self.loop_signal.stop(),
        }
        tracing::debug!(
            ?action,
            layout_us = start.elapsed().as_micros() as u64,
            "keybind"
        );
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
