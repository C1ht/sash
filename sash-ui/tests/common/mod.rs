// SPDX-License-Identifier: Apache-2.0

//! Golden images shared by sash-ui's tests.

use std::path::PathBuf;

pub fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.png"))
}

/// Compares premultiplied RGBA `pixels` with tests/golden/NAME.png; SASH_UI_BLESS=1
/// writes it instead. Tolerance as sash-text's goldens: every channel within 2 on all
/// but 0.5% of the pixels, none more than 32 apart (the text rasteriser's
/// floating point may differ between CPUs; the rectangles are exact).
pub fn golden(name: &str, pixels: &[u8], w: u32, h: u32) {
    let path = golden_path(name);
    if std::env::var_os("SASH_UI_BLESS").is_some() {
        let file = std::fs::File::create(&path).expect("golden file");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(pixels).expect("png data");
        eprintln!("wrote {}", path.display());
        return;
    }
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&path).expect("golden missing (SASH_UI_BLESS=1 writes it)"),
    ));
    let mut reader = decoder.read_info().expect("png");
    let mut data = vec![0; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut data).expect("png frame");
    assert_eq!((info.width, info.height), (w, h), "{name}: size");
    let (mut off, mut worst) = (0usize, 0u8);
    for (a, b) in pixels.chunks(4).zip(data.chunks(4)) {
        let d = a
            .iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        if d > 2 {
            off += 1;
        }
        worst = worst.max(d);
    }
    let limit = (w * h) as usize / 200;
    assert!(
        off <= limit && worst <= 32,
        "{name}: {off} pixels off (limit {limit}), worst {worst}"
    );
}
