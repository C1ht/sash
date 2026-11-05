// SPDX-License-Identifier: Apache-2.0

//! What to draw for an output. Tiled windows at their tile, clipped to it; maximized,
//! floating and fullscreen windows unclipped above them; each window's popups directly
//! above it, unclipped; the snap preview just below
//! the dragged window; an opaque backdrop behind a fixed-size fullscreen window.
//! Shared by all backends.

use smithay::{
    backend::allocator::Fourcc,
    backend::renderer::{
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            render_elements,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::CropRenderElement,
        },
        gles::{GlesRenderer, element::PixelShaderElement},
    },
    input::pointer::CursorImageStatus,
    utils::{Point, Rectangle, Scale, Transform},
};

use crate::{
    layout::{Layer, Rect, layers::Level},
    popups::popups_of,
    rounded::MaskedSurface,
    state::Sash,
    windows::Visible,
};

/// Background where no window is.
pub const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
/// The accent blue #0A84FF.
const ACCENT: [f32; 4] = [10.0 / 255.0, 132.0 / 255.0, 1.0, 1.0];
const PREVIEW_FILL_ALPHA: f32 = 0.3;
const PREVIEW_BORDER_ALPHA: f32 = 0.8;
const PREVIEW_BORDER: i32 = 2;
const BACKDROP: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

render_elements! {
    pub OutputElement<=GlesRenderer>;
    /// A tiled window's surface: masked by its shape, clipped to its tile.
    Tiled=CropRenderElement<MaskedSurface>,
    /// A floating, maximized or fullscreen window's surface, masked by its shape.
    Masked=MaskedSurface,
    /// Popups, layer surfaces, the drag icon and client cursors: never masked.
    Window=WaylandSurfaceRenderElement<GlesRenderer>,
    Solid=SolidColorRenderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    /// Shape-clipped fills and the border ring (rounded.rs).
    Shader=PixelShaderElement,
}

/// Size of the built-in arrow cursor (drawn by the DRM backend for named shapes until
/// cursor themes exist).
const ARROW: (i32, i32) = (16, 24);

/// The built-in arrow: a black outline, white inside, hotspot at its tip (0, 0).
/// ARGB8888 pixels, generated rather than shipped as an image.
pub fn arrow_pixels() -> Vec<u8> {
    let (w, h) = ARROW;
    // Inside the arrow: a head that widens one pixel per row and tapers back from row
    // 10, and a stem going down and to the right from the head.
    let inside = |x: i32, y: i32| {
        let head = (0..16).contains(&y) && x >= 0 && x <= y.min(15 - (y - 10).max(0) * 2);
        let stem_start = 4 + (y - 11) / 2;
        let stem = (11..h).contains(&y) && (stem_start..stem_start + 3).contains(&x);
        head || stem
    };
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let edge = inside(x, y)
                && [(-1, 0), (1, 0), (0, -1), (0, 1)]
                    .iter()
                    .any(|(dx, dy)| !inside(x + dx, y + dy));
            let argb: u32 = match (inside(x, y), edge) {
                (true, true) => 0xff00_0000,
                (true, false) => 0xffff_ffff,
                _ => 0,
            };
            pixels.extend_from_slice(&argb.to_le_bytes());
        }
    }
    pixels
}

/// The built-in arrow as a buffer the renderer can draw.
pub fn arrow_buffer() -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(
        &arrow_pixels(),
        Fourcc::Argb8888,
        ARROW,
        1,
        Transform::Normal,
        None,
    )
}

/// A named cursor shape at the pointer (DRM backend): the theme's image (the current
/// frame of an animation) at the pointer minus its hotspot, or the built-in arrow when
/// the theme has none.
pub fn arrow_element(
    state: &Sash,
    renderer: &mut GlesRenderer,
    arrow: &MemoryRenderBuffer,
) -> Option<OutputElement> {
    if !matches!(state.cursor.image(), CursorImageStatus::Named(_)) {
        return None;
    }
    let location = state.seat.get_pointer()?.current_location();
    let themed = state.themed_cursor();
    let (buffer, at) = match &themed {
        Some(image) => {
            let (index, _) = image.frame_at(state.cursor.shown_for());
            let frame = &image.frames[index];
            let at = (
                location.x - frame.hotspot.0 as f64,
                location.y - frame.hotspot.1 as f64,
            );
            (&frame.buffer, at)
        }
        None => (arrow, (location.x, location.y)),
    };
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        at,
        buffer,
        None,
        None,
        None,
        Kind::Cursor,
    )
    .ok()
    .map(OutputElement::Memory)
}

/// Compositor-drawn rectangles. The buffers persist so damage tracking sees them as the
/// same elements from frame to frame.
pub struct Overlay {
    preview: Option<Rect>,
    /// Fill (inside the border), then the top, bottom, left and right border strips.
    pieces: [(SolidColorBuffer, Rect); 5],
    backdrop: Option<(SolidColorBuffer, Rect)>,
}

impl Overlay {
    pub fn new() -> Self {
        let piece = || (SolidColorBuffer::new((0, 0), ACCENT), Rect::default());
        Self {
            preview: None,
            pieces: [piece(), piece(), piece(), piece(), piece()],
            backdrop: None,
        }
    }

    /// Shows the snap preview over `rect`, or hides it.
    pub fn set_preview(&mut self, rect: Option<Rect>) {
        if self.preview == rect {
            return;
        }
        self.preview = rect;
        let Some(r) = rect else {
            return;
        };
        let b = PREVIEW_BORDER.min(r.w / 2).min(r.h / 2).max(0);
        let inner_h = (r.h - 2 * b).max(0);
        let rects = [
            Rect::new(r.x + b, r.y + b, (r.w - 2 * b).max(0), inner_h),
            Rect::new(r.x, r.y, r.w, b),
            Rect::new(r.x, r.bottom() - b, r.w, b),
            Rect::new(r.x, r.y + b, b, inner_h),
            Rect::new(r.right() - b, r.y + b, b, inner_h),
        ];
        for ((buffer, at), rect) in self.pieces.iter_mut().zip(rects) {
            buffer.resize((rect.w, rect.h));
            *at = rect;
        }
    }

    #[cfg(test)]
    pub fn preview(&self) -> Option<Rect> {
        self.preview
    }

    /// Shows the opaque backdrop over `rect`, or hides it.
    pub fn set_backdrop(&mut self, rect: Option<Rect>) {
        match (rect, &mut self.backdrop) {
            (None, backdrop) => *backdrop = None,
            (Some(r), Some((buffer, at))) => {
                buffer.resize((r.w, r.h));
                *at = r;
            }
            (Some(r), backdrop @ None) => {
                *backdrop = Some((SolidColorBuffer::new((r.w, r.h), BACKDROP), r))
            }
        }
    }

    fn element(
        buffer: &SolidColorBuffer,
        at: Rect,
        scale: Scale<f64>,
        alpha: f32,
    ) -> SolidColorRenderElement {
        let location = Point::from((at.x, at.y)).to_physical_precise_round(scale);
        SolidColorRenderElement::from_buffer(buffer, location, scale, alpha, Kind::Unspecified)
    }

    fn preview_elements(
        &self,
        scale: Scale<f64>,
    ) -> impl Iterator<Item = SolidColorRenderElement> + '_ {
        self.preview.into_iter().flat_map(move |_| {
            self.pieces
                .iter()
                .enumerate()
                .map(move |(i, (buffer, at))| {
                    let alpha = if i == 0 {
                        PREVIEW_FILL_ALPHA
                    } else {
                        PREVIEW_BORDER_ALPHA
                    };
                    Self::element(buffer, *at, scale, alpha)
                })
        })
    }

    fn backdrop_element(&self, scale: Scale<f64>) -> Option<SolidColorRenderElement> {
        self.backdrop
            .as_ref()
            .map(|(buffer, at)| Self::element(buffer, *at, scale, 1.0))
    }
}

/// The cursor surface sash-wm draws, if the cursor is one: on top of everything.
/// Not part of `output_elements`, so offscreen captures show no cursor.
pub fn cursor_elements(
    state: &Sash,
    renderer: &mut GlesRenderer,
    scale: f64,
) -> Vec<OutputElement> {
    let Some((surface, origin)) = state.cursor_surface() else {
        return Vec::new();
    };
    let scale = Scale::from(scale);
    let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(
            renderer,
            &surface,
            origin.to_physical_precise_round(scale),
            scale,
            1.0,
            Kind::Cursor,
        );
    surfaces.into_iter().map(OutputElement::Window).collect()
}

/// Render elements of the layer surfaces of `level` and their popups, top first.
fn layer_elements(
    state: &Sash,
    renderer: &mut GlesRenderer,
    level: Level,
    scale: Scale<f64>,
) -> Vec<OutputElement> {
    let mut out = Vec::new();
    for (surface, origin) in state.layers_to_draw(level) {
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            render_elements_from_surface_tree(
                renderer,
                &surface,
                origin.to_physical_precise_round(scale),
                scale,
                1.0,
                Kind::Unspecified,
            );
        out.extend(surfaces.into_iter().map(OutputElement::Window));
    }
    out
}

/// Render elements for the output at the origin with the given scale, front to back
/// (the first element is drawn on top). Order (layout::layers::STACKING): drag icon,
/// overlay layer, a fullscreen window, top layer, the other windows (snap preview
/// among them), bottom layer, background layer; each layer's popups above it.
pub fn output_elements(
    state: &Sash,
    renderer: &mut GlesRenderer,
    scale: f64,
) -> Vec<OutputElement> {
    let scale = Scale::from(scale);
    let dragged = state.dragged_window();
    let mut elements = Vec::new();
    // A drag-and-drop icon is above everything: windows, popups and overlays.
    if let Some((icon, origin)) = state.drag_icon() {
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            render_elements_from_surface_tree(
                renderer,
                &icon,
                origin.to_physical_precise_round(scale),
                scale,
                1.0,
                Kind::Unspecified,
            );
        elements.extend(surfaces.into_iter().map(OutputElement::Window));
    }
    elements.extend(layer_elements(state, renderer, Level::Overlay, scale));
    // The top layer goes below a fullscreen window and above every other window.
    let mut top_drawn = false;
    let mut windows_start = elements.len();
    let mut preview_drawn = false;
    let visible_windows = state.visible_windows();
    let drawn: Vec<_> = visible_windows.iter().map(|v| v.id).collect();
    state.decorations.retain(&drawn);
    let popup_elements = |renderer: &mut GlesRenderer, visible: &Visible<'_>| {
        let mut out = Vec::new();
        for (popup, origin) in popups_of(visible.window, visible.rect) {
            let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    origin.to_physical_precise_round(scale),
                    scale,
                    1.0,
                    Kind::Unspecified,
                );
            out.extend(surfaces.into_iter().map(OutputElement::Window));
        }
        out
    };
    // Popups go directly above their window, never clipped to its tile; tiled windows
    // have no order among themselves, so their popups go above all of them.
    let mut tiled_popups_drawn = false;
    for visible in visible_windows.iter().rev() {
        if !top_drawn && visible.layer != Layer::Fullscreen {
            top_drawn = true;
            elements.extend(layer_elements(state, renderer, Level::Top, scale));
            windows_start = elements.len();
        }
        if visible.layer != Layer::Tiled {
            elements.extend(popup_elements(renderer, visible));
        } else if !tiled_popups_drawn {
            tiled_popups_drawn = true;
            for tiled in visible_windows
                .iter()
                .rev()
                .filter(|v| v.layer == Layer::Tiled)
            {
                elements.extend(popup_elements(renderer, tiled));
            }
        }
        // Windows with server-side decorations are drawn through their rounded shape
        // (M15b step 2), the toplevel and its subsurfaces alike.
        let mask = state
            .windows
            .layout
            .shape(visible.id, scale.x as f32)
            .and_then(|shape| crate::rounded::programs(renderer).map(|p| (p.mask, shape)));
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            match visible.window.toplevel() {
                Some(toplevel) => render_elements_from_surface_tree(
                    renderer,
                    toplevel.wl_surface(),
                    visible.position.to_physical_precise_round(scale),
                    scale,
                    1.0,
                    Kind::Unspecified,
                ),
                None => Vec::new(),
            };
        // Bar and border above the client's surfaces (a client's shadow stays below).
        elements.extend(crate::decorations::decoration_elements(
            state,
            renderer,
            visible.id,
            visible.outer,
            visible.decoration,
            scale,
        ));
        match visible.layer {
            Layer::Tiled => {
                let r = visible.rect;
                let clip = Rectangle::new((r.x, r.y).into(), (r.w, r.h).into())
                    .to_physical_precise_round(scale);
                elements.extend(
                    surfaces
                        .into_iter()
                        .map(|surface| MaskedSurface::new(surface, mask.clone()))
                        .filter_map(|surface| CropRenderElement::from_element(surface, scale, clip))
                        .map(OutputElement::Tiled),
                );
            }
            Layer::Maximized | Layer::Floating | Layer::Fullscreen => {
                elements.extend(surfaces.into_iter().map(|surface| {
                    OutputElement::Masked(MaskedSurface::new(surface, mask.clone()))
                }));
            }
        }
        if visible.layer == Layer::Fullscreen {
            elements.extend(
                state
                    .overlay
                    .backdrop_element(scale)
                    .map(OutputElement::Solid),
            );
        }
        // The preview goes just below the dragged window, above everything else.
        if Some(visible.id) == dragged {
            elements.extend(
                state
                    .overlay
                    .preview_elements(scale)
                    .map(OutputElement::Solid),
            );
            preview_drawn = true;
        }
    }
    if !top_drawn {
        elements.extend(layer_elements(state, renderer, Level::Top, scale));
        windows_start = elements.len();
    }
    if !preview_drawn {
        // Above the windows, below the top layer.
        let preview: Vec<_> = state
            .overlay
            .preview_elements(scale)
            .map(OutputElement::Solid)
            .collect();
        elements.splice(windows_start..windows_start, preview);
    }
    elements.extend(layer_elements(state, renderer, Level::Bottom, scale));
    elements.extend(layer_elements(state, renderer, Level::Background, scale));
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrow_has_an_outline_and_its_tip_at_the_origin() {
        let pixels = arrow_pixels();
        let (w, h) = ARROW;
        let at = |x: i32, y: i32| {
            let i = ((y * w + x) * 4) as usize;
            u32::from_le_bytes(pixels[i..i + 4].try_into().unwrap())
        };
        let mut picture = String::new();
        for y in 0..h {
            for x in 0..w {
                picture.push(match at(x, y) {
                    0xff00_0000 => '#',
                    0xffff_ffff => '.',
                    _ => ' ',
                });
            }
            picture.push('\n');
        }
        println!("{picture}");
        assert_eq!(at(0, 0), 0xff00_0000, "the tip is the hotspot");
        assert_eq!(at(w - 1, 0), 0, "transparent around it");
        assert!(
            pixels.chunks(4).any(|p| p == [0xff, 0xff, 0xff, 0xff]),
            "white inside"
        );
    }
}
