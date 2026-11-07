// SPDX-License-Identifier: Apache-2.0

//! sash-ui: the first-party UI toolkit (M17).
//!
//! CPU drawing with tiny-skia into wl_shm, limited to damage; an idle surface draws
//! and commits nothing (owner, after M16). Wayland goes through smithay-client-toolkit
//! behind this crate's own types; text comes from sash-text. sash-ui never depends on
//! sash-wm.
//!
//! E1 (this step): rectangles and text in a layer-shell panel ([`panel::Panel`]).
//! Widgets, layout, focus and accessibility come with E2.

pub mod canvas;
pub mod damage;
pub mod geometry;
pub mod panel;

pub use canvas::{Canvas, Color};
pub use damage::Damage;
pub use geometry::Rect;
pub use panel::{Edges, Layer, Panel, PanelSpec, Stats, View};
