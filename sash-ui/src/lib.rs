// SPDX-License-Identifier: Apache-2.0

//! sash-ui: the first-party UI toolkit (M17).
//!
//! CPU drawing with tiny-skia into wl_shm, limited to damage; an idle surface draws
//! and commits nothing (owner, after M16). Wayland goes through smithay-client-toolkit
//! behind this crate's own types; text comes from sash-text. sash-ui never depends on
//! sash-wm.
//!
//! E1: rectangles and text in a layer-shell panel ([`panel::Panel`]).
//! E2: widgets (label, icon, button) in a tree with layout, semantics and keyboard
//! focus ([`ui::Ui`]), theme tokens with contrast rules, and animation that never
//! delays logical state.

pub mod anim;
pub mod canvas;
pub mod damage;
mod focus;
pub mod geometry;
pub mod icon;
pub mod input;
mod layout;
pub mod panel;
pub mod theme;
pub mod tree;
pub mod ui;

pub use canvas::{Canvas, Color};
pub use damage::Damage;
pub use geometry::Rect;
pub use icon::{IconRaster, IconSource};
pub use input::{Input, Key, Pointer};
pub use panel::{Edges, Keyboard, Layer, Panel, PanelSpec, Stats, View};
pub use theme::{Surface, TextLevel, Theme};
pub use tree::{Align, Axis, Insets, Node, NodeId, Role, Semantics, Size, Text};
pub use ui::{Ui, UiEvent};
