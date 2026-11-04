// SPDX-License-Identifier: Apache-2.0

//! Server-side decorations (M11b): the xdg-decoration protocol (every client that
//! negotiates is answered server_side), the pointer on the bar, buttons and resize
//! zones, and what is drawn for them. The geometry is in `layout::decor`.

use std::{cell::RefCell, collections::HashMap};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportAll, ImportMem, Renderer, Texture,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::{SolidColorBuffer, SolidColorRenderElement},
            },
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
            delegate_global_dispatch,
        },
    },
    utils::{Logical, Point, Scale, Serial, Transform},
    wayland::shell::xdg::{
        ToplevelSurface,
        decoration::{XdgDecorationHandler, XdgDecorationManagerGlobalData, XdgDecorationState},
    },
};

use crate::{
    layout::{
        Edges, Rect, WindowId,
        decor::{BUTTONS, Button, Decoration, GLYPH, Part},
    },
    render::OutputElement,
    seat::BTN_LEFT,
    state::Sash,
    windows::DragKind,
};

/// Colours (dark values until the appearance setting exists).
const BAR_FILL: [f32; 4] = rgb(0x2C, 0x2C, 0x2E);
const BORDER_FOCUSED: [f32; 4] = rgb(0x0A, 0x84, 0xFF);
const BORDER_UNFOCUSED: [f32; 4] = rgb(0x3A, 0x3A, 0x3C);
const GLYPH_FOCUSED: f32 = 0.92;
const GLYPH_UNFOCUSED: f32 = 0.35;
const HOVER_CLOSE: [f32; 4] = rgb(0xFF, 0x45, 0x3A);
const HOVER_MAXIMIZE: [f32; 4] = rgb(0x3A, 0x3A, 0x3C);
/// The filled square a hovered button shows behind its glyph.
const HOVER_SIZE: i32 = 24;

const fn rgb(r: u8, g: u8, b: u8) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

/// A glyph, drawn from simple shapes (no font).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Close,
    Maximize,
    /// The maximize button of a maximized window: two overlapping squares.
    Restore,
}

/// One thing drawn for a window's decorations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Fill(Rect, [f32; 4]),
    /// A white glyph at this alpha.
    Glyph(Glyph, Rect, f32),
}

/// What is drawn for a window's decorations, bottom first: the border, the bar, each
/// button's hover square and its glyph. Focus shows in the border colour and the
/// glyphs' brightness together; hover adds a filled square (a fill, not only colour).
pub fn pieces(
    outer: Rect,
    decoration: Decoration,
    focused: bool,
    hover: Option<Button>,
    maximized: bool,
) -> Vec<Piece> {
    let mut out = Vec::new();
    let edges = decoration.border;
    if edges != Edges::NONE {
        let color = if focused {
            BORDER_FOCUSED
        } else {
            BORDER_UNFOCUSED
        };
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
            out.push(Piece::Fill(rect, color));
        }
    }
    let Some(bar) = decoration.bar_rect(outer) else {
        return out;
    };
    out.push(Piece::Fill(bar, BAR_FILL));
    for button in BUTTONS {
        let Some(area) = decoration.button_rect(outer, button) else {
            continue;
        };
        let hovered = hover == Some(button);
        if hovered {
            let color = match button {
                Button::Close => HOVER_CLOSE,
                Button::Maximize => HOVER_MAXIMIZE,
            };
            out.push(Piece::Fill(centered(area, HOVER_SIZE), color));
        }
        if area.w >= GLYPH && area.h >= GLYPH {
            let glyph = match button {
                Button::Close => Glyph::Close,
                Button::Maximize if maximized => Glyph::Restore,
                Button::Maximize => Glyph::Maximize,
            };
            let alpha = if focused || hovered {
                GLYPH_FOCUSED
            } else {
                GLYPH_UNFOCUSED
            };
            out.push(Piece::Glyph(glyph, centered(area, GLYPH), alpha));
        }
    }
    out
}

/// A size x size square centered in `area`, clipped to it.
fn centered(area: Rect, size: i32) -> Rect {
    let w = size.min(area.w);
    let h = size.min(area.h);
    Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h)
}

/// A glyph's GLYPH x GLYPH mask, row by row.
pub fn glyph_mask(glyph: Glyph) -> Vec<bool> {
    let n = GLYPH;
    let outline = |x: i32, y: i32, (x0, y0, x1, y1): (i32, i32, i32, i32)| {
        let inside = (x0..=x1).contains(&x) && (y0..=y1).contains(&y);
        // A 1 px outline with a 2 px top edge (a window's title).
        inside && (x == x0 || x == x1 || y == y0 || y == y0 + 1 || y == y1)
    };
    let inside = |x: i32, y: i32, (x0, y0, x1, y1): (i32, i32, i32, i32)| {
        (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
    };
    let mut mask = Vec::with_capacity((n * n) as usize);
    for y in 0..n {
        for x in 0..n {
            mask.push(match glyph {
                // Two diagonals about 2 px wide.
                Glyph::Close => (x - y).abs() <= 1 || (x + y - (n - 1)).abs() <= 1,
                Glyph::Maximize => outline(x, y, (1, 1, n - 2, n - 2)),
                Glyph::Restore => {
                    let front = (1, 4, n - 5, n - 2);
                    let back = (4, 1, n - 2, n - 5);
                    outline(x, y, front) || (outline(x, y, back) && !inside(x, y, front))
                }
            });
        }
    }
    mask
}

fn glyph_buffer(glyph: Glyph) -> MemoryRenderBuffer {
    let pixels: Vec<u8> = glyph_mask(glyph)
        .into_iter()
        .flat_map(|on| {
            let argb: u32 = if on { 0xffff_ffff } else { 0 };
            argb.to_le_bytes()
        })
        .collect();
    MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Argb8888,
        (GLYPH, GLYPH),
        1,
        Transform::Normal,
        None,
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
type Fill = (SolidColorBuffer, Rect, [f32; 4]);

/// The pointer on decorations, and the buffers drawn for them (kept from frame to
/// frame so damage tracking sees unchanged decorations as unchanged).
pub struct Decorations {
    /// The decoration under the pointer.
    pub hover: Option<(WindowId, Part)>,
    /// A button pressed and not released yet: it acts on release over it.
    pressed: Option<(WindowId, Button, u32)>,
    fills: RefCell<HashMap<(WindowId, usize), Fill>>,
    glyphs: [MemoryRenderBuffer; 3],
}

impl Decorations {
    pub fn new() -> Self {
        Self {
            hover: None,
            pressed: None,
            fills: RefCell::new(HashMap::new()),
            glyphs: [Glyph::Close, Glyph::Maximize, Glyph::Restore].map(glyph_buffer),
        }
    }

    fn glyph(&self, glyph: Glyph) -> &MemoryRenderBuffer {
        &self.glyphs[match glyph {
            Glyph::Close => 0,
            Glyph::Maximize => 1,
            Glyph::Restore => 2,
        }]
    }

    /// Forgets the buffers of windows that are not drawn any more.
    pub fn retain(&self, drawn: &[WindowId]) {
        self.fills
            .borrow_mut()
            .retain(|(window, _), _| drawn.contains(window));
    }
}

/// Render elements for one window's decorations, front to back.
pub fn decoration_elements<R>(
    state: &Sash,
    renderer: &mut R,
    id: WindowId,
    outer: Rect,
    decoration: Decoration,
    scale: Scale<f64>,
) -> Vec<OutputElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Send + Clone + 'static,
{
    let layout = &state.windows.layout;
    let hover = match state.decorations.hover {
        Some((window, Part::Button(button))) if window == id => Some(button),
        _ => None,
    };
    let pieces = pieces(
        outer,
        decoration,
        layout.focused() == Some(id),
        hover,
        layout.is_maximized(id),
    );
    let decorations = &state.decorations;
    let mut fills = decorations.fills.borrow_mut();
    let mut out = Vec::with_capacity(pieces.len());
    for (index, piece) in pieces.into_iter().enumerate() {
        match piece {
            Piece::Fill(rect, color) => {
                let (buffer, at, was) = fills.entry((id, index)).or_insert_with(|| {
                    (SolidColorBuffer::new((rect.w, rect.h), color), rect, color)
                });
                if (*at, *was) != (rect, color) {
                    buffer.update((rect.w, rect.h), color);
                    (*at, *was) = (rect, color);
                }
                let location =
                    Point::<i32, Logical>::from((rect.x, rect.y)).to_physical_precise_round(scale);
                out.push(OutputElement::Solid(SolidColorRenderElement::from_buffer(
                    buffer,
                    location,
                    scale,
                    1.0,
                    Kind::Unspecified,
                )));
            }
            Piece::Glyph(glyph, rect, alpha) => {
                let location = Point::<i32, Logical>::from((rect.x, rect.y))
                    .to_f64()
                    .to_physical(scale);
                if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    location,
                    decorations.glyph(glyph),
                    Some(alpha),
                    None,
                    None,
                    Kind::Unspecified,
                ) {
                    out.push(OutputElement::Memory(element));
                }
            }
        }
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

delegate_global_dispatch!(Sash: [ZxdgDecorationManagerV1: XdgDecorationManagerGlobalData] => XdgDecorationState);
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

    fn picture(glyph: Glyph) -> String {
        glyph_mask(glyph)
            .chunks(GLYPH as usize)
            .map(|row| {
                row.iter()
                    .map(|on| if *on { '#' } else { '.' })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn glyphs_are_distinct_shapes() {
        let close = picture(Glyph::Close);
        let maximize = picture(Glyph::Maximize);
        let restore = picture(Glyph::Restore);
        println!("{close}\n\n{maximize}\n\n{restore}");
        assert_ne!(close, maximize);
        assert_ne!(maximize, restore);
        assert_ne!(close, restore);
        // The close glyph is symmetric under both reflections.
        let mask = glyph_mask(Glyph::Close);
        let n = GLYPH as usize;
        for y in 0..n {
            for x in 0..n {
                assert_eq!(mask[y * n + x], mask[y * n + (n - 1 - x)]);
                assert_eq!(mask[y * n + x], mask[x * n + y]);
            }
        }
    }

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

    /// Greyscale picture of the decorations (luminance per pixel, white glyphs blended
    /// over what is below): focus and hover must show without colour.
    fn grey(pieces: &[Piece], outer: Rect) -> Vec<u8> {
        let lum = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let mut out = vec![0.0f32; (outer.w * outer.h) as usize];
        for piece in pieces {
            match *piece {
                Piece::Fill(r, color) => {
                    for y in r.y..r.bottom() {
                        for x in r.x..r.right() {
                            out[((y - outer.y) * outer.w + x - outer.x) as usize] = lum(color);
                        }
                    }
                }
                Piece::Glyph(glyph, r, alpha) => {
                    let mask = glyph_mask(glyph);
                    for (i, on) in mask.iter().enumerate() {
                        let (x, y) = (r.x + i as i32 % GLYPH, r.y + i as i32 / GLYPH);
                        if *on {
                            let p = &mut out[((y - outer.y) * outer.w + x - outer.x) as usize];
                            *p = alpha + (1.0 - alpha) * *p;
                        }
                    }
                }
            }
        }
        out.into_iter().map(|v| (v * 255.0).round() as u8).collect()
    }

    #[test]
    fn focus_and_hover_show_without_colour() {
        let outer = Rect::new(0, 0, 200, 120);
        let d = Decoration::FULL;
        let focused = grey(&pieces(outer, d, true, None, false), outer);
        let unfocused = grey(&pieces(outer, d, false, None, false), outer);
        // The border and the glyphs both change brightness.
        let border = |g: &[u8]| g[(60 * 200) as usize];
        assert!(
            border(&focused).abs_diff(border(&unfocused)) > 30,
            "border brightness"
        );
        let glyph_peak = |g: &[u8]| {
            (1..33)
                .flat_map(|y| (1..65).map(move |x| (x, y)))
                .map(|(x, y)| g[(y * 200 + x) as usize])
                .max()
                .unwrap()
        };
        assert!(
            glyph_peak(&focused) > glyph_peak(&unfocused) + 80,
            "glyph brightness"
        );
        // Hover adds a filled square: more pixels change than the glyph alone.
        for button in BUTTONS {
            let hovered = grey(&pieces(outer, d, true, Some(button), false), outer);
            let changed = hovered
                .iter()
                .zip(&focused)
                .filter(|(a, b)| a.abs_diff(**b) > 4)
                .count();
            assert!(changed > 200, "{button:?} hover changes {changed} pixels");
        }
        // Maximized shows the restore glyph.
        let maximized = grey(&pieces(outer, d, true, None, true), outer);
        assert_ne!(maximized, focused);
    }

    #[test]
    fn tiles_get_the_border_only_and_fullscreen_nothing() {
        let outer = Rect::new(0, 0, 100, 80);
        let border = pieces(outer, Decoration::BORDER, true, None, false);
        assert_eq!(border.len(), 4);
        assert!(
            border
                .iter()
                .all(|p| matches!(p, Piece::Fill(_, c) if *c == BORDER_FOCUSED))
        );
        assert!(pieces(outer, Decoration::NONE, true, None, false).is_empty());
        // A window too small for its glyphs still draws its bar, without glyphs.
        let tiny = pieces(Rect::new(0, 0, 10, 40), Decoration::FULL, true, None, false);
        assert!(tiny.iter().all(|p| matches!(p, Piece::Fill(..))));
    }
}
