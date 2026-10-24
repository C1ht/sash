// SPDX-License-Identifier: Apache-2.0

//! Debug builds only: renders the current output once more into an offscreen texture
//! and writes it as a PNG. Screenshots never touch the host screen. Triggered by
//! SIGUSR1; the files go to the directory in $SASH_CAPTURE_DIR.

use std::{error::Error, fs::File, io::BufWriter, path::Path};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, ImportAll, Offscreen, Renderer, Texture, damage::OutputDamageTracker,
        },
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use crate::{render, state::Sash};

pub const CAPTURE_DIR_VAR: &str = "SASH_CAPTURE_DIR";

/// Renders what the output shows right now (`size` in physical pixels, scale 1) and
/// writes it to `path` as an 8-bit RGBA PNG.
pub fn write_png<R, T>(
    renderer: &mut R,
    state: &Sash,
    size: Size<i32, Physical>,
    path: &Path,
) -> Result<(), Box<dyn Error>>
where
    R: Renderer + ImportAll + Offscreen<T> + Bind<T> + ExportMem,
    R::TextureId: Texture + Clone + 'static,
    R::Error: 'static,
{
    let buffer_size = Size::<i32, Buffer>::from((size.w, size.h));
    // ABGR8888 is R, G, B, A in memory: PNG's RGBA byte order.
    let mut target = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    let elements = render::output_elements(state, renderer, 1.0);
    let mapping = {
        let mut framebuffer = renderer.bind(&mut target)?;
        // A fresh tracker: no previous frame, so everything is drawn.
        let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        damage.render_output(
            renderer,
            &mut framebuffer,
            0,
            &elements,
            render::CLEAR_COLOR,
        )?;
        renderer.copy_framebuffer(
            &framebuffer,
            Rectangle::from_size(buffer_size),
            Fourcc::Abgr8888,
        )?
    };
    let pixels = renderer.map_texture(&mapping)?;

    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, size.w as u32, size.h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}
