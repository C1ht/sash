// SPDX-License-Identifier: Apache-2.0

//! Keyboard focus (M16 B3, owner after M16: full keyboard navigation with a defined
//! focus order).
//!
//! - The focus order is the document order of the widget tree (parents before
//!   children, children in the order they were added), not screen position (owner,
//!   after E2): a screen is built in the order it should be read.
//! - Tab and Shift+Tab move between tab stops and wrap around the surface. A
//!   focusable widget outside any group is a stop; a group (a row or column marked
//!   as one) is one stop, entered at the item focused in it last, else its first.
//! - Arrows move inside the nearest group along its axis, Home and End go to its first
//!   and last item; no wrap.
//! - Focus is never lost: when the focused widget goes away or is disabled, the next
//!   focusable widget in reading order takes it, else the previous one.

use std::collections::HashMap;

use crate::{
    input::Key,
    tree::{Axis, Kind, NodeId, Tree},
};

/// Focusable widgets in reading order.
pub(crate) fn focusables(tree: &Tree) -> Vec<NodeId> {
    tree.order()
        .into_iter()
        .filter(|&id| tree.is_focusable(id))
        .collect()
}

/// The tab stops in order, each as (the group or widget, the widget it focuses).
pub(crate) fn stops(tree: &Tree, roving: &HashMap<NodeId, NodeId>) -> Vec<(NodeId, NodeId)> {
    let mut out: Vec<(NodeId, NodeId)> = Vec::new();
    for id in focusables(tree) {
        let stop = outermost_group(tree, id).unwrap_or(id);
        if out.iter().any(|&(s, _)| s == stop) {
            continue;
        }
        let target = roving
            .get(&stop)
            .copied()
            .filter(|&r| tree.is_focusable(r) && outermost_group(tree, r) == Some(stop))
            .unwrap_or(id);
        out.push((stop, target));
    }
    out
}

pub(crate) fn outermost_group(tree: &Tree, id: NodeId) -> Option<NodeId> {
    let mut found = None;
    let mut next = tree.group_of(id);
    while let Some(g) = next {
        found = Some(g);
        next = tree.group_of(g);
    }
    found
}

/// Where Tab (or Shift+Tab with `back`) moves focus from `current`.
pub(crate) fn tab(
    tree: &Tree,
    current: Option<NodeId>,
    back: bool,
    roving: &HashMap<NodeId, NodeId>,
) -> Option<NodeId> {
    let stops = stops(tree, roving);
    if stops.is_empty() {
        return None;
    }
    let here = current.and_then(|c| {
        let stop = outermost_group(tree, c).unwrap_or(c);
        stops.iter().position(|&(s, _)| s == stop)
    });
    let n = stops.len();
    let next = match (here, back) {
        (None, false) => 0,
        (None, true) => n - 1,
        (Some(i), false) => (i + 1) % n,
        (Some(i), true) => (i + n - 1) % n,
    };
    Some(stops[next].1)
}

/// Where an arrow, Home or End moves focus inside `current`'s group; None if the key
/// does nothing there.
pub(crate) fn arrow(tree: &Tree, current: NodeId, key: Key) -> Option<NodeId> {
    let group = tree.group_of(current)?;
    let Kind::Container(c) = &tree.get(group)?.kind else {
        return None;
    };
    let items: Vec<NodeId> = focusables(tree)
        .into_iter()
        .filter(|&id| tree.group_of(id) == Some(group))
        .collect();
    let i = items.iter().position(|&id| id == current)?;
    let (back, forward) = match c.axis {
        Some(Axis::Horizontal) => (Key::Left, Key::Right),
        Some(Axis::Vertical) => (Key::Up, Key::Down),
        None => return None,
    };
    let to = match key {
        Key::Home => 0,
        Key::End => items.len() - 1,
        k if k == back => i.checked_sub(1)?,
        k if k == forward => i + 1,
        _ => return None,
    };
    items.get(to).copied().filter(|&id| id != current)
}

/// Who gets focus when `lost` stops being focusable; `before` is the focusable list
/// from before the change.
pub(crate) fn successor(tree: &Tree, before: &[NodeId], lost: NodeId) -> Option<NodeId> {
    let i = before.iter().position(|&id| id == lost)?;
    before[i + 1..]
        .iter()
        .chain(before[..i].iter().rev())
        .copied()
        .find(|&id| tree.is_focusable(id))
}
