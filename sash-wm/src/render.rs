// SPDX-License-Identifier: Apache-2.0

//! What to draw for an output: every mapped window at its tile, clipped to the tile.
//! Shared by all backends.

use smithay::{
    backend::renderer::{
        ImportAll, Renderer, Texture,
        element::{
            AsRenderElements, surface::WaylandSurfaceRenderElement, utils::CropRenderElement,
        },
    },
    utils::{Rectangle, Scale},
};

use crate::state::Sash;

pub type TileElement<R> = CropRenderElement<WaylandSurfaceRenderElement<R>>;

/// Render elements for the output at the origin with the given scale. Tiles do not
/// overlap, so their order does not matter.
pub fn tile_elements<R>(state: &Sash, renderer: &mut R, scale: f64) -> Vec<TileElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let scale = Scale::from(scale);
    let mut elements = Vec::new();
    for (window, tile, position) in state.visible_tiles() {
        let clip = Rectangle::new((tile.x, tile.y).into(), (tile.w, tile.h).into())
            .to_physical_precise_round(scale);
        let surfaces: Vec<WaylandSurfaceRenderElement<R>> = window.render_elements(
            renderer,
            position.to_physical_precise_round(scale),
            scale,
            1.0,
        );
        elements.extend(
            surfaces
                .into_iter()
                .filter_map(|surface| CropRenderElement::from_element(surface, scale, clip)),
        );
    }
    elements
}
