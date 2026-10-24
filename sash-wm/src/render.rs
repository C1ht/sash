// SPDX-License-Identifier: Apache-2.0

//! What to draw for an output. Tiled windows at their tile, clipped to it; floating
//! windows above them in stacking order, unclipped. Shared by all backends.

use smithay::{
    backend::renderer::{
        ImportAll, Renderer, Texture,
        element::{
            AsRenderElements, render_elements, surface::WaylandSurfaceRenderElement,
            utils::CropRenderElement,
        },
    },
    utils::{Rectangle, Scale},
};

use crate::{layout::Mode, state::Sash};

render_elements! {
    pub OutputElement<R> where R: ImportAll;
    Tiled=CropRenderElement<WaylandSurfaceRenderElement<R>>,
    Floating=WaylandSurfaceRenderElement<R>,
}

/// Render elements for the output at the origin with the given scale, front to back
/// (the first element is drawn on top).
pub fn output_elements<R>(state: &Sash, renderer: &mut R, scale: f64) -> Vec<OutputElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let scale = Scale::from(scale);
    let mut elements = Vec::new();
    for visible in state.visible_windows().into_iter().rev() {
        let surfaces: Vec<WaylandSurfaceRenderElement<R>> = visible.window.render_elements(
            renderer,
            visible.position.to_physical_precise_round(scale),
            scale,
            1.0,
        );
        match visible.mode {
            Mode::Floating => elements.extend(surfaces.into_iter().map(OutputElement::Floating)),
            Mode::Tiled => {
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
        }
    }
    elements
}
