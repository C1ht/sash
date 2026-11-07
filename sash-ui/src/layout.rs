// SPDX-License-Identifier: Apache-2.0

//! Layout in integer logical pixels: row, column and stack.
//!
//! Content sizes are computed bottom-up once, then rectangles top-down. On a row's or
//! column's main axis, fixed and content-sized children take their size and flex
//! children share what is left by weight: each gets the floor of its share and the
//! remaining pixels go one at a time to the first flex children, so the children and
//! the spacing cover the inner space exactly whenever one child is flexible and the
//! rest fits. Every child rectangle is cut to its parent's inner rectangle, so nothing
//! lies outside its parent even when the content does not fit.

use std::collections::HashMap;

use crate::{
    geometry::Rect,
    tree::{Align, Axis, BUTTON_PADDING, ICON_GAP, Insets, Kind, Node, NodeId, Size, Tree},
};

/// Measures a label or button text: (width, height), logical pixels, rounded up.
pub(crate) type Measure<'a> = dyn FnMut(NodeId, &Node) -> (i32, i32) + 'a;

/// Lays out the tree into `area`.
pub(crate) fn layout(tree: &mut Tree, area: Rect, measure: &mut Measure) {
    let mut content = HashMap::new();
    let root = tree.root();
    content_size(tree, root, measure, &mut content);
    place(tree, root, area, &content);
}

fn content_size(
    tree: &Tree,
    id: NodeId,
    measure: &mut Measure,
    out: &mut HashMap<NodeId, (i32, i32)>,
) -> (i32, i32) {
    let Some(node) = tree.get(id) else {
        return (0, 0);
    };
    let size = match &node.kind {
        Kind::Label(_) => measure(id, node),
        Kind::Icon(i) => (i.size.max(0), i.size.max(0)),
        Kind::Button(b) => {
            let icon = b.icon.as_ref().map_or(0, |i| i.size.max(0));
            if b.label.is_none() {
                (icon + 2 * BUTTON_PADDING, icon)
            } else {
                let (w, h) = measure(id, node);
                let gap = if icon > 0 { icon + ICON_GAP } else { 0 };
                (2 * BUTTON_PADDING + gap + w, h.max(icon))
            }
        }
        Kind::Container(c) => {
            let mut main = 0;
            let mut cross = 0;
            let children = node.children.clone();
            for (i, &child) in children.iter().enumerate() {
                let (cw, ch) = content_size(tree, child, measure, out);
                let Some(cn) = tree.get(child) else { continue };
                let w = fixed_or(cn.width, cw);
                let h = fixed_or(cn.height, ch);
                match c.axis {
                    Some(Axis::Horizontal) => {
                        main += w + if i > 0 { c.spacing } else { 0 };
                        cross = cross.max(h);
                    }
                    Some(Axis::Vertical) => {
                        main += h + if i > 0 { c.spacing } else { 0 };
                        cross = cross.max(w);
                    }
                    None => {
                        main = main.max(w);
                        cross = cross.max(h);
                    }
                }
            }
            let p = c.padding;
            match c.axis {
                Some(Axis::Vertical) => (cross + p.left + p.right, main + p.top + p.bottom),
                _ => (main + p.left + p.right, cross + p.top + p.bottom),
            }
        }
    };
    let size = (size.0.max(0), size.1.max(0));
    out.insert(id, size);
    size
}

/// A size for measuring: fixed, else the content (flex counts as its content).
fn fixed_or(size: Size, content: i32) -> i32 {
    match size {
        Size::Fixed(v) => v.max(0),
        Size::Content | Size::Flex(_) => content,
    }
}

fn inner(rect: Rect, p: Insets) -> Rect {
    Rect::new(
        rect.x + p.left,
        rect.y + p.top,
        (rect.w - p.left - p.right).max(0),
        (rect.h - p.top - p.bottom).max(0),
    )
}

/// Places a child of `size` on a cross axis of length `space` starting at `start`.
fn cross(size: Size, content: i32, align: Align, start: i32, space: i32) -> (i32, i32) {
    let len = match size {
        Size::Fixed(v) => v.max(0),
        Size::Flex(_) => space,
        Size::Content if align == Align::Stretch => space,
        Size::Content => content,
    };
    let offset = match align {
        Align::Start | Align::Stretch => 0,
        Align::Center => (space - len) / 2,
        Align::End => space - len,
    };
    (start + offset, len)
}

fn place(tree: &mut Tree, id: NodeId, rect: Rect, content: &HashMap<NodeId, (i32, i32)>) {
    let Some(node) = tree.get_mut(id) else { return };
    node.rect = rect;
    let Kind::Container(c) = &node.kind else {
        return;
    };
    let (axis, spacing, align) = (c.axis, c.spacing, c.align);
    let area = inner(rect, c.padding);
    let children = node.children.clone();
    let sizes: Vec<(Size, Size, (i32, i32))> = children
        .iter()
        .map(|&child| {
            let n = tree.get(child).expect("a child of a live node is live");
            (n.width, n.height, content[&child])
        })
        .collect();

    let mut rects = Vec::with_capacity(children.len());
    match axis {
        None => {
            for &(w, h, (cw, ch)) in &sizes {
                let (x, width) = cross(w, cw, align, area.x, area.w);
                let (y, height) = cross(h, ch, align, area.y, area.h);
                rects.push(Rect::new(x, y, width, height));
            }
        }
        Some(axis) => {
            let horizontal = axis == Axis::Horizontal;
            let (start, space) = if horizontal {
                (area.x, area.w)
            } else {
                (area.y, area.h)
            };
            let main = |&(w, h, (cw, ch)): &(Size, Size, (i32, i32))| {
                if horizontal { (w, cw) } else { (h, ch) }
            };
            let gaps = spacing * (children.len() as i32 - 1).max(0);
            let used: i32 = sizes
                .iter()
                .map(|s| match main(s) {
                    (Size::Fixed(v), _) => v.max(0),
                    (Size::Content, c) => c,
                    (Size::Flex(_), _) => 0,
                })
                .sum::<i32>()
                + gaps;
            let left = (space - used).max(0);
            let weights: u32 = sizes
                .iter()
                .map(|s| match main(s) {
                    (Size::Flex(w), _) => u32::from(w),
                    _ => 0,
                })
                .sum();
            let mut shares: Vec<i32> = sizes
                .iter()
                .map(|s| match main(s) {
                    (Size::Flex(w), _) if weights > 0 => {
                        (i64::from(left) * i64::from(w) / i64::from(weights)) as i32
                    }
                    _ => 0,
                })
                .collect();
            let mut rest = if weights > 0 {
                left - shares.iter().sum::<i32>()
            } else {
                0
            };
            for (i, s) in sizes.iter().enumerate() {
                if rest == 0 {
                    break;
                }
                if matches!(main(s), (Size::Flex(w), _) if w > 0) {
                    shares[i] += 1;
                    rest -= 1;
                }
            }
            let mut pos = start;
            for (i, s) in sizes.iter().enumerate() {
                let len = match main(s) {
                    (Size::Fixed(v), _) => v.max(0),
                    (Size::Content, c) => c,
                    (Size::Flex(_), _) => shares[i],
                };
                let (cs, cc) = if horizontal {
                    (s.1, s.2.1)
                } else {
                    (s.0, s.2.0)
                };
                let rect = if horizontal {
                    let (y, h) = cross(cs, cc, align, area.y, area.h);
                    Rect::new(pos, y, len, h)
                } else {
                    let (x, w) = cross(cs, cc, align, area.x, area.w);
                    Rect::new(x, pos, w, len)
                };
                rects.push(rect);
                pos += len + spacing;
            }
        }
    }
    for (&child, rect) in children.iter().zip(rects) {
        place(tree, child, clip(rect, area), content);
    }
}

/// `rect` cut to `area`, each axis on its own (a child with no height keeps its width).
fn clip(rect: Rect, area: Rect) -> Rect {
    let span = |start: i32, len: i32, lo: i32, hi: i32| {
        let a = start.clamp(lo, hi);
        let b = (start + len.max(0)).clamp(lo, hi);
        (a, b - a)
    };
    let (x, w) = span(rect.x, rect.w, area.x, area.right());
    let (y, h) = span(rect.y, rect.h, area.y, area.bottom());
    Rect::new(x, y, w, h)
}
