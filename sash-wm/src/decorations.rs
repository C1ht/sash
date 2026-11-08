// SPDX-License-Identifier: Apache-2.0

//! Server-side decorations (M11b): the xdg-decoration protocol (every client that
//! negotiates is answered server_side), the pointer on the bar, buttons and resize
//! zones, and what is drawn for them. The geometry is in `layout::decor`.

use std::{cell::RefCell, collections::HashMap};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::memory::MemoryRenderBuffer,
            gles::{GlesRenderer, element::PixelShaderElement},
        },
    },
    input::pointer::{CursorIcon, CursorImageStatus, GrabStartData},
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::{
            zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
            zxdg_toplevel_decoration_v1::{self, Mode as DecorationMode, ZxdgToplevelDecorationV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, backend::ClientId, delegate_dispatch,
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Serial, Transform},
    wayland::shell::xdg::{
        ToplevelSurface,
        decoration::{XdgDecorationHandler, XdgDecorationManagerGlobalData, XdgDecorationState},
    },
};

use sash_theme::{Color, Overlay, Theme};

use crate::{
    layout::{
        Edges, Rect, WindowId,
        decor::{BUTTONS, Button, Decoration, GLYPH, HOVER_RADIUS, HOVER_SQUARE, Part},
        shape::Shape,
    },
    render::OutputElement,
    seat::BTN_LEFT,
    state::Sash,
    windows::DragKind,
};

pub use crate::glyphs::Glyph;

/// How a window's decorations look this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub focused: bool,
    pub hover: Option<Button>,
    pub maximized: bool,
    /// Drawn through the rounded shape (the shaders compiled, the window has one).
    pub rounded: bool,
    /// The inactive dim over the client area (owner, M18b): unfocused, not fullscreen,
    /// the effect on.
    pub dim: bool,
}

/// One thing drawn for a window's decorations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    /// A fill placed by the window's frame: square windows, the outline's strips.
    Fill(Rect, Color),
    /// A fill clipped by the window's rounded shape (M15b step 2): the bar, the dim.
    ShapeFill(Rect, Color),
    /// A hovered button's square (HOVER_SQUARE, rounded by HOVER_RADIUS), centred in
    /// this button area.
    Square(Rect, Color),
    /// The outline as one ring along the rounded shape, on these edges.
    Ring(Edges, Color),
    /// A strip of the outline of a square window.
    Outline(Rect, Color),
    /// A glyph centred in this button area, in this ink.
    Glyph(Glyph, Rect, Color),
}

/// What is drawn for a window's decorations, bottom first: the dim over the client
/// area, the bar (reaching under the outline, so nothing shows through a 1.5 px outline
/// at fractional scales), each hovered button's square and every glyph, then the
/// outline on top (owner, M18b), on the bar tone. Focus shows by the bar's tone, the outline's alpha, the
/// glyphs' ink and the dim; never by a coloured outline.
pub fn pieces(theme: &Theme, outer: Rect, decoration: Decoration, look: Look) -> Vec<Piece> {
    let mut out = Vec::new();
    if decoration == Decoration::NONE {
        return out;
    }
    let fill = |rect: Rect, color: Color| {
        if look.rounded {
            Piece::ShapeFill(rect, color)
        } else {
            Piece::Fill(rect, color)
        }
    };
    if look.dim && !look.focused {
        let inner = decoration.inner(outer);
        if inner.w > 0 && inner.h > 0 {
            out.push(fill(inner, theme.dim));
        }
    }
    if let Some(bar) = decoration.bar_rect(outer) {
        let band = Rect::new(outer.x, outer.y, outer.w, bar.bottom() - outer.y);
        out.push(fill(band, theme.bar(look.focused)));
        for button in BUTTONS {
            let Some(area) = decoration.button_rect(outer, button) else {
                continue;
            };
            let hovered = look.hover == Some(button);
            if hovered && area.w >= HOVER_SQUARE && area.h >= HOVER_SQUARE {
                let color = match button {
                    Button::Close => theme.close_hover,
                    Button::Maximize => theme.overlay(Overlay::Strong),
                };
                out.push(Piece::Square(area, color));
            }
            if area.w >= GLYPH && area.h >= GLYPH {
                let glyph = match button {
                    Button::Close => Glyph::Close,
                    Button::Maximize if look.maximized => Glyph::Restore,
                    Button::Maximize => Glyph::Maximize,
                };
                let ink = if hovered {
                    theme.hovered_glyph()
                } else {
                    theme.bar_ink(look.focused)
                };
                out.push(Piece::Glyph(glyph, area, ink));
            }
        }
    }
    let edges = decoration.border;
    // The outline is a translucent token drawn on the window's own frame: over the bar
    // tone, made opaque, so the 1 px frame beside the client never shows what is behind
    // the window (in the bar the pixel is the same either way).
    let outline = sash_theme::over(theme.outline(look.focused), theme.bar(look.focused));
    if edges != Edges::NONE && look.rounded {
        out.push(Piece::Ring(edges, outline));
    } else if edges != Edges::NONE {
        let b = crate::layout::decor::BORDER;
        // Top and bottom strips run the full width; the sides fill between them.
        let top = if edges.top { b.min(outer.h) } else { 0 };
        let bottom = if edges.bottom { b.min(outer.h) } else { 0 };
        let side_h = (outer.h - top - bottom).max(0);
        let strips = [
            (edges.top, Rect::new(outer.x, outer.y, outer.w, top)),
            (
                edges.bottom,
                Rect::new(outer.x, outer.bottom() - bottom, outer.w, bottom),
            ),
            (
                edges.left,
                Rect::new(outer.x, outer.y + top, b.min(outer.w), side_h),
            ),
            (
                edges.right,
                Rect::new(outer.right() - b, outer.y + top, b.min(outer.w), side_h),
            ),
        ];
        for (_, rect) in strips.into_iter().filter(|(on, _)| *on) {
            out.push(Piece::Outline(rect, outline));
        }
    }
    out
}

/// A `size` square centred in the physical rectangle `area` (the hover square and the
/// glyphs; the renderer and the CPU reference place them alike).
pub fn centred_px(area: Rectangle<i32, Physical>, size: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(
        (
            area.loc.x + (area.size.w - size).div_euclid(2),
            area.loc.y + (area.size.h - size).div_euclid(2),
        )
            .into(),
        (size, size).into(),
    )
}

/// The cursor shape for a resize zone.
pub fn resize_icon(edges: Edges) -> CursorIcon {
    match (edges.left, edges.right, edges.top, edges.bottom) {
        (true, _, true, _) => CursorIcon::NwResize,
        (_, true, true, _) => CursorIcon::NeResize,
        (true, _, _, true) => CursorIcon::SwResize,
        (_, true, _, true) => CursorIcon::SeResize,
        (true, ..) => CursorIcon::WResize,
        (_, true, ..) => CursorIcon::EResize,
        (_, _, true, _) => CursorIcon::NResize,
        _ => CursorIcon::SResize,
    }
}

/// A filled rectangle's buffer, with where and in which colour it was last drawn.
type Fill = crate::render::Solid;

/// What a cached shader element was made for (it is rebuilt only when this changes, so
/// an unchanged ring or bar is no damage).
#[derive(Clone, Debug, PartialEq)]
struct ShaderKey {
    rect: Rect,
    clip: [i32; 4],
    paint: crate::rounded::Paint,
    color: [u32; 4],
    shape: [u32; 8],
    scale: u64,
}

impl ShaderKey {
    fn new(
        rect: Rect,
        clip: Rectangle<i32, Physical>,
        paint: crate::rounded::Paint,
        color: [f32; 4],
        shape: &Shape,
        scale: f64,
    ) -> Self {
        let [a, b, c, d] = shape.radii;
        Self {
            rect,
            clip: [clip.loc.x, clip.loc.y, clip.size.w, clip.size.h],
            paint,
            color: color.map(f32::to_bits),
            shape: [shape.x, shape.y, shape.w, shape.h, a, b, c, d].map(f32::to_bits),
            scale: scale.to_bits(),
        }
    }
}

type Shaders = HashMap<(WindowId, usize), (PixelShaderElement, ShaderKey)>;

/// Glyph buffers by glyph, scale and ink; a handful in practice (bounded anyway).
type GlyphBuffers = HashMap<(Glyph, u64, Color), MemoryRenderBuffer>;
const GLYPH_BUFFERS: usize = 64;

/// The pointer on decorations, and the buffers drawn for them (kept from frame to
/// frame so damage tracking sees unchanged decorations as unchanged).
pub struct Decorations {
    /// The decoration under the pointer.
    pub hover: Option<(WindowId, Part)>,
    /// A button pressed and not released yet: it acts on release over it.
    pressed: Option<(WindowId, Button, u32)>,
    fills: RefCell<HashMap<(WindowId, usize), Fill>>,
    /// The ring and clipped fills of rounded windows (rounded.rs).
    shaders: RefCell<Shaders>,
    glyphs: RefCell<GlyphBuffers>,
}

impl Decorations {
    pub fn new() -> Self {
        Self {
            hover: None,
            pressed: None,
            fills: RefCell::new(HashMap::new()),
            shaders: RefCell::new(HashMap::new()),
            glyphs: RefCell::new(HashMap::new()),
        }
    }

    /// The glyph's buffer at `scale` in `ink`, made on first use.
    fn glyph(&self, glyph: Glyph, scale: f64, ink: Color) -> MemoryRenderBuffer {
        let mut glyphs = self.glyphs.borrow_mut();
        if glyphs.len() >= GLYPH_BUFFERS {
            glyphs.clear();
        }
        glyphs
            .entry((glyph, scale.to_bits(), ink))
            .or_insert_with(|| {
                let n = crate::glyphs::size_px(scale);
                MemoryRenderBuffer::from_slice(
                    &crate::glyphs::argb(glyph, scale, ink),
                    Fourcc::Argb8888,
                    (n, n),
                    1,
                    Transform::Normal,
                    None,
                )
            })
            .clone()
    }

    /// Forgets the buffers of windows that are not drawn any more.
    pub fn retain(&self, drawn: &[WindowId]) {
        self.fills
            .borrow_mut()
            .retain(|(window, _), _| drawn.contains(window));
        self.shaders
            .borrow_mut()
            .retain(|(window, _), _| drawn.contains(window));
    }
}

impl Sash {
    /// How window `id`'s decorations look this frame.
    pub fn decoration_look(&self, id: WindowId, rounded: bool) -> Look {
        let layout = &self.windows.layout;
        let hover = match self.decorations.hover {
            Some((window, Part::Button(button))) if window == id => Some(button),
            _ => None,
        };
        Look {
            focused: layout.focused() == Some(id),
            hover,
            maximized: layout.is_maximized(id),
            rounded,
            dim: self.dim && layout.fullscreen() != Some(id),
        }
    }
}

/// Render elements for one window's decorations, front to back.
pub fn decoration_elements(
    state: &Sash,
    renderer: &mut GlesRenderer,
    id: WindowId,
    outer: Rect,
    decoration: Decoration,
    scale: Scale<f64>,
) -> Vec<OutputElement> {
    let layout = &state.windows.layout;
    // The rounded shape (M15b step 2) clips the bar and the dim and carries the outline.
    let programs = crate::rounded::programs(renderer);
    let shaped = layout
        .shape(id, scale.x as f32)
        .and_then(|shape| programs.as_ref().map(|p| (p.shape.clone(), shape)));
    let look = state.decoration_look(id, shaped.is_some());
    let pieces = pieces(state.theme(), outer, decoration, look);
    // Everything is placed by the window's frame, so it lines up with its edges.
    let Some(frame) = layout.frame(id, scale.x) else {
        return Vec::new();
    };
    let decorations = &state.decorations;
    let mut fills = decorations.fills.borrow_mut();
    let mut shader_cache = decorations.shaders.borrow_mut();
    let mut out = Vec::with_capacity(pieces.len() + 1);
    let mut shader = |index: usize,
                      rect: Rect,
                      clip: Rectangle<i32, Physical>,
                      shape: &Shape,
                      program: &smithay::backend::renderer::gles::GlesPixelProgram,
                      paint: crate::rounded::Paint,
                      color: Color| {
        let color = color.rgba_f32();
        let key = ShaderKey::new(rect, clip, paint, color, shape, scale.x);
        let cached = shader_cache.remove(&(id, index));
        let element = match cached {
            Some((element, was)) if was == key => element,
            cached => crate::rounded::shape_element(
                program,
                cached.map(|(e, _)| e),
                rect,
                clip,
                shape,
                scale.x,
                paint,
                color,
            ),
        };
        shader_cache.insert((id, index), (element.clone(), key));
        OutputElement::Shader(element)
    };
    // The title goes above the bar and glyphs, below the outline.
    let mut title_at = None;
    for (index, piece) in pieces.into_iter().enumerate() {
        if matches!(piece, Piece::Ring(..) | Piece::Outline(..)) {
            title_at.get_or_insert(out.len());
        }
        match (piece, &shaped) {
            (Piece::ShapeFill(rect, color), Some((program, shape))) => {
                let clip = frame.map(rect);
                out.push(shader(
                    index,
                    rect,
                    clip,
                    shape,
                    program,
                    crate::rounded::Paint::Fill,
                    color,
                ));
            }
            (Piece::Ring(edges, color), Some((program, shape))) => {
                let paint = crate::rounded::Paint::Ring {
                    width: crate::layout::decor::BORDER,
                    edges,
                };
                out.push(shader(
                    index,
                    outer,
                    frame.physical,
                    shape,
                    program,
                    paint,
                    color,
                ));
            }
            (Piece::Square(area, color), _) => {
                let size = crate::layout::gaps::physical_gap(HOVER_SQUARE, scale.x);
                let square = centred_px(frame.map(area), size);
                match &programs {
                    Some(programs) => {
                        let shape = Shape::from_physical(square, HOVER_RADIUS, scale.x as f32);
                        out.push(shader(
                            index,
                            area,
                            square,
                            &shape,
                            &programs.shape,
                            crate::rounded::Paint::Fill,
                            color,
                        ));
                    }
                    None => {
                        let solid = fills
                            .entry((id, index))
                            .or_insert_with(|| Fill::new(area, color.rgba_f32()));
                        solid.update(area, color.rgba_f32());
                        out.push(OutputElement::Solid(solid.element_at(square, 1.0)));
                    }
                }
            }
            (
                Piece::Fill(rect, color)
                | Piece::ShapeFill(rect, color)
                | Piece::Outline(rect, color),
                _,
            ) => {
                let solid = fills
                    .entry((id, index))
                    .or_insert_with(|| Fill::new(rect, color.rgba_f32()));
                solid.update(rect, color.rgba_f32());
                out.push(OutputElement::Solid(solid.element_in(&frame, 1.0)));
            }
            (Piece::Ring(..), None) => {}
            (Piece::Glyph(glyph, area, ink), _) => {
                let size = crate::glyphs::size_px(scale.x);
                let at = centred_px(frame.map(area), size);
                let buffer = decorations.glyph(glyph, scale.x, ink);
                if let Some(element) = crate::render::Exact::new(renderer, &buffer, at) {
                    out.push(OutputElement::Exact(element));
                }
            }
        }
    }
    // The title, above the bar's fill and below the outline (titles.rs).
    if let Some((drawn, at)) = state.bar_title(id, outer, decoration, scale)
        && let Some(element) = crate::render::Exact::new(renderer, &drawn.buffer, at)
    {
        out.insert(title_at.unwrap_or(out.len()), OutputElement::Exact(element));
    }
    // Front to back: the last piece is on top.
    out.reverse();
    out
}

impl Sash {
    /// The pointer moved (no grab): updates the hovered decoration and the cursor shape
    /// (a resize zone shows its resize shape; the bar, buttons and border the arrow).
    pub fn decoration_hover(&mut self, location: Point<f64, Logical>) {
        let hover = self.decoration_at(location);
        let before = std::mem::replace(&mut self.decorations.hover, hover);
        let button = |h: Option<(WindowId, Part)>| match h {
            Some((w, Part::Button(b))) => Some((w, b)),
            _ => None,
        };
        if button(before) != button(hover) {
            self.request_redraw();
        }
        let icon = match hover {
            Some((_, Part::Resize(edges))) => resize_icon(edges),
            _ => CursorIcon::Default,
        };
        if hover.is_some() || before.is_some() {
            self.set_cursor(CursorImageStatus::Named(icon));
        }
    }

    /// A press on a window's decorations (never delivered to the client): focuses and
    /// raises the window; with the left button, a button is armed (it acts on release),
    /// the bar starts a move and a resize zone a resize, as mod+drag does.
    pub fn decoration_press(
        &mut self,
        window: WindowId,
        part: Part,
        button: u32,
        location: Point<f64, Logical>,
        serial: Serial,
    ) {
        record_context!(self, format!("decoration press {part:?}"));
        self.focus_window(window);
        if button != BTN_LEFT {
            return;
        }
        let kind = match part {
            Part::Button(b) => {
                self.decorations.pressed = Some((window, b, button));
                return;
            }
            Part::Bar => DragKind::Move,
            Part::Resize(edges) => DragKind::Resize(edges),
            Part::Border => return,
        };
        // As with mod+drag: a move dismisses the window's popups, a resize keeps them.
        if matches!(kind, DragKind::Move) {
            self.dismiss_popups(window);
        }
        let start = GrabStartData {
            focus: None,
            button,
            location,
        };
        self.start_pointer_drag(window, kind, start, serial);
    }

    /// A release: a button armed by its press acts if the pointer is still over it.
    pub fn decoration_release(&mut self, button: u32, location: Point<f64, Logical>) {
        let Some((window, pressed, b)) = self.decorations.pressed else {
            return;
        };
        if b != button {
            return;
        }
        self.decorations.pressed = None;
        if self.decoration_at(location) != Some((window, Part::Button(pressed))) {
            return;
        }
        record!(self, "decoration_button", "window" => window, "button" => format!("{pressed:?}"));
        record_context!(self, format!("decoration button {pressed:?}"));
        self.dismiss_popups(window);
        match pressed {
            // What mod+Q does, for this window.
            Button::Close => {
                if let Some(managed) = self.windows.by_id(window) {
                    self.request_close(&managed.window.clone());
                }
            }
            // Toggles the top-zone snap (maximize).
            Button::Maximize => {
                let layout = &mut self.windows.layout;
                let on = !layout.is_maximized(window);
                if layout.set_maximized(window, on) {
                    self.arrange();
                }
            }
        }
    }

    #[cfg_attr(not(feature = "debug-record"), expect(dead_code))]
    fn window_of(&self, toplevel: &ToplevelSurface) -> Option<WindowId> {
        self.windows.by_surface(toplevel.wl_surface()).map(|m| m.id)
    }

    /// The client negotiated xdg-decoration (or destroyed its decoration object).
    fn set_server_side(&mut self, toplevel: &ToplevelSurface, server_side: bool) {
        let Some(id) = self.windows.by_surface(toplevel.wl_surface()).map(|m| m.id) else {
            return;
        };
        if server_side {
            tracing::debug!(window = id.0, "decorations negotiated: server_side");
        } else {
            tracing::debug!(
                window = id.0,
                "decoration object destroyed: client-side again"
            );
        }
        self.windows.set_server_side(id, server_side);
        self.windows.layout.set_server_side(id, server_side);
        if toplevel.is_initial_configure_sent() {
            self.windows.reply_due(id);
            self.arrange();
        }
    }
}

impl XdgDecorationHandler for Sash {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        record!(self, "decoration_mode", "window" => self.window_of(&toplevel), "requested" => "none");
        self.set_server_side(&toplevel, true);
    }

    /// Every request is answered server_side, also one for client_side (logged and
    /// recorded).
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        if mode == DecorationMode::ClientSide {
            let window = self
                .windows
                .by_surface(toplevel.wl_surface())
                .map(|m| m.id.0);
            tracing::info!(
                ?window,
                "client asked for client-side decorations; answered server_side"
            );
        }
        record!(
            self,
            "decoration_mode",
            "window" => self.window_of(&toplevel),
            "requested" => format!("{mode:?}"),
        );
        self.set_server_side(&toplevel, true);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        record!(self, "decoration_mode", "window" => self.window_of(&toplevel), "requested" => "unset");
        self.set_server_side(&toplevel, true);
    }
}

crate::limits::counted_global!(ZxdgDecorationManagerV1: XdgDecorationManagerGlobalData => XdgDecorationState);
delegate_dispatch!(Sash: [ZxdgDecorationManagerV1: ()] => XdgDecorationState);

/// Smithay tells the compositor nothing when a client destroys its decoration object;
/// the client then draws its own decorations again, so sash-wm notes it here and lets
/// Smithay handle the request.
impl Dispatch<ZxdgToplevelDecorationV1, ToplevelSurface> for Sash {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        data: &ToplevelSurface,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let destroy = matches!(request, zxdg_toplevel_decoration_v1::Request::Destroy);
        <XdgDecorationState as Dispatch<ZxdgToplevelDecorationV1, ToplevelSurface, Self>>::request(
            state, client, resource, request, data, display, data_init,
        );
        if destroy {
            data.with_pending_state(|pending| pending.decoration_mode = None);
            state.set_server_side(data, false);
        }
    }

    fn destroyed(
        state: &mut Self,
        client: ClientId,
        resource: &ZxdgToplevelDecorationV1,
        data: &ToplevelSurface,
    ) {
        <XdgDecorationState as Dispatch<ZxdgToplevelDecorationV1, ToplevelSurface, Self>>::destroyed(
            state, client, resource, data,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_icons_cover_every_edge_and_corner() {
        let e = |l, t, r, b| Edges {
            left: l,
            right: r,
            top: t,
            bottom: b,
        };
        assert_eq!(
            resize_icon(e(true, true, false, false)),
            CursorIcon::NwResize
        );
        assert_eq!(
            resize_icon(e(false, true, true, false)),
            CursorIcon::NeResize
        );
        assert_eq!(
            resize_icon(e(true, false, false, true)),
            CursorIcon::SwResize
        );
        assert_eq!(
            resize_icon(e(false, false, true, true)),
            CursorIcon::SeResize
        );
        assert_eq!(
            resize_icon(e(true, false, false, false)),
            CursorIcon::WResize
        );
        assert_eq!(
            resize_icon(e(false, false, true, false)),
            CursorIcon::EResize
        );
        assert_eq!(
            resize_icon(e(false, true, false, false)),
            CursorIcon::NResize
        );
        assert_eq!(
            resize_icon(e(false, false, false, true)),
            CursorIcon::SResize
        );
    }

    const OUTER: Rect = Rect::new(0, 0, 300, 160);

    fn look(focused: bool) -> Look {
        Look {
            focused,
            rounded: true,
            dim: true,
            ..Look::default()
        }
    }

    /// Bottom to top: the dim (unfocused only), the bar, glyphs, the outline.
    #[test]
    fn pieces_in_order_from_the_theme() {
        let t = &Theme::DARK;
        let focused = pieces(t, OUTER, Decoration::FULL, look(true));
        assert!(matches!(focused[0], Piece::ShapeFill(_, c) if c == t.chrome));
        let ring = sash_theme::over(t.outline_focused, t.chrome);
        assert!(matches!(focused.last(), Some(Piece::Ring(Edges::ALL, c)) if *c == ring));
        let glyphs: Vec<Color> = focused
            .iter()
            .filter_map(|p| match p {
                Piece::Glyph(_, _, ink) => Some(*ink),
                _ => None,
            })
            .collect();
        assert_eq!(glyphs, [t.text, t.text]);
        let unfocused = pieces(t, OUTER, Decoration::FULL, look(false));
        let inner = Decoration::FULL.inner(OUTER);
        assert_eq!(
            unfocused[0],
            Piece::ShapeFill(inner, t.dim),
            "the dim first"
        );
        assert!(matches!(unfocused[1], Piece::ShapeFill(_, c) if c == t.unfocused_bar));
        let ring = sash_theme::over(t.outline, t.unfocused_bar);
        assert!(matches!(unfocused.last(), Some(Piece::Ring(_, c)) if *c == ring));
        assert!(
            unfocused
                .iter()
                .any(|p| matches!(p, Piece::Glyph(_, _, c) if *c == t.secondary))
        );
    }

    /// The bar reaches under the outline (the outline is drawn on it) and never under the
    /// client; the dim is the client area exactly.
    #[test]
    fn the_bar_band_and_the_dim() {
        let t = &Theme::LIGHT;
        let p = pieces(t, OUTER, Decoration::FULL, look(false));
        assert_eq!(p[0], Piece::ShapeFill(Rect::new(1, 35, 298, 124), t.dim));
        assert_eq!(
            p[1],
            Piece::ShapeFill(Rect::new(0, 0, 300, 35), t.unfocused_bar)
        );
        let off = Look {
            dim: false,
            ..look(false)
        };
        assert!(!pieces(t, OUTER, Decoration::FULL, off).contains(&p[0]));
    }

    #[test]
    fn hover_squares_and_hovered_glyphs() {
        let t = &Theme::DARK;
        for (button, square) in [
            (Button::Close, t.close_hover),
            (Button::Maximize, t.overlay(Overlay::Strong)),
        ] {
            for focused in [true, false] {
                let p = pieces(
                    t,
                    OUTER,
                    Decoration::FULL,
                    Look {
                        hover: Some(button),
                        ..look(focused)
                    },
                );
                let area = Decoration::FULL.button_rect(OUTER, button).unwrap();
                assert!(p.contains(&Piece::Square(area, square)), "{button:?}");
                // The hovered glyph is the text token (secondary fails 3:1 on red).
                assert!(
                    p.iter()
                        .any(|q| matches!(q, Piece::Glyph(_, a, c) if *a == area && *c == t.text))
                );
            }
        }
        let max = pieces(
            t,
            OUTER,
            Decoration::FULL,
            Look {
                maximized: true,
                ..look(true)
            },
        );
        assert!(
            max.iter()
                .any(|p| matches!(p, Piece::Glyph(Glyph::Restore, ..)))
        );
    }

    #[test]
    fn tiles_get_the_outline_and_fullscreen_nothing() {
        let t = &Theme::DARK;
        let square = Look {
            rounded: false,
            ..look(true)
        };
        let border = pieces(t, OUTER, Decoration::BORDER, square);
        assert_eq!(border.len(), 4);
        assert!(
            border
                .iter()
                .all(|p| matches!(p, Piece::Outline(_, c) if *c == sash_theme::over(t.outline_focused, t.chrome)))
        );
        assert!(pieces(t, OUTER, Decoration::NONE, look(false)).is_empty());
        // A window too small for its glyphs still draws its bar, without glyphs.
        let tiny = pieces(t, Rect::new(0, 0, 10, 40), Decoration::FULL, square);
        assert!(
            tiny.iter()
                .all(|p| matches!(p, Piece::Fill(..) | Piece::Outline(..)))
        );
    }
}
