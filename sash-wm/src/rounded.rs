// SPDX-License-Identifier: Apache-2.0

//! Rounded corners on the GPU (M15b step 2): the shaders, compiled once per GL context
//! (a rebuilt renderer compiles them again), and the render elements that use them.
//!
//! - `MaskedSurface` draws a window's surface (shm or dmabuf, the toplevel and its
//!   subsurfaces) through the mask program: the texture is multiplied by the shape's
//!   coverage. It sets the program with Smithay's `override_default_tex_program` around
//!   the surface element's own draw.
//! - `shape_element` draws the border ring or a fill clipped by the shape (the bar, a
//!   hovered button) with the pixel program.
//!
//! The GLSL computes exactly `layout::shape` (the CPU reference): the signed distance of
//! a rectangle with a radius per corner, coverage `clamp(0.5 - d, 0, 1)` at the pixel
//! centre, and the ring between the edge and `width` pixels inside it.

use smithay::{
    backend::renderer::{
        element::{
            Element, Id, Kind, RenderElement, UnderlyingStorage,
            surface::WaylandSurfaceRenderElement,
        },
        gles::{
            GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform,
            UniformName, UniformType, element::PixelShaderElement,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Transform},
};

use crate::layout::{Edges, Rect, shape::Shape};

/// The signed distance, shared by both programs (GLSL ES 1.00). `shape` is x, y, w, h;
/// `radii` top-left, top-right, bottom-right, bottom-left.
const SDF: &str = r#"
uniform vec4 shape;
uniform vec4 radii;

float sdf(vec2 p) {
    vec2 half_size = shape.zw * 0.5;
    vec2 d = p - (shape.xy + half_size);
    float r = d.x < 0.0 ? (d.y < 0.0 ? radii.x : radii.w) : (d.y < 0.0 ? radii.y : radii.z);
    vec2 q = abs(d) - half_size + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
"#;

/// Precision: absolute pixel positions reach 5120 and more; mediump is not enough.
const PRECISION: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
"#;

/// The texture program: a surface's texture times the shape's coverage. `map` turns
/// texture coordinates into output pixels: p = v_coords * map.xy + map.zw.
fn mask_source() -> String {
    format!(
        r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
{PRECISION}
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec4 map;
{SDF}
void main() {{
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif
    vec2 p = v_coords * map.xy + map.zw;
    color = color * clamp(0.5 - sdf(p), 0.0, 1.0);
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}}
"#
    )
}

/// The pixel program: a premultiplied colour over the shape (`ring` 0) or over the
/// ring `ring` pixels wide along the edges whose flag is set in `edges` (left, top,
/// right, bottom), only inside `clip` (left, top, right, bottom). The shape and the clip
/// are relative to the drawn area; `extent` is the area's size in output pixels
/// (Smithay's `size` is its logical size).
fn shape_source() -> String {
    format!(
        r#"{PRECISION}
uniform vec2 size;
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec2 extent;
uniform vec4 color;
uniform float ring;
uniform vec4 edges;
uniform vec4 clip;
{SDF}
float ring_drawn(vec2 p) {{
    float left = p.x - shape.x;
    float top = p.y - shape.y;
    float right = shape.x + shape.z - p.x;
    float bottom = shape.y + shape.w - p.y;
    if (radii.x > 0.0 && left < radii.x && top < radii.x) return max(edges.x, edges.y);
    if (radii.y > 0.0 && right < radii.y && top < radii.y) return max(edges.z, edges.y);
    if (radii.z > 0.0 && right < radii.z && bottom < radii.z) return max(edges.z, edges.w);
    if (radii.w > 0.0 && left < radii.w && bottom < radii.w) return max(edges.x, edges.w);
    float nearest = min(min(left, top), min(right, bottom));
    if (nearest == left) return edges.x;
    if (nearest == top) return edges.y;
    if (nearest == right) return edges.z;
    return edges.w;
}}

void main() {{
    vec2 p = v_coords * extent;
    float d = sdf(p);
    float cov = clamp(0.5 - d, 0.0, 1.0);
    if (ring > 0.0) {{
        cov = (cov - clamp(0.5 - (d + ring), 0.0, 1.0)) * ring_drawn(p);
    }}
    cov = cov * step(clip.x, p.x) * step(p.x, clip.z) * step(clip.y, p.y) * step(p.y, clip.w);
    vec4 c = color * cov * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        c = vec4(0.0, 0.2, 0.0, 0.2) + c * 0.8;
#endif
    gl_FragColor = c;
}}
"#
    )
}

/// The compiled programs of one GL context.
#[derive(Clone)]
pub struct Programs {
    pub mask: GlesTexProgram,
    pub shape: GlesPixelProgram,
}

/// The programs for `renderer`'s GL context, compiled on first use and kept with the
/// context (a renderer rebuilt after a lost context compiles them again). None if they
/// do not compile: windows are then drawn square (logged once per context).
pub fn programs(renderer: &mut GlesRenderer) -> Option<Programs> {
    #[derive(Clone)]
    struct Compiled(Option<Programs>);
    if let Some(Compiled(programs)) = renderer.egl_context().user_data().get::<Compiled>() {
        return programs.clone();
    }
    let compiled = compile(renderer)
        .inspect_err(
            |err| tracing::warn!(%err, "rounded-corner shaders did not compile: square corners"),
        )
        .ok();
    renderer
        .egl_context()
        .user_data()
        .insert_if_missing(|| Compiled(compiled.clone()));
    compiled
}

fn compile(renderer: &mut GlesRenderer) -> Result<Programs, GlesError> {
    let shape_uniforms = [
        UniformName::new("shape", UniformType::_4f),
        UniformName::new("radii", UniformType::_4f),
    ];
    let mask = renderer.compile_custom_texture_shader(
        mask_source(),
        &[
            shape_uniforms[0].clone(),
            shape_uniforms[1].clone(),
            UniformName::new("map", UniformType::_4f),
        ],
    )?;
    let shape = renderer.compile_custom_pixel_shader(
        shape_source(),
        &[
            shape_uniforms[0].clone(),
            shape_uniforms[1].clone(),
            UniformName::new("extent", UniformType::_2f),
            UniformName::new("color", UniformType::_4f),
            UniformName::new("ring", UniformType::_1f),
            UniformName::new("edges", UniformType::_4f),
            UniformName::new("clip", UniformType::_4f),
        ],
    )?;
    Ok(Programs { mask, shape })
}

fn shape_uniforms(shape: &Shape, dx: f32, dy: f32) -> [Uniform<'static>; 2] {
    [
        Uniform::new("shape", [shape.x - dx, shape.y - dy, shape.w, shape.h]),
        Uniform::new("radii", shape.radii),
    ]
}

/// A window's surface drawn through its shape (or plainly without one).
#[derive(Debug)]
pub struct MaskedSurface {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    /// The mask program and the window's shape in output pixels.
    mask: Option<(GlesTexProgram, Shape)>,
}

impl MaskedSurface {
    pub fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        mask: Option<(GlesTexProgram, Shape)>,
    ) -> Self {
        // Only buffers drawn untransformed are masked (the texture-to-pixel map below
        // assumes it); clients practically never set a buffer transform.
        let mask = mask.filter(|_| inner.transform() == Transform::Normal);
        Self { inner, mask }
    }
}

impl Element for MaskedSurface {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    /// The surface's opaque regions without the rounded corners (what shows through
    /// there must still be drawn).
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let regions = self.inner.opaque_regions(scale);
        let Some((_, shape)) = &self.mask else {
            return regions;
        };
        let r = shape.radii.iter().fold(0.0f32, |a, b| a.max(*b)).ceil() as i32;
        let geometry = self.inner.geometry(scale);
        let (x, y) = (
            shape.x.round() as i32 - geometry.loc.x,
            shape.y.round() as i32 - geometry.loc.y,
        );
        let (w, h) = (shape.w.round() as i32, shape.h.round() as i32);
        // The shape minus its corner squares, as three bands.
        let bands = [
            Rectangle::<i32, Physical>::new((x + r, y).into(), ((w - 2 * r).max(0), r).into()),
            Rectangle::new((x, y + r).into(), (w, (h - 2 * r).max(0)).into()),
            Rectangle::new((x + r, y + h - r).into(), ((w - 2 * r).max(0), r).into()),
        ];
        regions
            .into_iter()
            .flat_map(|region| {
                bands
                    .iter()
                    .filter_map(move |band| region.intersection(*band))
            })
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for MaskedSurface {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let Some((program, shape)) = &self.mask else {
            return self.inner.draw(frame, src, dst, damage, opaque_regions);
        };
        // Texture coordinates are buffer coordinates over the whole buffer (the
        // element's full src): v = buffer / buffer size. The drawn part `src` maps to
        // `dst`: p = dst.loc + (v * buffer size - src.loc) * dst.size / src.size.
        let full = self.inner.src();
        let (bw, bh) = (full.size.w.max(1e-6), full.size.h.max(1e-6));
        let kx = dst.size.w as f64 / src.size.w.max(1e-6);
        let ky = dst.size.h as f64 / src.size.h.max(1e-6);
        // The full src starts at the buffer's origin (sash-wm offers no viewporter).
        let map = [
            (bw * kx) as f32,
            (bh * ky) as f32,
            (dst.loc.x as f64 - src.loc.x * kx) as f32,
            (dst.loc.y as f64 - src.loc.y * ky) as f32,
        ];
        let [shape_u, radii_u] = shape_uniforms(shape, 0.0, 0.0);
        frame.override_default_tex_program(
            program.clone(),
            vec![shape_u, radii_u, Uniform::new("map", map)],
        );
        let drawn = self.inner.draw(frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        drawn
    }

    /// A masked surface is never handed to a plane as it is (the corners need the
    /// shader); a square one keeps its storage (direct scan-out stays possible).
    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        if self.mask.is_some() {
            None
        } else {
            self.inner.underlying_storage(renderer)
        }
    }
}

/// What a shape element draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    /// A fill of `area` clipped by the shape.
    Fill,
    /// The border ring, `width` logical pixels, along `edges`.
    Ring { width: i32, edges: Edges },
}

/// The pixel-shader element for a fill or ring over `clip` (output pixels, from the
/// window's frame) clipped by `shape` (output pixels) in `color` (straight RGBA, 0..1),
/// at `scale`. `area` is the same rectangle in logical pixels: Smithay draws it one
/// logical pixel larger on every side (its own rounding may fall a pixel short of the
/// frame's edges) and the shader cuts it to `clip`. `reuse` keeps
/// an earlier element (its id); the caller passes it only when something changed, since
/// new uniforms count as damage.
#[allow(clippy::too_many_arguments)]
pub fn shape_element(
    program: &GlesPixelProgram,
    reuse: Option<PixelShaderElement>,
    area: Rect,
    clip: Rectangle<i32, Physical>,
    shape: &Shape,
    scale: f64,
    paint: Paint,
    color: [f32; 4],
) -> PixelShaderElement {
    let premultiplied = [
        color[0] * color[3],
        color[1] * color[3],
        color[2] * color[3],
        color[3],
    ];
    let (ring, edges) = match paint {
        Paint::Fill => (0.0, [0.0; 4]),
        Paint::Ring { width, edges } => (
            width as f32 * scale as f32,
            [edges.left, edges.top, edges.right, edges.bottom].map(|e| if e { 1.0 } else { 0.0 }),
        ),
    };
    let logical = Rectangle::<i32, Logical>::new(
        (area.x - 1, area.y - 1).into(),
        (area.w + 2, area.h + 2).into(),
    );
    // Where Smithay draws the element (its geometry at this scale).
    let drawn: Rectangle<i32, Physical> = logical.to_physical_precise_round(scale);
    let [shape_u, radii_u] = shape_uniforms(shape, drawn.loc.x as f32, drawn.loc.y as f32);
    let (cx, cy) = (clip.loc.x - drawn.loc.x, clip.loc.y - drawn.loc.y);
    let clip = [cx, cy, cx + clip.size.w, cy + clip.size.h].map(|v| v as f32);
    let uniforms = vec![
        shape_u,
        radii_u,
        Uniform::new("extent", [drawn.size.w as f32, drawn.size.h as f32]),
        Uniform::new("color", premultiplied),
        Uniform::new("ring", ring),
        Uniform::new("edges", edges),
        Uniform::new("clip", clip),
    ];
    match reuse {
        Some(mut element) => {
            element.resize(logical, None);
            element.update_uniforms(uniforms);
            element
        }
        None => PixelShaderElement::new(
            program.clone(),
            logical,
            None,
            1.0,
            uniforms,
            Kind::Unspecified,
        ),
    }
}
