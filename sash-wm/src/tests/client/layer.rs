// SPDX-License-Identifier: Apache-2.0

//! The test client's layer-shell side (M13b step 3, tests only): layer surfaces with
//! a configurable layer, anchors, size, margins, zone and keyboard mode, the configures
//! and `closed` events they get. tools/check-release.sh checks that `MARKER` is in the
//! test binary and in no release binary.

use std::collections::HashMap;

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    backend::ObjectId,
    delegate_noop,
    protocol::{wl_output::WlOutput, wl_region::WlRegion, wl_surface::WlSurface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use super::{Events, PopupSpec, TestClient, TestPopup};

/// The string tools/check-release.sh looks for.
pub const MARKER: &str = "layer-shell test client (tests only)";

/// What a layer surface asks for.
#[derive(Clone, Debug)]
pub struct LayerSpec {
    pub layer: Layer,
    pub namespace: String,
    pub anchor: Anchor,
    pub size: (u32, u32),
    pub margin: (i32, i32, i32, i32),
    pub zone: i32,
    pub keyboard: KeyboardInteractivity,
}

impl LayerSpec {
    /// A panel along the top edge: `height` px high, reserving `height + margin_top`
    /// (the first-party panel rule: zone = visible size plus margin).
    pub fn top_panel(height: u32, margin_top: i32) -> Self {
        Self {
            layer: Layer::Top,
            namespace: "panel".into(),
            anchor: Anchor::Top | Anchor::Left | Anchor::Right,
            size: (0, height),
            margin: (margin_top, 0, 0, 0),
            zone: height as i32 + margin_top,
            keyboard: KeyboardInteractivity::None,
        }
    }

    /// A centered surface on `layer` with keyboard mode `keyboard`.
    pub fn centered(layer: Layer, size: (u32, u32), keyboard: KeyboardInteractivity) -> Self {
        Self {
            layer,
            namespace: "launcher".into(),
            anchor: Anchor::empty(),
            size,
            margin: (0, 0, 0, 0),
            zone: 0,
            keyboard,
        }
    }
}

/// What a layer surface got from the compositor.
#[derive(Default, Debug)]
pub struct LayerInfo {
    /// (serial, width, height) of every configure.
    pub configures: Vec<(u32, u32, u32)>,
    pub closed: bool,
}

#[derive(Default)]
pub struct LayerEvents {
    pub shell: Option<ZwlrLayerShellV1>,
    pub surfaces: HashMap<ObjectId, LayerInfo>,
}

pub struct TestLayer {
    pub surface: WlSurface,
    pub layer: ZwlrLayerSurfaceV1,
}

impl TestClient {
    pub fn layer_shell(&self) -> Option<ZwlrLayerShellV1> {
        self.events.layers.shell.clone()
    }

    pub fn layer_info(&self, layer: &TestLayer) -> &LayerInfo {
        self.events
            .layers
            .surfaces
            .get(&layer.layer.id())
            .expect("known layer surface")
    }

    /// Binds the output (wl_output) for an explicit output assignment.
    pub fn bind_output(&mut self) -> WlOutput {
        let qh = self.qh();
        let name = self
            .events
            .globals
            .iter()
            .find(|(_, i)| i == "wl_output")
            .map(|(n, _)| *n)
            .expect("wl_output");
        let registry = self.events.registry.as_ref().expect("registry");
        let output = registry.bind(name, 1, &qh, ());
        self.roundtrip();
        output
    }

    /// A popup of a layer surface: an xdg_popup without a parent, given to the layer
    /// surface with get_popup. Waits for its configure, acks and maps it.
    pub fn layer_popup(&mut self, layer: &TestLayer, spec: PopupSpec) -> TestPopup {
        let qh = self.qh();
        let positioner = self.positioner(spec);
        let surface = self
            .events
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, ());
        let wm_base = self.events.wm_base.as_ref().expect("xdg_wm_base");
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let popup = xdg_surface.get_popup(None, &positioner, &qh, ());
        positioner.destroy();
        layer.layer.get_popup(&popup);
        surface.commit();
        let popup = TestPopup {
            surface,
            xdg_surface,
            popup,
        };
        let id = popup.xdg_surface.id();
        self.dispatch_until("layer popup configure", |e| e.serials.contains_key(&id));
        self.ack_and_commit_popup(&popup);
        popup
    }

    /// A layer surface with `spec`, committed once (no buffer): the compositor answers
    /// with its first configure.
    pub fn create_layer(&mut self, spec: &LayerSpec) -> TestLayer {
        self.create_layer_on(spec, None)
    }

    /// `create_layer` on an explicit output (None: the compositor chooses).
    pub fn create_layer_on(&mut self, spec: &LayerSpec, output: Option<&WlOutput>) -> TestLayer {
        let qh = self.qh();
        let shell = self
            .layer_shell()
            .expect("zwlr_layer_shell_v1 (privileged clients only)");
        let surface = self
            .events
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, ());
        let layer = shell.get_layer_surface(
            &surface,
            output,
            spec.layer,
            spec.namespace.clone(),
            &qh,
            (),
        );
        self.events
            .layers
            .surfaces
            .insert(layer.id(), LayerInfo::default());
        layer.set_size(spec.size.0, spec.size.1);
        layer.set_anchor(spec.anchor);
        let (t, r, b, l) = spec.margin;
        layer.set_margin(t, r, b, l);
        layer.set_exclusive_zone(spec.zone);
        layer.set_keyboard_interactivity(spec.keyboard);
        surface.commit();
        TestLayer { surface, layer }
    }

    /// Creates, waits for the configure, acks it and commits a buffer of that size.
    pub fn map_layer(&mut self, spec: &LayerSpec) -> TestLayer {
        let layer = self.create_layer(spec);
        let id = layer.layer.id();
        self.dispatch_until("the layer surface's configure", |e| {
            e.layers
                .surfaces
                .get(&id)
                .is_some_and(|i| !i.configures.is_empty() || i.closed)
        });
        self.ack_and_commit_layer(&layer);
        layer
    }

    /// Acks the latest configure and commits a buffer of its size (at least 1x1).
    pub fn ack_and_commit_layer(&mut self, layer: &TestLayer) {
        let (serial, w, h) = *self
            .layer_info(layer)
            .configures
            .last()
            .expect("configured");
        layer.layer.ack_configure(serial);
        let size = ((w as i32).max(1), (h as i32).max(1));
        let buffer = self.buffer(size, 0xff20_4060);
        layer.surface.attach(Some(&buffer), 0, 0);
        layer.surface.damage_buffer(0, 0, size.0, size.1);
        layer.surface.commit();
        self.roundtrip();
    }

    /// Unmaps a layer surface (a null buffer).
    pub fn unmap_layer(&mut self, layer: &TestLayer) {
        layer.surface.attach(None, 0, 0);
        layer.surface.commit();
        self.roundtrip();
    }

    /// Changes a mapped layer surface's state and commits it with its buffer.
    pub fn change_layer(&mut self, layer: &TestLayer, change: impl FnOnce(&ZwlrLayerSurfaceV1)) {
        change(&layer.layer);
        layer.surface.commit();
        self.roundtrip();
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for Events {
    fn event(
        _: &mut Self,
        _: &ZwlrLayerShellV1,
        _: <ZwlrLayerShellV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for Events {
    fn event(
        events: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let info = events.layers.surfaces.entry(layer.id()).or_default();
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => info.configures.push((serial, width, height)),
            zwlr_layer_surface_v1::Event::Closed => info.closed = true,
            _ => {}
        }
    }
}

delegate_noop!(Events: ignore WlOutput);
delegate_noop!(Events: ignore WlRegion);
