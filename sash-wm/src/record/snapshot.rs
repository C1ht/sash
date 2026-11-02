// SPDX-License-Identifier: Apache-2.0

//! What the recorder writes about the layout: a compact snapshot for every layout
//! operation (before and after), and the full state.json of a bundle. Compositor-level
//! data only: window titles are left out (they can carry what was typed).

use smithay::{
    utils::Logical,
    wayland::{compositor, shell::xdg::XdgToplevelSurfaceData},
};

use super::Json;
use crate::{
    layout::{OutlineNode, WindowId},
    state::Sash,
    windows::DragKind,
};

/// The BSP tree in preorder, with split directions and exact ratios.
fn tree(state: &Sash) -> Json {
    let nodes = state.windows.layout.tree().outline();
    Json::Arr(
        nodes
            .into_iter()
            .map(|node| match node {
                OutlineNode::Split {
                    depth,
                    side_by_side,
                    ratio,
                } => Json::obj([
                    ("depth", depth.into()),
                    (
                        "split",
                        if side_by_side {
                            "left|right"
                        } else {
                            "top/bottom"
                        }
                        .into(),
                    ),
                    ("ratio", format!("{}/{}", ratio.num(), ratio.den()).into()),
                ]),
                OutlineNode::Leaf {
                    depth,
                    window,
                    mode,
                } => Json::obj([
                    ("depth", depth.into()),
                    ("window", window.into()),
                    ("leaf", format!("{mode:?}").to_lowercase().into()),
                ]),
            })
            .collect(),
    )
}

/// Every window's mode and rectangles.
fn windows(state: &Sash) -> Json {
    let layout = &state.windows.layout;
    Json::Arr(
        state
            .windows
            .all()
            .map(|managed| {
                let id = managed.id;
                Json::obj([
                    ("id", id.into()),
                    ("mapped", managed.mapped.into()),
                    (
                        "mode",
                        layout
                            .mode(id)
                            .map(|m| format!("{m:?}").to_lowercase())
                            .into(),
                    ),
                    ("forced_floating", layout.is_forced(id).into()),
                    ("rect", layout.rect_of(id).into()),
                    ("shown", layout.shown_rect(id).into()),
                    (
                        "snapped",
                        layout.snapped_zone(id).map(|z| format!("{z:?}")).into(),
                    ),
                    ("maximized", layout.is_maximized(id).into()),
                    ("fullscreen", (layout.fullscreen() == Some(id)).into()),
                ])
            })
            .collect(),
    )
}

/// The snapshot recorded before and after each layout operation.
pub fn layout_json(state: &Sash) -> Json {
    let layout = &state.windows.layout;
    let area = layout.area();
    Json::obj([
        ("output", vec![area.w.into(), area.h.into()].into()),
        ("focus", layout.focused().into()),
        ("windows", windows(state)),
        ("tree", tree(state)),
    ])
}

fn app_id(state: &Sash, id: WindowId) -> Json {
    let Some(toplevel) = state
        .windows
        .by_id(id)
        .and_then(|m| m.window.toplevel().cloned())
    else {
        return Json::Null;
    };
    compositor::with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok().and_then(|d| d.app_id.clone()))
    })
    .into()
}

/// A bundle's state.json: the layout tree, every window, z-order, focus, the window
/// under the pointer, the active grab, the output mode.
pub fn state_json(state: &Sash) -> Json {
    let layout = &state.windows.layout;
    let pointer = state
        .seat
        .get_pointer()
        .map(|p| p.current_location())
        .unwrap_or_default();
    let under = state.window_at(pointer);
    let z: Vec<Json> = layout
        .stacking()
        .into_iter()
        .map(|(id, rect, layer)| {
            Json::obj([
                ("window", id.into()),
                ("rect", rect.into()),
                ("layer", format!("{layer:?}").into()),
            ])
        })
        .collect();
    let drag = state.windows.drag.as_ref().map(|d| {
        Json::obj([
            ("window", d.window.into()),
            (
                "kind",
                match d.kind {
                    DragKind::Move => "move".to_string(),
                    DragKind::Resize(edges) => format!("resize {edges:?}"),
                }
                .into(),
            ),
        ])
    });
    let popup_grab = state
        .popups
        .grab_root()
        .and_then(|root| state.windows.by_surface(&root).map(|m| m.id));
    let output = state
        .output
        .as_ref()
        .and_then(|o| o.current_mode())
        .map(|m| {
            Json::obj([
                ("width", m.size.w.into()),
                ("height", m.size.h.into()),
                ("refresh_mhz", m.refresh.into()),
            ])
        });
    let app_ids: Vec<Json> = state
        .windows
        .all()
        .map(|m| Json::obj([("id", m.id.into()), ("app_id", app_id(state, m.id))]))
        .collect();
    Json::obj([
        ("modifier", format!("{:?}", state.keybinds.modifier).into()),
        ("output_mode", output.into()),
        ("layout_area", layout.area().into()),
        (
            "default_mode",
            format!("{:?}", layout.default_mode()).to_lowercase().into(),
        ),
        ("focus", layout.focused().into()),
        (
            "focus_history",
            layout
                .recent_first()
                .into_iter()
                .map(Json::from)
                .collect::<Vec<_>>()
                .into(),
        ),
        ("pointer", pointer_json(pointer)),
        ("window_under_pointer", under.into()),
        ("drag", drag.into()),
        ("popup_grab_window", popup_grab.into()),
        ("z_order_bottom_to_top", z.into()),
        ("windows", windows(state)),
        ("app_ids", app_ids.into()),
        ("tree", tree(state)),
    ])
}

fn pointer_json(p: smithay::utils::Point<f64, Logical>) -> Json {
    Json::obj([("x", p.x.into()), ("y", p.y.into())])
}
