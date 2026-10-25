// SPDX-License-Identifier: Apache-2.0

//! What to draw for an output. Tiled windows at their tile, clipped to it; maximized,
//! floating and fullscreen windows unclipped above them; each window's popups directly
//! above it, unclipped; the snap preview just below
//! the dragged window; an opaque backdrop behind a fixed-size fullscreen window.
//! Shared by all backends.

use smithay::{
    backend::renderer::{
        ImportAll, Renderer, Texture,
        element::{
            Kind, render_elements,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::CropRenderElement,
        },
    },
    utils::{Point, Rectangle, Scale},
};

use crate::{
    layout::{Layer, Rect},
    popups::popups_of,
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
    pub OutputElement<R> where R: ImportAll;
    Tiled=CropRenderElement<WaylandSurfaceRenderElement<R>>,
    Window=WaylandSurfaceRenderElement<R>,
    Solid=SolidColorRenderElement,
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
pub fn cursor_elements<R>(state: &Sash, renderer: &mut R, scale: f64) -> Vec<OutputElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let Some((surface, origin)) = state.cursor_surface() else {
        return Vec::new();
    };
    let scale = Scale::from(scale);
    let surfaces: Vec<WaylandSurfaceRenderElement<R>> = render_elements_from_surface_tree(
        renderer,
        &surface,
        origin.to_physical_precise_round(scale),
        scale,
        1.0,
        Kind::Cursor,
    );
    surfaces.into_iter().map(OutputElement::Window).collect()
}

/// Render elements for the output at the origin with the given scale, front to back
/// (the first element is drawn on top).
pub fn output_elements<R>(state: &Sash, renderer: &mut R, scale: f64) -> Vec<OutputElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let scale = Scale::from(scale);
    let dragged = state.dragged_window();
    let mut elements = Vec::new();
    let mut preview_drawn = false;
    let visible_windows = state.visible_windows();
    let popup_elements = |renderer: &mut R, visible: &Visible<'_>| {
        let mut out = Vec::new();
        for (popup, origin) in popups_of(visible.window, visible.rect) {
            let surfaces: Vec<WaylandSurfaceRenderElement<R>> = render_elements_from_surface_tree(
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
        let surfaces: Vec<WaylandSurfaceRenderElement<R>> = match visible.window.toplevel() {
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
        match visible.layer {
            Layer::Tiled => {
                let r = visible.rect;
                let clip = Rectangle::new((r.x, r.y).into(), (r.w, r.h).into())
                    .to_physical_precise_round(scale);
                elements.extend(
                    surfaces
                        .into_iter()
                        .filter_map(|surface| CropRenderElement::from_element(surface, scale, clip))
                        .map(OutputElement::Tiled),
                );
            }
            Layer::Maximized | Layer::Floating | Layer::Fullscreen => {
                elements.extend(surfaces.into_iter().map(OutputElement::Window));
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
    if !preview_drawn {
        let preview: Vec<_> = state
            .overlay
            .preview_elements(scale)
            .map(OutputElement::Solid)
            .collect();
        elements.splice(0..0, preview);
    }
    elements
}
