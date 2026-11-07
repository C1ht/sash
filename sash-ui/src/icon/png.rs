// SPDX-License-Identifier: Apache-2.0

//! PNG icons (theme and app icons), decoded with limits: at most [`MAX_PNG_BYTES`] of
//! file, [`MAX_PNG_SIDE`] pixels per side, and the decoder's allocation capped to
//! what such an image needs. The image is fitted into the requested square, halved
//! with bilinear filtering while it is more than twice the target, so a 512 px icon
//! drawn at 24 px is not aliased.

use std::io::Cursor;

use resvg::tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};

pub const MAX_PNG_BYTES: usize = 1024 * 1024;
pub const MAX_PNG_SIDE: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngError {
    TooLarge,
    Decode,
    BadSize,
}

/// Decodes `bytes` into a `px` x `px` pixmap (premultiplied RGBA), fitted and centred.
pub fn render(bytes: &[u8], px: u32) -> Result<Pixmap, PngError> {
    if bytes.len() > MAX_PNG_BYTES {
        return Err(PngError::TooLarge);
    }
    let limits = png::Limits {
        bytes: (MAX_PNG_SIDE * MAX_PNG_SIDE * 4) as usize + 64 * 1024,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|_| PngError::Decode)?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > MAX_PNG_SIDE || h > MAX_PNG_SIDE {
        return Err(PngError::BadSize);
    }
    let mut buf = vec![0; reader.output_buffer_size().ok_or(PngError::BadSize)?];
    let info = reader.next_frame(&mut buf).map_err(|_| PngError::Decode)?;
    let (w, h) = (info.width, info.height);
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(PngError::Decode),
    };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in buf[..(w * h) as usize * channels].chunks_exact(channels) {
        let (r, g, b, a) = match channels {
            1 => (p[0], p[0], p[0], 255),
            2 => (p[0], p[0], p[0], p[1]),
            3 => (p[0], p[1], p[2], 255),
            _ => (p[0], p[1], p[2], p[3]),
        };
        let pre = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
        rgba.extend_from_slice(&[pre(r), pre(g), pre(b), a]);
    }
    let size = IntSize::from_wh(w, h).ok_or(PngError::BadSize)?;
    let mut image = Pixmap::from_vec(rgba, size).ok_or(PngError::BadSize)?;
    let px = px.clamp(1, super::MAX_ICON_PX);
    // Halve while more than twice the target.
    while image.width().max(image.height()) > 2 * px {
        let (hw, hh) = ((image.width() / 2).max(1), (image.height() / 2).max(1));
        image = scaled(&image, hw, hh, 0.0, 0.0).ok_or(PngError::BadSize)?;
    }
    let scale = px as f32 / image.width().max(image.height()) as f32;
    let (dw, dh) = (image.width() as f32 * scale, image.height() as f32 * scale);
    let mut out = Pixmap::new(px, px).ok_or(PngError::BadSize)?;
    draw(
        &mut out,
        &image,
        scale,
        (px as f32 - dw) / 2.0,
        (px as f32 - dh) / 2.0,
    );
    Ok(out)
}

fn scaled(src: &Pixmap, w: u32, h: u32, x: f32, y: f32) -> Option<Pixmap> {
    let mut out = Pixmap::new(w, h)?;
    draw(&mut out, src, w as f32 / src.width() as f32, x, y);
    Some(out)
}

fn draw(out: &mut Pixmap, src: &Pixmap, scale: f32, x: f32, y: f32) {
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    out.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &paint,
        Transform::from_row(scale, 0.0, 0.0, scale, x, y),
        None,
    );
}
