// SPDX-License-Identifier: Apache-2.0

//! The usable area end to end (M13b step 2): layer surfaces are given to the compositor
//! directly (`Sash::set_layer_surfaces`; the protocol comes in step 3) on the headless
//! harness (output 800x600, modifier Alt).

use smithay::backend::input::ButtonState;

use super::{
    client::TestClient,
    decorations::{FLOAT, decorated, floating, size},
    harness::Harness,
};
use crate::{
    layout::{
        Edges, Rect, WindowId,
        layers::{LayerId, LayerSurface, Level, Margins},
    },
    seat::BTN_LEFT,
};

/// A panel along the top edge reserving `height` px.
fn top_panel(id: u64, height: u32) -> LayerSurface {
    LayerSurface {
        id: LayerId(id),
        level: Level::Top,
        anchor: Edges {
            left: true,
            right: true,
            top: true,
            bottom: false,
        },
        margin: Margins::default(),
        size: (0, height),
        exclusive: height as i32,
        mapped: id,
    }
}

#[test]
fn snap_zones_use_the_usable_area_and_trigger_at_the_true_output_edge() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        server.on_server(|h| h.server.state.set_layer_surfaces(vec![top_panel(1, 40)]));
        c.roundtrip();
        c.ack_and_commit(&w);
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(FLOAT), "below the panel: not moved");
            h.pointer_to(300.0, 120.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            let preview = |h: &mut Harness, x: f64, y: f64| {
                h.pointer_to(x, y);
                h.server.state.overlay.preview()
            };
            let usable = Rect::new(0, 40, 800, 560);
            let left = Some(Rect::new(0, 40, 400, 560));
            assert_eq!(
                preview(h, 400.0, 0.0),
                Some(usable),
                "y = 0, over the panel"
            );
            assert_eq!(preview(h, 400.0, 7.0), Some(usable), "y = 7");
            assert_eq!(preview(h, 400.0, 8.0), None, "y = 8: still over the panel");
            assert_eq!(preview(h, 400.0, 41.0), None, "just below the panel");
            assert_eq!(
                preview(h, 0.0, 30.0),
                Some(Rect::new(0, 40, 400, 280)),
                "corner"
            );
            assert_eq!(preview(h, 0.0, 300.0), left);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), left);
            let layout = &h.server.state.windows.layout;
            let border = layout.decoration(WindowId(1)).border;
            assert!(border.top && border.right && !border.left && !border.bottom);
            let zones = layout.resize_zones(WindowId(1));
            assert!(zones.top && zones.right && !zones.left && !zones.bottom);
        });
        c.roundtrip();
        c.ack_and_commit(&w);
        // 400 x 560 minus the bar (32), the right and the top border.
        assert_eq!(size(&c, &w), (399, 527));
    });
}

#[test]
fn maximize_fills_the_usable_area_and_fullscreen_the_output() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| h.server.state.set_layer_surfaces(vec![top_panel(1, 50)]));
        c.roundtrip();
        // The tile keeps its 1 px border on the panel edge.
        assert_eq!(c.config(&w).expect("configured").size, (800, 549));
        w.toplevel.set_maximized();
        c.roundtrip();
        let maximized = c.config(&w).expect("configured");
        assert!(maximized.maximized);
        assert_eq!(
            maximized.size,
            (800, 549),
            "the usable area, border on the panel edge"
        );
        w.toplevel.set_fullscreen(None);
        c.roundtrip();
        let fullscreen = c.config(&w).expect("configured");
        assert!(fullscreen.fullscreen);
        assert_eq!(fullscreen.size, (800, 600));
    });
}

/// An output resize and a panel change in the same frame give one layout update: each
/// window gets one configure, at its final size.
#[test]
fn a_resize_and_a_panel_change_together_send_one_configure_each() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut a = TestClient::connect(&socket);
        let wa = a.map_window();
        server.pointer_over_focused();
        let mut b = TestClient::connect(&socket);
        let wb = b.map_window();
        a.roundtrip();
        let before = (a.events.configures, b.events.configures);
        server.on_server(|h| {
            h.server
                .state
                .set_output_and_layers((1000, 700).into(), vec![top_panel(1, 60)]);
        });
        a.roundtrip();
        b.roundtrip();
        assert_eq!(a.events.configures, before.0 + 1);
        assert_eq!(b.events.configures, before.1 + 1);
        // Side by side under the panel; each keeps the border on its inner edge and
        // on the panel edge (1 px each).
        assert_eq!(a.config(&wa).expect("configured").size, (499, 639));
        assert_eq!(b.config(&wb).expect("configured").size, (499, 639));
    });
}

/// A snapped window keeps its resize zone against a panel; a resize from it unsnaps
/// the window and continues from the snapped rectangle (owner, after M13b step 2).
#[test]
fn a_resize_from_the_panel_edge_unsnaps_and_keeps_the_rectangle() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        server.on_server(|h| {
            h.server.state.set_layer_surfaces(vec![top_panel(1, 40)]);
            let layout = &mut h.server.state.windows.layout;
            assert!(layout.snap(WindowId(1), crate::layout::ZoneId::LEFT));
            h.server.state.arrange();
        });
        c.roundtrip();
        c.ack_and_commit(&w);
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 40, 400, 560)));
            // The top zone, inside the 1 px border on the panel edge.
            h.pointer_to(200.0, 43.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(200.0, 103.0);
            h.button(BTN_LEFT, ButtonState::Released);
            let layout = &h.server.state.windows.layout;
            assert_eq!(layout.snapped_zone(WindowId(1)), None, "unsnapped");
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 100, 400, 500)));
        });
    });
}
