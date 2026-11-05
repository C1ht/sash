// SPDX-License-Identifier: Apache-2.0

//! The default debug screenshot: window outlines with their ids, drawn from the layout
//! alone (no renderer, no window contents). Every pixel is one of `PALETTE`, so a test
//! can prove that nothing a client drew is in the picture.

use crate::layout::{Layer, Rect, WindowId, decor::Decoration};

pub type Rgba = [u8; 4];

pub const BACKGROUND: Rgba = [26, 26, 26, 255];
pub const TILED_FILL: Rgba = [52, 56, 64, 255];
pub const FLOATING_FILL: Rgba = [72, 62, 48, 255];
pub const BORDER: Rgba = [190, 190, 190, 255];
pub const FOCUSED_BORDER: Rgba = [10, 132, 255, 255];
pub const TEXT: Rgba = [255, 255, 255, 255];
pub const POINTER: Rgba = [255, 64, 64, 255];
/// The outlines of the bar band and the client's rectangle inside the border (M11b).
pub const DECORATION: Rgba = [140, 140, 140, 255];

/// Every colour an outline picture can contain.
#[cfg(test)]
pub const PALETTE: [Rgba; 8] = [
    DECORATION,
    BACKGROUND,
    TILED_FILL,
    FLOATING_FILL,
    BORDER,
    FOCUSED_BORDER,
    TEXT,
    POINTER,
];

/// 3x5 digits, one row per byte, bits 2..0 from left to right.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// One window to draw, bottom to top.
pub struct Outline {
    pub id: WindowId,
    pub rect: Rect,
    pub layer: Layer,
    pub focused: bool,
    /// The server-side decorations: their bar band and border show as outlines.
    pub decoration: Decoration,
}

/// An RGBA canvas `width` x `height` showing an output of `output` (w, h) pixels,
/// scaled down by a whole factor so the width is at most `max_width`.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    scale: usize,
    pub pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(output: (usize, usize), max_width: usize) -> Self {
        let scale = output.0.div_ceil(max_width).max(1);
        let (width, height) = ((output.0 / scale).max(1), (output.1 / scale).max(1));
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            pixels.extend_from_slice(&BACKGROUND);
        }
        Self {
            width,
            height,
            scale,
            pixels,
        }
    }

    /// Fills output-space rectangle (x, y, w, h), clipped to the canvas.
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, colour: Rgba) {
        let s = self.scale as i32;
        let (x0, y0) = (x.div_euclid(s).max(0), y.div_euclid(s).max(0));
        let x1 = ((x + w) / s).min(self.width as i32);
        let y1 = ((y + h) / s).min(self.height as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                let i = (py as usize * self.width + px as usize) * 4;
                self.pixels[i..i + 4].copy_from_slice(&colour);
            }
        }
    }

    /// Fills a rectangle in canvas pixels.
    fn fill_canvas(&mut self, x: i32, y: i32, w: i32, h: i32, colour: Rgba) {
        let s = self.scale as i32;
        self.fill(x * s, y * s, w * s, h * s, colour);
    }

    /// A window: its fill, a 2-pixel border (canvas pixels) and its id.
    pub fn window(&mut self, outline: &Outline) {
        let r = outline.rect;
        let fill = match outline.layer {
            Layer::Tiled => TILED_FILL,
            _ => FLOATING_FILL,
        };
        let border = if outline.focused {
            FOCUSED_BORDER
        } else {
            BORDER
        };
        self.fill(r.x, r.y, r.w, r.h, fill);
        // Decorations: the bar band and the client's rectangle, 1 canvas pixel each
        // (below the window's own outline, which stays visible where they meet).
        if outline.decoration != Decoration::NONE {
            if let Some(bar) = outline.decoration.bar_rect(r) {
                self.outline(bar, DECORATION);
            }
            self.outline(outline.decoration.inner(r), DECORATION);
        }
        let s = self.scale as i32;
        let b = 2 * s;
        self.fill(r.x, r.y, r.w, b, border);
        self.fill(r.x, r.y + r.h - b, r.w, b, border);
        self.fill(r.x, r.y, b, r.h, border);
        self.fill(r.x + r.w - b, r.y, b, r.h, border);
        // The id, 4 canvas pixels per font dot, inside the top-left corner.
        let (cx, cy) = ((r.x.max(0) / s) + 8, (r.y.max(0) / s) + 8);
        self.number(cx, cy, outline.id.0, 4);
    }

    /// A 1 canvas pixel outline of an output-space rectangle.
    fn outline(&mut self, r: Rect, colour: Rgba) {
        let s = self.scale as i32;
        self.fill(r.x, r.y, r.w, s, colour);
        self.fill(r.x, r.y + r.h - s, r.w, s, colour);
        self.fill(r.x, r.y, s, r.h, colour);
        self.fill(r.x + r.w - s, r.y, s, r.h, colour);
    }

    fn number(&mut self, x: i32, y: i32, n: u64, dot: i32) {
        for (i, ch) in n.to_string().bytes().enumerate() {
            let glyph = DIGITS[(ch - b'0') as usize];
            let left = x + i as i32 * 4 * dot;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..3 {
                    if bits >> (2 - col) & 1 == 1 {
                        self.fill_canvas(left + col * dot, y + row as i32 * dot, dot, dot, TEXT);
                    }
                }
            }
        }
    }

    /// The pointer: a small cross at an output-space position.
    pub fn pointer(&mut self, x: i32, y: i32) {
        let s = self.scale as i32;
        let (cx, cy) = (x.div_euclid(s), y.div_euclid(s));
        self.fill_canvas(cx - 6, cy - 1, 13, 3, POINTER);
        self.fill_canvas(cx - 1, cy - 6, 3, 13, POINTER);
    }
}

/// The outline picture of `windows` (bottom to top) on an output, as RGBA pixels.
pub fn draw(
    output: (usize, usize),
    windows: &[Outline],
    pointer: Option<(i32, i32)>,
    max_width: usize,
) -> Canvas {
    let mut canvas = Canvas::new(output, max_width);
    for window in windows {
        canvas.window(window);
    }
    if let Some((x, y)) = pointer {
        canvas.pointer(x, y);
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(c: &Canvas, x: usize, y: usize) -> Rgba {
        let i = (y * c.width + x) * 4;
        c.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn outlines_ids_and_pointer_only_in_the_palette() {
        let windows = [
            Outline {
                id: WindowId(1),
                rect: Rect::new(0, 0, 2560, 1440),
                layer: Layer::Tiled,
                focused: false,
                decoration: Decoration::BORDER,
            },
            Outline {
                id: WindowId(12),
                rect: Rect::new(2560, 0, 2560, 1440),
                layer: Layer::Tiled,
                focused: true,
                decoration: Decoration::BORDER,
            },
            Outline {
                id: WindowId(3),
                rect: Rect::new(1000, 300, 900, 600),
                layer: Layer::Floating,
                focused: false,
                decoration: Decoration::FULL,
            },
        ];
        let c = draw((5120, 1440), &windows, Some((4000, 700)), 1920);
        assert_eq!((c.width, c.height), (1706, 480), "5120 px scaled by 3");
        assert_eq!(c.pixels.len(), c.width * c.height * 4);
        for px in c.pixels.chunks(4) {
            assert!(PALETTE.contains(&px.try_into().unwrap()), "{px:?}");
        }
        // Borders, fills, the floating window above the tile, the focused border.
        assert_eq!(at(&c, 0, 0), BORDER);
        assert_eq!(at(&c, 300, 200), TILED_FILL);
        assert_eq!(at(&c, 500, 200), FLOATING_FILL, "drawn above the tile");
        assert_eq!(at(&c, 853, 0), FOCUSED_BORDER);
        assert_eq!(at(&c, 1333, 233), POINTER);
        // Window 1's id: "1" starts at canvas (8, 8); its top row is 010.
        assert_eq!(at(&c, 8 + 4, 8), TEXT);
        assert_eq!(at(&c, 8, 8), TILED_FILL);
        // Window 3's bar band (32 px at scale 3: canvas rows 100..111) and client
        // rectangle are outlined.
        assert_eq!(
            at(&c, 400, 100 + 32 / 3),
            DECORATION,
            "the bar band's lower edge"
        );
        assert_eq!(at(&c, 400, 111), DECORATION, "the client rectangle's top");
        assert_eq!(at(&c, 400, 120), FLOATING_FILL);
    }

    /// Gaps (M15b) show in the outline screenshot: between two tiles 8 px apart the
    /// background shows, drawn from the layout's visible rectangles.
    #[test]
    fn outline_screenshots_show_the_gaps() {
        use crate::layout::{MonitorLayout, NewWindow};
        let mut layout = MonitorLayout::new(Rect::new(0, 0, 800, 600));
        layout.open(WindowId(1), NewWindow::default());
        layout.open(WindowId(2), NewWindow::default());
        layout.set_gap(8);
        let windows: Vec<Outline> = layout
            .stacking()
            .into_iter()
            .map(|(id, rect, layer)| Outline {
                id,
                rect,
                layer,
                focused: false,
                decoration: layout.decoration(id),
            })
            .collect();
        let c = draw((800, 600), &windows, None, 1920);
        let at = |x: usize, y: usize| {
            let i = (y * c.width + x) * 4;
            [
                c.pixels[i],
                c.pixels[i + 1],
                c.pixels[i + 2],
                c.pixels[i + 3],
            ]
        };
        for x in 396..404 {
            assert_eq!(at(x, 300), BACKGROUND, "x {x}: the gap");
        }
        assert_ne!(at(395, 300), BACKGROUND, "tile 1's edge");
        assert_ne!(at(404, 300), BACKGROUND, "tile 2's edge");
    }
}
