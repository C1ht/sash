// SPDX-License-Identifier: Apache-2.0

//! The widget host: a tree, its layout, focus, hover and press state, animations, and
//! drawing limited to damage. It is a [`View`], so a [`crate::Panel`] shows it.
//!
//! Logical state changes at once: a key or click moves focus, presses or activates
//! before anything is drawn, and the next input acts on the new state. Only the drawn
//! ring position and hover fills follow over [`crate::anim::ANIMATION`], on the time the
//! caller passes in. Every change marks just the widgets it touches.

use std::{collections::HashMap, rc::Rc, time::Duration};

use sash_text::{Image, Style, TextStack};

use crate::{
    anim::Tween,
    canvas::{Canvas, Color, to_device},
    damage::Damage,
    focus,
    geometry::Rect,
    icon::IconRaster,
    input::{Input, Key, Pointer},
    layout::layout,
    panel::View,
    theme::{Overlay, Surface, TextLevel, Theme},
    tree::{
        BUTTON_PADDING, Button, ButtonStyle, ICON_GAP, Icon, Issue, Kind, Node, NodeId, Semantics,
        Text, Tree,
    },
};

/// The focus ring (docs/design.md, through sash-theme): 2 px wide, 3 px outside the
/// widget (logical pixels).
pub const RING_WIDTH: i32 = crate::theme::FOCUS_RING_WIDTH;
pub const RING_GAP: i32 = crate::theme::FOCUS_RING_OFFSET;
/// Chrome radius (docs/design.md): min(shorter side x RADIUS_FACTOR, MAX_RADIUS).
pub const RADIUS_FACTOR: f32 = 0.236;
pub const MAX_RADIUS: f32 = 21.0;
/// The spec's hand-set exception: the dock and the launcher panel.
pub const PANEL_RADIUS: f32 = 34.0;

/// The radius of a widget at `rect` (device pixels) at `scale`.
pub fn radius(rect: Rect, scale: f32) -> f32 {
    (rect.w.min(rect.h) as f32 * RADIUS_FACTOR).min(MAX_RADIUS * scale)
}

/// What happened, for the app to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent {
    Activated(NodeId),
    /// Escape with nothing open to close.
    Escape,
}

struct CachedText {
    key: (u32, [u8; 4], i32),
    image: Image,
    height: f32,
}

pub struct Ui {
    tree: Tree,
    theme: Theme,
    background: Surface,
    scale: f32,
    /// Logical size.
    size: (i32, i32),
    text: TextStack,
    focus: Option<NodeId>,
    focus_visible: bool,
    /// The widget each group was last focused at, by its outermost group.
    roving: HashMap<NodeId, NodeId>,
    hover: Option<NodeId>,
    pressed: Option<NodeId>,
    ring: Tween<Rect>,
    ring_shown: bool,
    fades: HashMap<NodeId, Tween<f32>>,
    now: Duration,
    /// Device pixels.
    damage: Damage,
    events: Vec<UiEvent>,
    texts: HashMap<NodeId, CachedText>,
    icons: HashMap<(NodeId, u32), Option<Rc<IconRaster>>>,
}

impl Ui {
    pub fn new(root: Node, theme: Theme, background: Surface) -> Self {
        let ui = Self {
            tree: Tree::new(root),
            theme,
            background,
            scale: 1.0,
            size: (0, 0),
            text: TextStack::new(),
            focus: None,
            focus_visible: false,
            roving: HashMap::new(),
            hover: None,
            pressed: None,
            ring: Tween::at(Rect::default()),
            ring_shown: false,
            fades: HashMap::new(),
            now: Duration::ZERO,
            damage: Damage::default(),
            events: Vec::new(),
            texts: HashMap::new(),
            icons: HashMap::new(),
        };
        ui.check();
        ui
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn root(&self) -> NodeId {
        self.tree.root()
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Adds `node` as the last child of `parent`.
    pub fn add(&mut self, parent: NodeId, node: Node) -> NodeId {
        let id = self.tree.add(parent, node);
        self.check();
        self.relayout();
        id
    }

    /// Removes `id` and its subtree; focus moves on if it was inside.
    pub fn remove(&mut self, id: NodeId) {
        if id == self.root() {
            return;
        }
        let before = focus::focusables(&self.tree);
        let gone: Vec<NodeId> = self
            .tree
            .order()
            .into_iter()
            .filter(|&n| self.is_within(n, id))
            .collect();
        for &n in &gone {
            if let Some(rect) = self.tree.get(n).map(Node::rect) {
                self.mark(rect);
            }
            self.texts.remove(&n);
            self.fades.remove(&n);
            self.icons.retain(|&(i, _), _| i != n);
        }
        self.tree.remove(id);
        if self.hover.is_some_and(|h| gone.contains(&h)) {
            self.hover = None;
        }
        if self.pressed.is_some_and(|p| gone.contains(&p)) {
            self.pressed = None;
        }
        self.keep_focus(&before);
        self.relayout();
    }

    pub fn set_disabled(&mut self, id: NodeId, disabled: bool) {
        let before = focus::focusables(&self.tree);
        let Some(node) = self.tree.get_mut(id) else {
            return;
        };
        if node.disabled == disabled {
            return;
        }
        node.disabled = disabled;
        let gone: Vec<NodeId> = self
            .tree
            .order()
            .into_iter()
            .filter(|&n| self.is_within(n, id))
            .collect();
        for n in gone {
            if let Some(rect) = self.tree.get(n).map(Node::rect) {
                self.mark(rect);
            }
            if disabled && self.hover == Some(n) {
                self.set_hover(None);
            }
            if disabled && self.pressed == Some(n) {
                self.pressed = None;
            }
        }
        self.keep_focus(&before);
    }

    /// Changes a label's or a button's text.
    pub fn set_text(&mut self, id: NodeId, text: impl Into<Text>) {
        let Some(node) = self.tree.get_mut(id) else {
            return;
        };
        match &mut node.kind {
            Kind::Label(l) => l.text = text.into(),
            Kind::Button(b) => b.label = Some(text.into()),
            _ => return,
        }
        let rect = node.rect;
        self.texts.remove(&id);
        self.mark(rect);
        self.check();
        self.relayout();
    }

    /// Device pixels per logical pixel (1, 1.5, 2, ...).
    pub fn set_scale(&mut self, scale: f32) {
        if scale.is_finite() && scale > 0.0 && scale != self.scale {
            self.scale = scale;
            self.mark_all();
        }
    }

    /// The logical size of the surface.
    pub fn resize(&mut self, width: i32, height: i32) {
        if self.size != (width, height) {
            self.size = (width.max(0), height.max(0));
            self.relayout();
            self.mark_all();
        }
    }

    pub fn focused(&self) -> Option<NodeId> {
        self.focus
    }

    /// Whether the focus ring is shown (keyboard focus).
    pub fn focus_visible(&self) -> bool {
        self.focus_visible && self.focus.is_some()
    }

    pub fn hovered(&self) -> Option<NodeId> {
        self.hover
    }

    pub fn pressed(&self) -> Option<NodeId> {
        self.pressed
    }

    /// Moves keyboard focus to `id` (with the ring), or clears it.
    pub fn set_focus(&mut self, id: Option<NodeId>) {
        let id = id.filter(|&i| self.tree.is_focusable(i));
        self.move_focus(id, true);
    }

    pub fn issues(&self) -> Vec<Issue> {
        self.tree.issues()
    }

    /// The tree as an assistive technology sees it, in reading order.
    pub fn semantics(&self) -> Vec<Semantics<'_>> {
        self.tree
            .order()
            .into_iter()
            .filter_map(|id| {
                let node = self.tree.get(id)?;
                Some(Semantics {
                    id,
                    role: node.role()?,
                    name: node.name(),
                    rect: node.rect,
                    focusable: self.tree.is_focusable(id),
                    focused: self.focus == Some(id),
                    disabled: self.tree.is_disabled(id),
                    pressed: self.pressed == Some(id),
                    actions: node.actions(),
                })
            })
            .collect()
    }

    pub fn take_events(&mut self) -> Vec<UiEvent> {
        std::mem::take(&mut self.events)
    }

    /// What changed since the last call, in device pixels.
    pub fn take_damage(&mut self) -> Damage {
        std::mem::take(&mut self.damage)
    }

    /// The focus ring where it is drawn now (logical), if shown.
    pub fn drawn_ring(&self) -> Option<Rect> {
        self.ring_shown.then(|| self.ring.value(self.now))
    }

    /// Where the ring is heading (logical), if shown.
    pub fn ring_target(&self) -> Option<Rect> {
        self.ring_shown.then(|| self.ring.target())
    }

    /// The drawn hover fill of `id`, 0 to 1.
    pub fn drawn_hover(&self, id: NodeId) -> f32 {
        self.fades.get(&id).map_or(0.0, |t| t.value(self.now))
    }

    pub fn animating(&self) -> bool {
        (self.ring_shown && self.ring.running(self.now))
            || self.fades.values().any(|t| t.running(self.now))
    }

    /// Moves the clock to `now` and marks what the running animations change.
    pub fn advance(&mut self, now: Duration) {
        let then = self.now;
        if now <= then {
            return;
        }
        if self.ring_shown && self.ring.running(then) {
            self.mark(self.ring.value(then));
            self.mark(self.ring.value(now));
        }
        let fading: Vec<NodeId> = self
            .fades
            .iter()
            .filter(|(_, t)| t.running(then))
            .map(|(&id, _)| id)
            .collect();
        for id in fading {
            if let Some(rect) = self.tree.get(id).map(Node::rect) {
                self.mark(rect);
            }
        }
        self.now = now;
        self.fades.retain(|_, t| t.running(now) || t.target() > 0.0);
    }

    pub fn key(&mut self, key: Key, now: Duration) {
        self.advance(now);
        match key {
            Key::Tab | Key::BackTab => {
                let to = focus::tab(&self.tree, self.focus, key == Key::BackTab, &self.roving);
                self.move_focus(to, true);
            }
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => {
                let to = self.focus.and_then(|f| focus::arrow(&self.tree, f, key));
                // An arrow shows the ring even when it moves nothing.
                self.move_focus(to.or(self.focus), true);
            }
            Key::Enter | Key::Space => {
                if let Some(f) = self.focus.filter(|&f| self.tree.is_focusable(f)) {
                    self.events.push(UiEvent::Activated(f));
                }
            }
            Key::Escape => self.events.push(UiEvent::Escape),
        }
    }

    pub fn pointer(&mut self, event: Pointer, now: Duration) {
        self.advance(now);
        match event {
            Pointer::Motion { x, y } => {
                let hit = self.hit(x, y);
                self.set_hover(hit);
            }
            Pointer::Leave => {
                self.set_hover(None);
                self.set_pressed(None);
            }
            Pointer::Press { x, y } => {
                let hit = self.hit(x, y);
                self.set_hover(hit);
                if let Some(id) = hit {
                    self.set_pressed(Some(id));
                    // A click focuses without the ring.
                    self.move_focus(Some(id), false);
                }
            }
            Pointer::Release { x, y } => {
                let hit = self.hit(x, y);
                if let Some(id) = self.pressed.filter(|&p| Some(p) == hit) {
                    self.events.push(UiEvent::Activated(id));
                }
                self.set_pressed(None);
                self.set_hover(hit);
            }
        }
    }

    /// The topmost enabled interactive widget at (x, y), logical pixels.
    pub fn hit(&self, x: f64, y: f64) -> Option<NodeId> {
        let (x, y) = (x.floor() as i32, y.floor() as i32);
        self.tree.order().into_iter().rev().find(|&id| {
            self.tree.is_focusable(id) && self.tree.get(id).is_some_and(|n| n.rect.contains(x, y))
        })
    }

    fn check(&self) {
        let issues = self.tree.issues();
        debug_assert!(
            issues.is_empty(),
            "interactive widgets without an accessible name: {issues:?}"
        );
    }

    fn is_within(&self, node: NodeId, ancestor: NodeId) -> bool {
        let mut next = Some(node);
        while let Some(n) = next {
            if n == ancestor {
                return true;
            }
            next = self.tree.get(n).and_then(Node::parent);
        }
        false
    }

    fn keep_focus(&mut self, before: &[NodeId]) {
        if let Some(f) = self.focus.filter(|&f| !self.tree.is_focusable(f)) {
            let to = focus::successor(&self.tree, before, f);
            let visible = self.focus_visible;
            self.move_focus(to, visible);
        }
    }

    fn move_focus(&mut self, to: Option<NodeId>, visible: bool) {
        if let Some(id) = to {
            let stop = focus::outermost_group(&self.tree, id).unwrap_or(id);
            self.roving.insert(stop, id);
        }
        self.focus = to;
        self.focus_visible = visible;
        self.update_ring();
    }

    fn ring_rect(&self, id: NodeId) -> Option<Rect> {
        let r = self.tree.get(id)?.rect;
        let out = RING_GAP + RING_WIDTH;
        Some(Rect::new(
            r.x - out,
            r.y - out,
            r.w + 2 * out,
            r.h + 2 * out,
        ))
    }

    fn update_ring(&mut self) {
        let target = if self.focus_visible {
            self.focus.and_then(|f| self.ring_rect(f))
        } else {
            None
        };
        match (self.ring_shown, target) {
            (true, None) => {
                self.mark(self.ring.value(self.now));
                self.ring_shown = false;
            }
            (false, None) => {}
            (false, Some(t)) => {
                self.ring.jump(t);
                self.ring_shown = true;
                self.mark(t);
            }
            (true, Some(t)) => {
                if t != self.ring.target() {
                    self.mark(self.ring.value(self.now));
                    self.ring.retarget(t, self.now);
                    self.mark(t);
                }
            }
        }
    }

    fn set_hover(&mut self, to: Option<NodeId>) {
        if to == self.hover {
            return;
        }
        let now = self.now;
        for (id, target) in [(self.hover, 0.0), (to, 1.0)] {
            let Some(id) = id else { continue };
            self.fades
                .entry(id)
                .or_insert(Tween::at(1.0 - target))
                .retarget(target, now);
            if let Some(rect) = self.tree.get(id).map(Node::rect) {
                self.mark(rect);
            }
        }
        self.hover = to;
    }

    fn set_pressed(&mut self, to: Option<NodeId>) {
        if to == self.pressed {
            return;
        }
        for id in [self.pressed, to].into_iter().flatten() {
            if let Some(rect) = self.tree.get(id).map(Node::rect) {
                self.mark(rect);
            }
        }
        self.pressed = to;
    }

    fn relayout(&mut self) {
        let before: HashMap<NodeId, Rect> = self
            .tree
            .order()
            .into_iter()
            .filter_map(|id| Some((id, self.tree.get(id)?.rect)))
            .collect();
        let text = &mut self.text;
        layout(
            &mut self.tree,
            Rect::sized(self.size.0, self.size.1),
            &mut |_, node| measure(text, node),
        );
        for id in self.tree.order() {
            let Some(now) = self.tree.get(id).map(Node::rect) else {
                continue;
            };
            match before.get(&id) {
                Some(old) if *old == now => {}
                Some(&old) => {
                    self.mark(old);
                    self.mark(now);
                    self.texts.remove(&id);
                }
                None => self.mark(now),
            }
        }
        if self.ring_shown
            && let Some(t) = self.focus.and_then(|f| self.ring_rect(f))
            && t != self.ring.target()
        {
            self.mark(self.ring.value(self.now));
            self.ring.jump(t);
            self.mark(t);
        }
    }

    fn device_size(&self) -> Rect {
        to_device(Rect::sized(self.size.0, self.size.1), self.scale)
    }

    /// Marks a logical rectangle, with room for the focus ring and anti-aliasing.
    fn mark(&mut self, rect: Rect) {
        if rect.w <= 0 && rect.h <= 0 {
            return;
        }
        let out = RING_GAP + RING_WIDTH + 1;
        let grown = Rect::new(
            rect.x - out,
            rect.y - out,
            rect.w + 2 * out,
            rect.h + 2 * out,
        );
        let device = to_device(grown, self.scale).intersect(&self.device_size());
        self.damage.add(device);
    }

    fn mark_all(&mut self) {
        self.damage = Damage::full(self.device_size());
    }

    /// Draws everything inside the canvas's clip, at the canvas's scale.
    pub fn draw(&mut self, canvas: &mut Canvas) {
        let s = canvas.scale();
        let clip = canvas.clip();
        canvas.fill(clip, self.theme.surface_color(self.background));
        let out = RING_GAP + RING_WIDTH + 1;
        let visible: Vec<(NodeId, Rect, Kind)> = self
            .tree
            .order()
            .into_iter()
            .filter_map(|id| {
                let node = self.tree.get(id)?;
                let r = node.rect;
                let grown = Rect::new(r.x - out, r.y - out, r.w + 2 * out, r.h + 2 * out);
                if to_device(grown, s).intersect(&clip).is_empty() {
                    return None;
                }
                Some((id, r, node.kind.clone()))
            })
            .collect();
        for (id, rect, kind) in visible {
            let disabled = self.tree.is_disabled(id);
            match kind {
                Kind::Container(_) => {}
                Kind::Label(l) => {
                    let level = if disabled {
                        TextLevel::Secondary
                    } else {
                        l.level
                    };
                    let color = self.theme.text(level);
                    self.draw_text(canvas, id, &l.text, rect, color);
                }
                Kind::Icon(i) => {
                    let level = if disabled {
                        TextLevel::Secondary
                    } else {
                        TextLevel::Primary
                    };
                    let color = self.theme.text(level);
                    self.draw_icon(canvas, id, &i, rect, color);
                }
                Kind::Button(b) => self.draw_button(canvas, id, &b, rect, disabled),
            }
        }
        if self.ring_shown {
            let r = to_device(self.ring.value(self.now), s);
            let width = ((RING_WIDTH as f32 * s).round() as i32).max(1);
            let outset = ((RING_GAP + RING_WIDTH) as f32 * s).round();
            // Concentric with the widget's corners: its radius plus the outset.
            let o = outset as i32;
            let widget = Rect::new(r.x + o, r.y + o, r.w - 2 * o, r.h - 2 * o);
            let radius = radius(widget, s) + outset;
            canvas.ring(r, radius, width, self.theme.accent());
        }
    }

    fn draw_button(&mut self, canvas: &mut Canvas, id: NodeId, b: &Button, rect: Rect, off: bool) {
        let s = canvas.scale();
        let r = to_device(rect, s);
        let radius = radius(r, s);
        let px = |v: i32| ((v as f32 * s).round() as i32).max(1);
        let inset = |r: Rect, d: i32| Rect::new(r.x + d, r.y + d, r.w - 2 * d, r.h - 2 * d);
        let hover = self.drawn_hover(id);
        let pressed = self.pressed == Some(id);
        let theme = self.theme;
        // A colour at `k` (0..1) of its own alpha: hover fades in and out.
        let faded = |c: crate::canvas::Color, k: f32| {
            c.with_alpha(f32::from(c.a) / 255.0 * k.clamp(0.0, 1.0))
        };
        let content = match b.style {
            ButtonStyle::Flat => {
                if off {
                    // Disabled: the secondary token, and no hover or pressed shape.
                } else if pressed {
                    // Pressed: the 8% fill, a pixel smaller (a change of shape).
                    canvas.fill_rounded(inset(r, px(1)), radius, theme.overlay(Overlay::Pressed));
                } else if hover > 0.0 {
                    // Hover: the 5% fill and a hairline edge (never only a fill).
                    canvas.fill_rounded(r, radius, faded(theme.overlay(Overlay::Hover), hover));
                    canvas.ring(r, radius, px(1), faded(theme.hairline(), hover));
                }
                theme.text(if off {
                    TextLevel::Secondary
                } else {
                    TextLevel::Primary
                })
            }
            ButtonStyle::Accent if off => {
                // Disabled: the outline only, no fill (a state shape), secondary text.
                canvas.ring(r, radius, px(1), theme.accent());
                theme.text(TextLevel::Secondary)
            }
            ButtonStyle::Accent => {
                let body = if pressed { inset(r, px(1)) } else { r };
                canvas.fill_rounded(body, radius, theme.accent_fill());
                // The outline marks the shape (owner, after E2: interactive shapes get
                // an outline or a state shape, never only a fill). Hover widens it to
                // 2 px: a change of shape.
                let width = if hover > 0.5 && !pressed {
                    px(2)
                } else {
                    px(1)
                };
                canvas.ring(r, radius, width, theme.accent());
                theme.on_accent()
            }
        };
        let mut x = rect.x + BUTTON_PADDING;
        match (&b.icon, &b.label) {
            (Some(icon), None) => self.draw_icon(canvas, id, icon, rect, content),
            (icon, Some(label)) => {
                if let Some(icon) = icon {
                    let slot = Rect::new(x, rect.y, icon.size, rect.h);
                    self.draw_icon(canvas, id, icon, slot, content);
                    x += icon.size + ICON_GAP;
                }
                let right = rect.right() - BUTTON_PADDING;
                let slot = Rect::new(x, rect.y, (right - x).max(0), rect.h);
                self.draw_text(canvas, id, label, slot, content);
            }
            (None, None) => {}
        }
    }

    /// Draws `text` in `slot` (logical), left-aligned, centred vertically, cut with an
    /// ellipsis at the slot's width.
    fn draw_text(
        &mut self,
        canvas: &mut Canvas,
        id: NodeId,
        text: &Text,
        slot: Rect,
        color: Color,
    ) {
        let s = canvas.scale();
        let key = (s.to_bits(), color.rgba(), slot.w);
        if self.texts.get(&id).is_none_or(|c| c.key != key) {
            let line = self
                .text
                .line(&text.shaped(), Style::default(), Some(slot.w as f32));
            let image = self.text.draw(&line, s, color.rgba());
            self.texts.insert(
                id,
                CachedText {
                    key,
                    image,
                    height: line.height(),
                },
            );
        }
        let cached = &self.texts[&id];
        let top = slot.y as f32 + (slot.h as f32 - cached.height) / 2.0;
        let x = (slot.x as f32 * s).round() as i32;
        let y = (top * s).round() as i32;
        canvas.text(&cached.image, x, y);
    }

    /// Draws `icon` centred in `slot` (logical).
    fn draw_icon(
        &mut self,
        canvas: &mut Canvas,
        id: NodeId,
        icon: &Icon,
        slot: Rect,
        color: Color,
    ) {
        let s = canvas.scale();
        let px = ((icon.size as f32 * s).round() as u32).max(1);
        let raster = self
            .icons
            .entry((id, px))
            .or_insert_with(|| icon.source.raster(px, s));
        let d = to_device(slot, s);
        let x = d.x + (d.w - px as i32) / 2;
        let y = d.y + (d.h - px as i32) / 2;
        match raster.as_deref() {
            Some(IconRaster::Mask(alpha)) => canvas.mask(alpha, px, x, y, color),
            Some(IconRaster::Color(rgba)) => canvas.image(rgba, px, x, y),
            None => {}
        }
    }
}

fn measure(text: &mut TextStack, node: &Node) -> (i32, i32) {
    let label = match &node.kind {
        Kind::Label(l) => &l.text,
        Kind::Button(Button { label: Some(l), .. }) => l,
        _ => return (0, 0),
    };
    let line = text.line(&label.shaped(), Style::default(), None);
    (line.width().ceil() as i32, line.height().ceil() as i32)
}

impl View for Ui {
    fn draw(&mut self, canvas: &mut Canvas) {
        Ui::draw(self, canvas);
    }

    fn configure(&mut self, size: (u32, u32)) {
        let w = (size.0 as f32 / self.scale).floor() as i32;
        let h = (size.1 as f32 / self.scale).floor() as i32;
        self.resize(w, h);
        // The panel draws everything after a configure itself.
        self.damage.clear();
    }

    fn input(&mut self, input: Input, now: Duration) -> Damage {
        match input {
            Input::Key(key) => self.key(key, now),
            Input::Pointer(p) => self.pointer(p, now),
        }
        self.take_damage()
    }

    fn animating(&self) -> bool {
        Ui::animating(self)
    }

    fn frame(&mut self, now: Duration) -> Damage {
        self.advance(now);
        self.take_damage()
    }
}
