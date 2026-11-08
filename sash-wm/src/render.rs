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
            Id, Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            render_elements,
            solid::SolidColorRenderElement,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::CropRenderElement,
        },
        gles::{GlesRenderer, element::PixelShaderElement},
        utils::CommitCounter,
    },
    input::pointer::CursorImageStatus,
    utils::{Physical, Point, Rectangle, Scale, Transform},
};

use std::cell::RefCell;

use crate::{
    layout::{Layer, Rect, gaps::Gapped, layers::Level},
    popups::popups_of,
    rounded::MaskedSurface,
    state::Sash,
    windows::Visible,
};

/// Background where no window is.
/// Background where no window is: the theme's page (until a wallpaper exists).
pub fn clear_color(state: &Sash) -> [f32; 4] {
    state.theme().page.rgba_f32()
}

/// The snap preview (owner, M18b): the accent at 12% with a 1 px edge at 60%.
const PREVIEW_FILL: u8 = 12;
const PREVIEW_EDGE: u8 = 60;
const PREVIEW_EDGE_PX: i32 = 1;
/// Behind a fixed-size fullscreen window: opaque black.
const BACKDROP: sash_theme::Color = sash_theme::BLACK;

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
    /// Glyphs and titles, pixel for pixel.
    Exact=Exact,
}

/// A buffer drawn pixel for pixel at a physical rectangle (glyphs and titles, drawn at
/// the output scale): Smithay's memory element would size it from its logical size,
/// rounded apart from its position.
pub struct Exact {
    inner: MemoryRenderBufferRenderElement<GlesRenderer>,
    at: Rectangle<i32, Physical>,
}

impl Exact {
    /// `buffer` (scale 1, as many pixels as `at`) at `at`.
    pub fn new(
        renderer: &mut GlesRenderer,
        buffer: &MemoryRenderBuffer,
        at: Rectangle<i32, Physical>,
    ) -> Option<Self> {
        let inner = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.loc.to_f64(),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()?;
        Some(Self { inner, at })
    }
}

impl smithay::backend::renderer::element::Element for Exact {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.at.loc
    }

    fn src(&self) -> Rectangle<f64, smithay::utils::Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.at
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> smithay::backend::renderer::utils::DamageSet<i32, Physical> {
        if commit == Some(self.current_commit()) {
            Default::default()
        } else {
            smithay::backend::renderer::utils::DamageSet::from_slice(&[Rectangle::from_size(
                self.at.size,
            )])
        }
    }

    fn alpha(&self) -> f32 {
        1.0
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl smithay::backend::renderer::element::RenderElement<GlesRenderer> for Exact {
    fn draw(
        &self,
        frame: &mut smithay::backend::renderer::gles::GlesFrame<'_, '_>,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        self.inner.draw(frame, src, dst, damage, opaque_regions)
    }
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
            let [r, g, b, a] = match (inside(x, y), edge) {
                (true, true) => sash_theme::BLACK.rgba8(),
                (true, false) => sash_theme::WHITE.rgba8(),
                _ => [0; 4],
            };
            pixels.extend_from_slice(&[b, g, r, a]);
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

/// A compositor-drawn rectangle of one colour. It keeps its id and commit counter from
/// frame to frame, so damage tracking sees an unchanged one as unchanged, and it is
/// drawn at its physical rectangle with each edge rounded on its own
/// (`layout::gaps::to_physical`; Smithay's `SolidColorBuffer` rounds the size apart
/// from the position, owner M18a).
pub struct Solid {
    id: Id,
    commit: CommitCounter,
    rect: Rect,
    color: [f32; 4],
}

impl Solid {
    pub fn new(rect: Rect, color: [f32; 4]) -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
            rect,
            color,
        }
    }

    pub fn update(&mut self, rect: Rect, color: [f32; 4]) {
        if (rect, color) != (self.rect, self.color) {
            (self.rect, self.color) = (rect, color);
            self.commit.increment();
        }
    }

    pub fn element(&self, scale: Scale<f64>, alpha: f32) -> SolidColorRenderElement {
        self.element_at(crate::layout::gaps::to_physical(self.rect, scale.x), alpha)
    }

    /// Drawn inside a window: placed by the window's frame.
    pub fn element_in(
        &self,
        frame: &crate::layout::gaps::Frame,
        alpha: f32,
    ) -> SolidColorRenderElement {
        self.element_at(frame.map(self.rect), alpha)
    }

    /// At a physical rectangle.
    pub fn element_at(
        &self,
        rect: Rectangle<i32, Physical>,
        alpha: f32,
    ) -> SolidColorRenderElement {
        let c = self.color;
        SolidColorRenderElement::new(
            self.id.clone(),
            rect,
            self.commit,
            // Straight to premultiplied.
            [
                c[0] * c[3] * alpha,
                c[1] * c[3] * alpha,
                c[2] * c[3] * alpha,
                c[3] * alpha,
            ],
            Kind::Unspecified,
        )
    }
}

/// What a cached preview element was made for.
type PreviewKey = ([i32; 4], [u32; 4], u64, i32);

/// Compositor-drawn rectangles, kept from frame to frame.
pub struct Overlay {
    preview: Option<Gapped>,
    /// Without the shaders: the fill inside the edge, then the top, bottom, left and
    /// right edge strips.
    pieces: RefCell<[Solid; 5]>,
    /// With them: the rounded fill and the edge ring.
    shaders: RefCell<[Option<(PixelShaderElement, PreviewKey)>; 2]>,
    backdrop: Option<Solid>,
}

impl Overlay {
    pub fn new() -> Self {
        let piece = || Solid::new(Rect::default(), BACKDROP.rgba_f32());
        Self {
            preview: None,
            pieces: RefCell::new([piece(), piece(), piece(), piece(), piece()]),
            shaders: RefCell::new([None, None]),
            backdrop: None,
        }
    }

    /// Shows the snap preview over what `zone` shows, or hides it.
    pub fn set_preview(&mut self, zone: Option<Gapped>) {
        self.preview = zone;
    }

    #[cfg(test)]
    pub fn preview(&self) -> Option<Rect> {
        self.preview.map(|z| z.logical())
    }

    /// Shows the opaque backdrop over `rect`, or hides it.
    pub fn set_backdrop(&mut self, rect: Option<Rect>) {
        let color = BACKDROP.rgba_f32();
        match (rect, &mut self.backdrop) {
            (None, backdrop) => *backdrop = None,
            (Some(r), Some(solid)) => solid.update(r, color),
            (Some(r), backdrop @ None) => *backdrop = Some(Solid::new(r, color)),
        }
    }

    /// The preview, front to back: the same rounded shape (`radius`) and gaps as the
    /// window it previews, its frame at this scale.
    fn preview_elements(
        &self,
        renderer: &mut GlesRenderer,
        scale: Scale<f64>,
        theme: &sash_theme::Theme,
        radius: i32,
    ) -> Vec<OutputElement> {
        let Some(zone) = self.preview else {
            return Vec::new();
        };
        let frame = zone.frame(scale.x);
        let fill = theme.accent.at(PREVIEW_FILL).rgba_f32();
        let edge = theme.accent.at(PREVIEW_EDGE).rgba_f32();
        if let Some(programs) = crate::rounded::programs(renderer) {
            let shape =
                crate::layout::shape::Shape::from_physical(frame.physical, radius, scale.x as f32);
            let paints = [
                (
                    crate::rounded::Paint::Ring {
                        width: PREVIEW_EDGE_PX,
                        edges: crate::layout::Edges::ALL,
                    },
                    edge,
                ),
                (crate::rounded::Paint::Fill, fill),
            ];
            let p = frame.physical;
            let mut shaders = self.shaders.borrow_mut();
            return paints
                .into_iter()
                .zip(shaders.iter_mut())
                .map(|((paint, color), cached)| {
                    let key = (
                        [p.loc.x, p.loc.y, p.size.w, p.size.h],
                        color.map(f32::to_bits),
                        scale.x.to_bits(),
                        radius,
                    );
                    let element = match cached.take() {
                        Some((element, was)) if was == key => element,
                        old => crate::rounded::shape_element(
                            &programs.shape,
                            old.map(|(e, _)| e),
                            frame.logical,
                            p,
                            &shape,
                            scale.x,
                            paint,
                            color,
                        ),
                    };
                    *cached = Some((element.clone(), key));
                    OutputElement::Shader(element)
                })
                .collect();
        }
        let r = frame.logical;
        let b = PREVIEW_EDGE_PX.min(r.w / 2).min(r.h / 2).max(0);
        let inner_h = (r.h - 2 * b).max(0);
        let rects = [
            (
                Rect::new(r.x + b, r.y + b, (r.w - 2 * b).max(0), inner_h),
                fill,
            ),
            (Rect::new(r.x, r.y, r.w, b), edge),
            (Rect::new(r.x, r.bottom() - b, r.w, b), edge),
            (Rect::new(r.x, r.y + b, b, inner_h), edge),
            (Rect::new(r.right() - b, r.y + b, b, inner_h), edge),
        ];
        let mut pieces = self.pieces.borrow_mut();
        pieces
            .iter_mut()
            .zip(rects)
            .map(|(piece, (rect, color))| {
                piece.update(rect, color);
                OutputElement::Solid(piece.element_in(&frame, 1.0))
            })
            .collect()
    }

    fn backdrop_element(&self, scale: Scale<f64>) -> Option<SolidColorRenderElement> {
        self.backdrop
            .as_ref()
            .map(|solid| solid.element(scale, 1.0))
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
        let frame = state.windows.layout.frame(visible.id, scale.x);
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
                    frame.map_or_else(
                        || visible.position.to_physical_precise_round(scale),
                        |f| f.point(visible.position.x, visible.position.y),
                    ),
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
                let clip = frame.map_or_else(
                    || crate::layout::gaps::to_physical(visible.rect, scale.x),
                    |f| f.map(visible.rect),
                );
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
            let theme = state.theme();
            let radius = state.windows.layout.radius();
            elements.extend(
                state
                    .overlay
                    .preview_elements(renderer, scale, theme, radius),
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
        let theme = state.theme();
        let radius = state.windows.layout.radius();
        let preview = state
            .overlay
            .preview_elements(renderer, scale, theme, radius);
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
