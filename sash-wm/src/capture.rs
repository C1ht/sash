// SPDX-License-Identifier: Apache-2.0

//! Debug builds only: renders the current output once more into an offscreen texture
//! and writes it as a PNG. Screenshots never touch the host screen. Triggered by
//! SIGUSR1; the files go to the directory in $SASH_CAPTURE_DIR.

use std::{error::Error, io::BufWriter, path::Path};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, Texture,
            damage::OutputDamageTracker,
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
    R: Renderer + ImportAll + ImportMem + Offscreen<T> + Bind<T> + ExportMem,
    R::TextureId: Texture + Send + Clone + 'static,
    R::Error: 'static,
{
    let pixels = render_rgba::<R, T>(renderer, state, size)?;
    let png = encode_png(&pixels, size.w as u32, size.h as u32)?;
    std::fs::write(path, png)?;
    Ok(())
}

/// Renders what the output shows right now into an offscreen buffer (never the
/// screen) and reads it back: RGBA, 8 bits per channel, rows top to bottom.
pub fn render_rgba<R, T>(
    renderer: &mut R,
    state: &Sash,
    size: Size<i32, Physical>,
) -> Result<Vec<u8>, Box<dyn Error>>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<T> + Bind<T> + ExportMem,
    R::TextureId: Texture + Send + Clone + 'static,
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
    Ok(renderer.map_texture(&mapping)?.to_vec())
}

/// RGBA pixels as an 8-bit RGBA PNG.
pub fn encode_png(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(BufWriter::new(&mut png), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(pixels)?;
    }
    Ok(png)
}
