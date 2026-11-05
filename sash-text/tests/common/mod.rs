// SPDX-License-Identifier: Apache-2.0

//! Shared by the integration tests: golden images and font checks.

#![allow(dead_code)]

use std::path::PathBuf;

use sash_text::{ClientText, Image, Style, TextStack, fonts::FALLBACK};

pub const INK: [u8; 4] = [20, 20, 20, 255];

pub fn render(stack: &mut TextStack, text: &str, style: Style, scale: f32) -> Image {
    let line = stack.line(&ClientText::from(text), style, None);
    stack.draw(&line, scale, INK)
}

/// The named fallback fonts that are not installed, as a skip reason; None if all are.
pub fn missing_fonts(names: &[&str]) -> Option<String> {
    let missing: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| {
            FALLBACK
                .iter()
                .find(|(n, _, _)| n == name)
                .is_none_or(|(_, path, _)| !std::path::Path::new(path).exists())
        })
        .collect();
    (!missing.is_empty()).then(|| format!("font not installed: {}", missing.join(", ")))
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.png"))
}

/// Straight RGBA rows from a premultiplied ARGB8888 image.
pub fn to_rgba(image: &Image) -> Vec<u8> {
    let mut out = Vec::with_capacity((image.width * image.height * 4) as usize);
    for y in 0..image.height {
        for x in 0..image.width {
            let (a, r, g, b) = image.argb(x, y);
            let un = |c: u8| {
                if a == 0 {
                    0
                } else {
                    ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                }
            };
            out.extend_from_slice(&[un(r), un(g), un(b), a]);
        }
    }
    out
}

pub fn write_png(path: &std::path::Path, image: &Image) {
    let file = std::fs::File::create(path).expect("png file");
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        image.width.max(1),
        image.height.max(1),
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    let mut data = to_rgba(image);
    data.resize((image.width.max(1) * image.height.max(1) * 4) as usize, 0);
    writer.write_image_data(&data).expect("png data");
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect("golden"),
    ));
    let mut reader = decoder.read_info().expect("png info");
    let mut data = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut data).expect("png frame");
    data.truncate(info.buffer_size());
    (info.width, info.height, data)
}

/// Compares `image` with the golden image `name` (tests/golden/NAME.png); with
/// SASH_TEXT_BLESS=1 writes it instead. Tolerance: the same size, and every channel of
/// every pixel within 2 of the golden on all but 0.5% of the pixels, none more than
/// 32 apart. sash-text and swash are deterministic for one build; the tolerance only
/// absorbs floating-point differences between CPUs (FMA) in the rasteriser.
pub fn golden(name: &str, image: &Image) {
    let path = golden_path(name);
    if std::env::var_os("SASH_TEXT_BLESS").is_some() {
        write_png(&path, image);
        eprintln!("wrote {}", path.display());
        return;
    }
    let (w, h, golden) = read_png(&path);
    assert_eq!(
        (w, h),
        (image.width.max(1), image.height.max(1)),
        "{name}: size"
    );
    let mut got = to_rgba(image);
    got.resize(golden.len(), 0);
    let (mut off, mut worst) = (0usize, 0u8);
    for (a, b) in got.as_chunks::<4>().0.iter().zip(golden.as_chunks::<4>().0) {
        let d = a
            .iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        worst = worst.max(d);
        if d > 2 {
            off += 1;
        }
    }
    let pixels = (w * h) as usize;
    assert!(
        off * 200 <= pixels && worst <= 32,
        "{name}: {off} of {pixels} pixels differ by more than 2 (worst {worst})"
    );
}
