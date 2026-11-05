// SPDX-License-Identifier: Apache-2.0

//! wlr-layer-shell (M13b step 3): panels, docks, launchers and overlays from
//! first-party clients. The global is privileged: only connections sash-wm created
//! see it (trust.rs).
//!
//! Smithay keeps the protocol objects, the double-buffered state and posts the
//! protocol's errors (invalid layer, size, anchor, keyboard interactivity, a role
//! already set, a wrong ack serial, a buffer before the first configure through
//! `ensure_configured`). sash-wm decides everything else here:
//! - placement and the usable area (`layout::layers`), applied to the windows in one
//!   update (`Sash::set_layer_surfaces`);
//! - mapping: a buffer commit after an acked configure maps a surface, a null buffer
//!   unmaps it (the next commit gets a new configure, as after creation);
//! - limits: beyond `PER_CLIENT`, `PER_OUTPUT` surfaces or a namespace longer than
//!   `NAMESPACE`, the surface is sent `closed` at once and ignored;
//! - keyboard: `none` never takes it; `on_demand` takes it on a click; `exclusive` on
//!   the top or overlay layer takes it while mapped (the top-most layer wins, then the
//!   newest), and window-targeting keybinds do nothing meanwhile; `exclusive` on the
//!   bottom or background layer behaves like `on_demand`;
//! - hit-testing and drawing order (`layout::layers::STACKING`): overlay above
//!   everything but the drag icon and cursor; top above windows but below a
//!   fullscreen window; bottom and background below windows; each layer's popups
//!   directly above it.

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{PopupKind, PopupManager, WindowSurfaceType, utils::under_from_surface_tree},
    reexports::{
        wayland_protocols_wlr::layer_shell::v1::server::{
            zwlr_layer_shell_v1::ZwlrLayerShellV1,
            zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
        },
        wayland_server::{
            Resource,
            backend::ClientId,
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor,
        shell::{
            wlr_layer::{
                Anchor, ExclusiveZone, KeyboardInteractivity, Layer, LayerSurface as Handle,
                LayerSurfaceCachedState, WlrLayerShellGlobalData, WlrLayerShellHandler,
                WlrLayerShellState, WlrLayerSurfaceUserData,
            },
            xdg::PopupSurface,
        },
    },
};

use crate::{
    layout::{
        Edges, Rect,
        layers::{self, LayerId, LayerSurface, Level, Margins},
    },
    state::Sash,
};

/// Layer surfaces one client may have at once.
pub const PER_CLIENT: usize = 16;
/// Layer surfaces on one output at once.
pub const PER_OUTPUT: usize = 32;
/// The longest namespace accepted, in bytes.
pub const NAMESPACE: usize = 128;

/// A layer surface hit by the pointer: its id, the surface under the point (the layer
/// surface, a subsurface or a popup) and that surface's global origin.
type LayerHit = (LayerId, (WlSurface, Point<f64, Logical>));

/// A layer surface sash-wm accepted.
pub struct LayerEntry {
    pub id: LayerId,
    pub handle: Handle,
    pub namespace: String,
    client: Option<ClientId>,
    /// When it was mapped (reservation and stacking order); None while unmapped.
    pub mapped: Option<u64>,
    /// The next commit without a buffer gets a configure (after creation and after an
    /// unmap).
    needs_configure: bool,
    /// Where it is placed (the configured size).
    pub rect: Rect,
}

#[derive(Default)]
pub struct Layers {
    pub entries: Vec<LayerEntry>,
    next_id: u64,
    next_map: u64,
    /// The surface an on_demand click gave the keyboard to.
    pub on_demand: Option<LayerId>,
    /// The mapped surfaces last given to the layout (to skip unchanged commits).
    applied: Vec<LayerSurface>,
}

impl Layers {
    fn by_surface(&self, surface: &WlSurface) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.handle.wl_surface() == surface)
    }

    pub fn by_id(&self, id: LayerId) -> Option<&LayerEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Mapped surfaces of `level`, bottom to top.
    fn stacked(&self, level: Level) -> Vec<&LayerEntry> {
        let pure: Vec<LayerSurface> = self.entries.iter().filter_map(pure_mapped).collect();
        layers::stack(&pure, level)
            .into_iter()
            .filter_map(|id| self.by_id(id))
            .collect()
    }
}

/// The committed protocol state of a layer surface.
fn cached(handle: &Handle) -> LayerSurfaceCachedState {
    compositor::with_states(handle.wl_surface(), |states| {
        *states
            .cached_state
            .get::<LayerSurfaceCachedState>()
            .current()
    })
}

fn level(layer: Layer) -> Level {
    match layer {
        Layer::Background => Level::Background,
        Layer::Bottom => Level::Bottom,
        Layer::Top => Level::Top,
        Layer::Overlay => Level::Overlay,
    }
}

/// The entry as the arranger sees it, mapped at `mapped`.
fn pure(entry: &LayerEntry, mapped: u64) -> LayerSurface {
    let state = cached(&entry.handle);
    LayerSurface {
        id: entry.id,
        level: level(state.layer),
        anchor: Edges {
            left: state.anchor.contains(Anchor::LEFT),
            right: state.anchor.contains(Anchor::RIGHT),
            top: state.anchor.contains(Anchor::TOP),
            bottom: state.anchor.contains(Anchor::BOTTOM),
        },
        margin: Margins {
            top: state.margin.top,
            right: state.margin.right,
            bottom: state.margin.bottom,
            left: state.margin.left,
        },
        size: (state.size.w.max(0) as u32, state.size.h.max(0) as u32),
        exclusive: match state.exclusive_zone {
            ExclusiveZone::Exclusive(zone) => i32::try_from(zone).unwrap_or(i32::MAX),
            ExclusiveZone::Neutral => 0,
            ExclusiveZone::DontCare => -1,
        },
        mapped,
    }
}

fn pure_mapped(entry: &LayerEntry) -> Option<LayerSurface> {
    entry.mapped.map(|serial| pure(entry, serial))
}

fn interactivity(entry: &LayerEntry) -> KeyboardInteractivity {
    cached(&entry.handle).keyboard_interactivity
}

/// Whether a surface holds the keyboard by being exclusive (top or overlay only).
fn holds_exclusively(entry: &LayerEntry) -> bool {
    let state = cached(&entry.handle);
    entry.mapped.is_some()
        && state.keyboard_interactivity == KeyboardInteractivity::Exclusive
        && matches!(state.layer, Layer::Top | Layer::Overlay)
}

/// Where the client's buffer is drawn inside its rectangle: a buffer of another size
/// than configured is centered on an axis anchored to both edges or to neither, and
/// kept against the edge otherwise.
fn draw_origin(entry: &LayerEntry) -> Point<i32, Logical> {
    let r = entry.rect;
    let size = with_renderer_surface_state(entry.handle.wl_surface(), |s| s.surface_size())
        .flatten()
        .map_or((r.w, r.h), |s| (s.w, s.h));
    let anchor = cached(&entry.handle).anchor;
    let axis =
        |start: i32, room: i32, len: i32, at_start: bool, at_end: bool| match (at_start, at_end) {
            (true, false) => start,
            (false, true) => start + room - len,
            _ => start + (room - len) / 2,
        };
    (
        axis(
            r.x,
            r.w,
            size.0,
            anchor.contains(Anchor::LEFT),
            anchor.contains(Anchor::RIGHT),
        ),
        axis(
            r.y,
            r.h,
            size.1,
            anchor.contains(Anchor::TOP),
            anchor.contains(Anchor::BOTTOM),
        ),
    )
        .into()
}

impl WlrLayerShellHandler for Sash {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.protocols.layer_shell
    }

    fn new_layer_surface(
        &mut self,
        surface: Handle,
        output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        // A second layer_surface for a wl_surface that has one.
        if self
            .layers
            .entries
            .iter()
            .any(|e| e.handle.wl_surface() == surface.wl_surface() && e.handle.alive())
        {
            surface.shell_surface().post_error(
                zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                "the wl_surface already has a layer surface",
            );
            return;
        }
        let client = surface.wl_surface().client().map(|c| c.id());
        let of_client = self
            .layers
            .entries
            .iter()
            .filter(|e| e.client == client)
            .count();
        let ours = output
            .as_ref()
            .is_none_or(|o| self.output.as_ref().is_some_and(|ours| ours.owns(o)));
        let refusal = if namespace.len() > NAMESPACE {
            Some("namespace too long")
        } else if of_client >= PER_CLIENT {
            Some("too many layer surfaces for this client")
        } else if self.layers.entries.len() >= PER_OUTPUT {
            Some("too many layer surfaces on the output")
        } else if !ours {
            Some("not on sash-wm's output")
        } else {
            None
        };
        if let Some(reason) = refusal {
            tracing::warn!(
                reason,
                namespace_bytes = namespace.len(),
                "layer surface refused (sent closed)"
            );
            record!(self, "layer_refused", "reason" => reason, "namespace_bytes" => namespace.len());
            surface.send_close();
            return;
        }
        self.layers.next_id += 1;
        let id = LayerId(self.layers.next_id);
        tracing::debug!(id = id.0, %namespace, ?layer, "new layer surface");
        record!(self, "layer_new", "layer" => id.0, "namespace" => namespace.clone(), "level" => format!("{:?}", level(layer)));
        self.layers.entries.push(LayerEntry {
            id,
            handle: surface,
            namespace,
            client,
            mapped: None,
            needs_configure: true,
            rect: Rect::default(),
        });
    }

    fn new_popup(&mut self, _parent: Handle, popup: PopupSurface) {
        // xdg_surface.get_popup came first, without a parent; now it has one.
        let positioner = popup.with_pending_state(|state| state.positioner);
        self.new_popup(popup, positioner);
    }

    fn layer_destroyed(&mut self, surface: Handle) {
        self.forget_layer(surface.wl_surface());
    }
}

impl Sash {
    /// A layer surface, or its wl_surface, was destroyed: unmapped and forgotten; its
    /// reservation is freed and the keyboard goes back by the focus rules.
    pub fn forget_layer(&mut self, surface: &WlSurface) {
        let Some(index) = self.layers.by_surface(surface) else {
            return;
        };
        let entry = self.layers.entries.remove(index);
        tracing::debug!(id = entry.id.0, "layer surface destroyed");
        record!(self, "layer_destroyed", "layer" => entry.id.0, "namespace" => entry.namespace.clone());
        if self.layers.on_demand == Some(entry.id) {
            self.layers.on_demand = None;
        }
        self.update_layers();
        self.sync_keyboard_focus();
        self.request_redraw();
    }
}

// What `delegate_layer_shell!` does, with binds counted (limits.rs).
crate::limits::counted_global!(ZwlrLayerShellV1: WlrLayerShellGlobalData => WlrLayerShellState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [ZwlrLayerShellV1: ()] => WlrLayerShellState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [ZwlrLayerSurfaceV1: WlrLayerSurfaceUserData] => WlrLayerShellState);

impl Sash {
    /// A commit on a layer surface's root. Returns false if `surface` is not one.
    pub fn layer_committed(&mut self, surface: &WlSurface) -> bool {
        let Some(index) = self.layers.by_surface(surface) else {
            // A refused layer surface: never configured, never drawn.
            return compositor::get_role(surface)
                == Some(smithay::wayland::shell::wlr_layer::LAYER_SURFACE_ROLE);
        };
        let has_buffer =
            with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
        let (id, needs_configure, mapped) = {
            let entry = &self.layers.entries[index];
            (entry.id, entry.needs_configure, entry.mapped)
        };
        if needs_configure && !has_buffer {
            // Placed while still waiting, so no other configure goes out first.
            self.update_layers();
            let entry = &mut self.layers.entries[index];
            entry.needs_configure = false;
            let (w, h) = (entry.rect.w, entry.rect.h);
            entry
                .handle
                .with_pending_state(|state| state.size = Some((w, h).into()));
            entry.handle.send_configure();
            record!(self, "layer_configure", "layer" => id.0, "w" => w, "h" => h);
            return true;
        }
        // A buffer before the first ack is the client's protocol error (Smithay).
        if has_buffer && !self.layers.entries[index].handle.ensure_configured() {
            return true;
        }
        match (has_buffer, mapped) {
            (true, None) => {
                self.layers.next_map += 1;
                let entry = &mut self.layers.entries[index];
                entry.mapped = Some(self.layers.next_map);
                if let Some(output) = &self.output {
                    output.enter(surface);
                }
                record!(
                    self,
                    "layer_map",
                    "layer" => id.0,
                    "namespace" => entry.namespace.clone(),
                    "level" => format!("{:?}", level(cached(&entry.handle).layer)),
                    "w" => entry.rect.w,
                    "h" => entry.rect.h,
                    "zone" => pure(entry, 0).exclusive,
                );
                tracing::debug!(id = id.0, namespace = %entry.namespace, "layer surface mapped");
                self.update_layers();
                self.sync_keyboard_focus();
            }
            (false, Some(_)) => {
                let entry = &mut self.layers.entries[index];
                entry.mapped = None;
                entry.needs_configure = true;
                if let Some(output) = &self.output {
                    output.leave(surface);
                }
                record!(self, "layer_unmap", "layer" => id.0);
                tracing::debug!(id = id.0, "layer surface unmapped");
                if self.layers.on_demand == Some(id) {
                    self.layers.on_demand = None;
                }
                self.update_layers();
                self.sync_keyboard_focus();
            }
            // A state change (size, anchors, zone, layer, keyboard mode) applies now.
            _ => {
                self.update_layers();
                self.sync_keyboard_focus();
            }
        }
        self.request_redraw();
        true
    }

    /// Places every layer surface again, gives the usable area to the layout when the
    /// mapped surfaces changed, and configures the surfaces whose size changed.
    pub fn update_layers(&mut self) {
        self.layers.entries.retain(|e| e.handle.alive());
        let output = self.windows.layout.output();
        let mapped: Vec<LayerSurface> =
            self.layers.entries.iter().filter_map(pure_mapped).collect();
        if mapped != self.layers.applied {
            self.layers.applied = mapped.clone();
            self.set_layer_surfaces(mapped.clone());
        }
        // Unmapped surfaces waiting for a configure are placed as if mapped after all
        // others: their own reservation does not move anything until they map.
        let mut all = mapped;
        let mut serial = self.layers.next_map;
        for entry in self.layers.entries.iter().filter(|e| e.mapped.is_none()) {
            serial += 1;
            all.push(pure(entry, serial));
        }
        let arrangement = layers::arrange(output, &all);
        for entry in &mut self.layers.entries {
            let Some(rect) = arrangement.rect_of(entry.id) else {
                continue;
            };
            let resized = (rect.w, rect.h) != (entry.rect.w, entry.rect.h);
            entry.rect = rect;
            if resized && !entry.needs_configure {
                entry
                    .handle
                    .with_pending_state(|state| state.size = Some((rect.w, rect.h).into()));
                entry.handle.send_pending_configure();
                record!(self, "layer_configure", "layer" => entry.id.0, "w" => rect.w, "h" => rect.h);
            }
        }
    }

    /// The mapped layer surface holding the keyboard exclusively: top-most layer
    /// first, then the newest.
    pub fn exclusive_layer(&self) -> Option<LayerId> {
        self.layers
            .entries
            .iter()
            .filter(|e| holds_exclusively(e))
            .max_by_key(|e| (level(cached(&e.handle).layer), e.mapped))
            .map(|e| e.id)
    }

    /// The layer surface that should have the keyboard, if any: the exclusive one,
    /// else the one an on_demand click chose.
    pub fn layer_keyboard_target(&self) -> Option<WlSurface> {
        let id = self.exclusive_layer().or(self.layers.on_demand)?;
        let entry = self.layers.by_id(id)?;
        (entry.mapped.is_some() && interactivity(entry) != KeyboardInteractivity::None)
            .then(|| entry.handle.wl_surface().clone())
    }

    /// A press on a layer surface: on_demand (and exclusive on bottom or background)
    /// surfaces take the keyboard.
    pub fn layer_clicked(&mut self, id: LayerId) {
        let Some(entry) = self.layers.by_id(id) else {
            return;
        };
        if interactivity(entry) != KeyboardInteractivity::None && !holds_exclusively(entry) {
            let _namespace = entry.namespace.clone();
            self.layers.on_demand = Some(id);
            record!(self, "layer_focus", "layer" => id.0, "namespace" => _namespace);
            self.sync_keyboard_focus();
        }
    }

    /// A window was focused (click or keybind): an on_demand surface loses the
    /// keyboard.
    pub fn layer_focus_lost(&mut self) {
        if self.layers.on_demand.take().is_some() {
            self.sync_keyboard_focus();
        }
    }

    /// The topmost layer surface (or its popup) at `location` on `levels`, tried in
    /// that order, and the surface hit with its global origin.
    fn layer_hit(&self, location: Point<f64, Logical>, levels: &[Level]) -> Option<LayerHit> {
        for level in levels {
            let stacked = self.layers.stacked(*level);
            // Popups of a layer are above all its surfaces.
            for entry in stacked.iter().rev() {
                for (popup, origin) in layer_popups(entry) {
                    if let Some((surface, at)) = under_from_surface_tree(
                        popup.wl_surface(),
                        location,
                        origin,
                        WindowSurfaceType::ALL,
                    ) {
                        return Some((entry.id, (surface, at.to_f64())));
                    }
                }
            }
            for entry in stacked.iter().rev() {
                if let Some((surface, at)) = under_from_surface_tree(
                    entry.handle.wl_surface(),
                    location,
                    draw_origin(entry),
                    WindowSurfaceType::ALL,
                ) {
                    return Some((entry.id, (surface, at.to_f64())));
                }
            }
        }
        None
    }

    /// A layer surface above the windows at `location`: overlay, and top unless a
    /// fullscreen window covers it.
    pub fn layer_above(&self, location: Point<f64, Logical>) -> Option<LayerHit> {
        if self.windows.layout.fullscreen().is_some() {
            self.layer_hit(location, &[Level::Overlay])
        } else {
            self.layer_hit(location, &[Level::Overlay, Level::Top])
        }
    }

    /// A layer surface below the windows at `location` (none under a fullscreen
    /// window, which covers the output).
    pub fn layer_below(&self, location: Point<f64, Logical>) -> Option<LayerHit> {
        if self.windows.layout.fullscreen().is_some() {
            return None;
        }
        self.layer_hit(location, &[Level::Bottom, Level::Background])
    }

    /// Mapped surfaces of `level` to draw, top first, each with its popups first:
    /// (popups with their origins, the surface, its origin).
    pub fn layers_to_draw(&self, level: Level) -> Vec<(WlSurface, Point<i32, Logical>)> {
        let stacked = self.layers.stacked(level);
        let mut out: Vec<(WlSurface, Point<i32, Logical>)> = Vec::new();
        for entry in stacked.iter().rev() {
            out.extend(
                layer_popups(entry)
                    .into_iter()
                    .map(|(p, o)| (p.wl_surface().clone(), o)),
            );
        }
        for entry in stacked.iter().rev() {
            out.push((entry.handle.wl_surface().clone(), draw_origin(entry)));
        }
        out
    }

    /// Global origin of a layer surface's buffer, if `root` is a mapped one (popup
    /// placement).
    pub fn layer_origin(&self, root: &WlSurface) -> Option<Point<i32, Logical>> {
        let entry = &self.layers.entries[self.layers.by_surface(root)?];
        entry.mapped.map(|_| draw_origin(entry))
    }

    /// Every mapped layer surface and its popups (frame callbacks).
    pub fn layer_surfaces_shown(&self) -> Vec<WlSurface> {
        let mut out = Vec::new();
        for entry in self.layers.entries.iter().filter(|e| e.mapped.is_some()) {
            out.push(entry.handle.wl_surface().clone());
            out.extend(
                layer_popups(entry)
                    .into_iter()
                    .map(|(p, _)| p.wl_surface().clone()),
            );
        }
        out
    }

    /// The root surfaces of mapped layer surfaces (popup re-constraining).
    pub fn layer_roots(&self) -> Vec<WlSurface> {
        self.layers
            .entries
            .iter()
            .filter(|e| e.mapped.is_some())
            .map(|e| e.handle.wl_surface().clone())
            .collect()
    }
}

/// A layer surface's popups with the global origin of each popup's buffer, topmost
/// first.
fn layer_popups(entry: &LayerEntry) -> Vec<(PopupKind, Point<i32, Logical>)> {
    let origin = draw_origin(entry);
    PopupManager::popups_for_surface(entry.handle.wl_surface())
        .map(|(popup, offset)| (popup.clone(), origin + offset - popup.geometry().loc))
        .collect()
}
