// SPDX-License-Identifier: Apache-2.0

//! Window titles in the bars (M15). Bar pictures are composed on the CPU the way a
//! frame draws them (fill, button glyphs, the title image where `bar_title` puts it);
//! goldens live in fixtures/titles/ (SASH_TEXT_BLESS=1 writes them; look at each).
//! Goldens that need the Noto fonts are ignored here and run in the debian:trixie
//! container by tools/text-goldens.sh (an ignored test is listed as ignored, never as
//! passed).

use std::{path::PathBuf, time::Instant};

use sash_text::{ClientText, Image, TextStack};
use smithay::{backend::input::ButtonState, utils::Scale};

use super::{
    client::TestClient,
    decorations::{FLOAT, decorated, floating},
    harness::Harness,
};
use crate::{
    decorations::{Glyph, glyph_mask},
    layout::{
        Rect, WindowId, ZoneId,
        decor::{BUTTONS, Decoration, GLYPH},
    },
    seat::BTN_LEFT,
    titles::{self, FOCUSED, UNFOCUSED},
};

const FILL: [u8; 3] = [0x2C, 0x2C, 0x2E];

/// The bar of a FLOAT-sized window with title `text`, at `scale`: opaque RGB rows
/// (width = bar width x scale).
fn bar_picture(
    stack: &mut TextStack,
    text: &str,
    focused: bool,
    outer_w: i32,
    scale: f32,
) -> (u32, u32, Vec<u8>) {
    let outer = Rect::new(0, 0, outer_w, 300);
    let d = Decoration::FULL;
    let bar = d.bar_rect(outer).expect("a bar");
    let px = |v: i32| (v as f32 * scale).round() as i32;
    let (w, h) = (px(bar.w) as u32, px(bar.h) as u32);
    let mut rgb = FILL.repeat((w * h) as usize);
    let mut over = |x: i32, y: i32, a: u8, color: [u8; 3]| {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return;
        }
        let i = (y as usize * w as usize + x as usize) * 3;
        for c in 0..3 {
            let dst = u32::from(rgb[i + c]);
            rgb[i + c] = ((u32::from(color[c]) * u32::from(a) + dst * (255 - u32::from(a)) + 127)
                / 255) as u8;
        }
    };
    let glyph_alpha = ((if focused { 0.92 } else { 0.35 }) * 255.0f32).round() as u8;
    for (button, glyph) in BUTTONS.iter().zip([Glyph::Close, Glyph::Maximize]) {
        let area = d.button_rect(outer, *button).expect("button");
        let mask = glyph_mask(glyph);
        let x0 = area.x - bar.x + (area.w - GLYPH) / 2;
        let y0 = area.y - bar.y + (area.h - GLYPH) / 2;
        for y in 0..px(GLYPH) {
            for x in 0..px(GLYPH) {
                let (gx, gy) = ((x as f32 / scale) as i32, (y as f32 / scale) as i32);
                if mask[(gy.min(GLYPH - 1) * GLYPH + gx.min(GLYPH - 1)) as usize] {
                    over(px(x0) + x, px(y0) + y, glyph_alpha, [255, 255, 255]);
                }
            }
        }
    }
    if let Some(area) = d.title_rect(outer) {
        let line = titles::shape(stack, &ClientText::from(text), focused);
        let image = titles::draw(stack, &line, focused, area.w, scale);
        let top = (f64::from(area.y - bar.y)
            + ((f64::from(area.h) - f64::from(line.height())) / 2.0).round())
            * f64::from(scale);
        let x0 = px(area.x - bar.x) - image.origin.0 as i32;
        let y0 = top as i32 - image.origin.1 as i32;
        // The image is premultiplied ARGB (white text, colour emoji): source over.
        for y in 0..image.height {
            for x in 0..image.width {
                let (a, r, g, b) = image.argb(x, y);
                let (dx, dy) = (x0 + x as i32, y0 + y as i32);
                if a == 0 || dx < 0 || dy < 0 || dx >= w as i32 || dy >= h as i32 {
                    continue;
                }
                let i = (dy as usize * w as usize + dx as usize) * 3;
                for (c, s) in [r, g, b].into_iter().enumerate() {
                    let dst = u32::from(rgb[i + c]);
                    rgb[i + c] =
                        (u32::from(s) + (dst * (255 - u32::from(a)) + 127) / 255).min(255) as u8;
                }
            }
        }
    }
    (w, h, rgb)
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/titles")
}

/// Compares with fixtures/titles/NAME.png (SASH_TEXT_BLESS=1 writes it). Tolerance as
/// sash-text's goldens: the same size, all but 0.5% of pixels within 2 per channel,
/// none more than 32 apart.
fn golden(name: &str, (w, h, rgb): (u32, u32, Vec<u8>)) {
    let path = golden_dir().join(format!("{name}.png"));
    if std::env::var_os("SASH_TEXT_BLESS").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgb)
            .unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut data).unwrap();
    assert_eq!((info.width, info.height), (w, h), "{name}: size");
    let (mut off, mut worst) = (0usize, 0u8);
    for (a, b) in rgb.as_chunks::<3>().0.iter().zip(data.as_chunks::<3>().0) {
        let d = a
            .iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        worst = worst.max(d);
        off += usize::from(d > 2);
    }
    let pixels = (w * h) as usize;
    assert!(
        off * 200 <= pixels && worst <= 32,
        "{name}: {off} of {pixels} pixels differ (worst {worst})"
    );
}

#[test]
fn bars_with_latin_danish_accents_and_an_empty_title() {
    let mut stack = TextStack::new();
    for (name, text) in [
        ("latin", "Settings - Displays & Monitors"),
        ("danish", "Ærø Ø Å æblegrød på øen"),
        ("accents", "Café naïve façade Zürich São Paulo Łódź"),
        ("empty", ""),
    ] {
        golden(
            &format!("bar-{name}"),
            bar_picture(&mut stack, text, true, FLOAT.w, 1.0),
        );
    }
    golden(
        "bar-latin-unfocused",
        bar_picture(
            &mut stack,
            "Settings - Displays & Monitors",
            false,
            FLOAT.w,
            1.0,
        ),
    );
    for (scale, tag) in [(1.5, "1.5"), (2.0, "2")] {
        golden(
            &format!("bar-latin-x{tag}"),
            bar_picture(
                &mut stack,
                "Settings - Displays & Monitors",
                true,
                FLOAT.w,
                scale,
            ),
        );
    }
    // A title longer than the bar ends with an ellipsis at the right padding.
    golden(
        "bar-long",
        bar_picture(
            &mut stack,
            &"Text Editor - a very long document name ".repeat(4),
            true,
            FLOAT.w,
            1.0,
        ),
    );
}

#[test]
#[ignore = "needs the Noto fonts: tools/text-goldens.sh runs it in debian:trixie"]
fn bars_with_cjk_and_emoji() {
    let mut stack = TextStack::new();
    for (name, text) in [
        ("cjk", "設定 日本語のテキスト 中文标题 한국어"),
        ("emoji", "Party 🎉 smile 😀 heart ❤ flag 🇩🇰"),
    ] {
        let line = titles::shape(&mut stack, &ClientText::from(text), true);
        assert!(
            line.fonts_used().len() > 1,
            "{name}: fallback fonts installed: {:?}",
            line.fonts_used()
        );
        for (scale, tag) in [(1.0, "1"), (1.5, "1.5"), (2.0, "2")] {
            golden(
                &format!("bar-{name}-x{tag}"),
                bar_picture(&mut stack, text, true, FLOAT.w, scale),
            );
        }
    }
}

/// At every bar width the title's ink stays between the buttons and the right padding
/// (1 px of anti-aliasing allowed), and a title that does not fit ends with an ellipsis.
#[test]
fn truncation_at_the_button_and_padding_edges() {
    let mut stack = TextStack::new();
    let text = ClientText::from("Files - Documents and Pictures and Music and Videos");
    for focused in [true, false] {
        let line = titles::shape(&mut stack, &text, focused);
        for outer_w in (60..=520).step_by(7) {
            let outer = Rect::new(0, 0, outer_w, 300);
            let Some(area) = Decoration::FULL.title_rect(outer) else {
                assert!(outer_w <= 2 * 32 + 2 * 12 + 2, "{outer_w}: no title area");
                continue;
            };
            let image = titles::draw(&mut stack, &line, focused, area.w, 1.0);
            let fitted = line.fit(Some(area.w as f32));
            assert!(fitted.width() <= area.w as f32 + 0.01, "{outer_w}");
            assert_eq!(
                fitted.truncated(),
                line.width() > area.w as f32,
                "{outer_w}"
            );
            let ink: Vec<i32> = (0..image.height)
                .flat_map(|y| (0..image.width).map(move |x| (x, y)))
                .filter(|(x, y)| image.argb(*x, *y).0 > 0)
                .map(|(x, _)| area.x + x as i32 - image.origin.0 as i32)
                .collect();
            if let (Some(min), Some(max)) = (ink.iter().min(), ink.iter().max()) {
                let buttons_end = 1 + BUTTONS.len() as i32 * 32;
                assert!(
                    *min >= buttons_end + 12 - 1,
                    "{outer_w}: ink at {min} reaches the buttons' padding"
                );
                assert!(
                    *max < area.right() + 1,
                    "{outer_w}: ink at {max} past {}",
                    area.right()
                );
            }
        }
    }
}

#[test]
fn focus_changes_weight_and_brightness() {
    let mut stack = TextStack::new();
    let text = ClientText::from("Settings");
    let draw = |stack: &mut TextStack, focused| {
        let line = titles::shape(stack, &text, focused);
        titles::draw(stack, &line, focused, 300, 1.0)
    };
    let (on, off) = (draw(&mut stack, true), draw(&mut stack, false));
    let max = |i: &Image| {
        i.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .max()
            .unwrap_or(0)
    };
    assert_eq!(max(&on), (FOCUSED.1 * 255.0).round() as u8, "focused: 92%");
    assert_eq!(
        max(&off),
        (UNFOCUSED.1 * 255.0).round() as u8,
        "unfocused: 55%"
    );
    // Weight: the focused title has more ink per unit of alpha (500 against 400).
    let coverage = |i: &Image, alpha: f32| {
        i.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| f32::from(p[3]))
            .sum::<f32>()
            / alpha
    };
    assert!(coverage(&on, FOCUSED.1) > coverage(&off, UNFOCUSED.1) * 1.03);
}

/// WCAG contrast of the unfocused title (white at 55% over #2C2C2E) against the bar.
#[test]
fn contrast_of_the_unfocused_title() {
    fn luminance(rgb: [f64; 3]) -> f64 {
        let lin = |c: f64| {
            let c = c / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
    }
    let fill = FILL.map(f64::from);
    let ratio = |alpha: f64| {
        let text = fill.map(|c| 255.0 * alpha + c * (1.0 - alpha));
        (luminance(text) + 0.05) / (luminance(fill) + 0.05)
    };
    let (unfocused, focused) = (ratio(f64::from(UNFOCUSED.1)), ratio(f64::from(FOCUSED.1)));
    eprintln!("contrast on #2C2C2E: unfocused {unfocused:.2}:1, focused {focused:.2}:1");
    assert!(unfocused >= 4.5, "WCAG AA for normal text: {unfocused:.2}");
}

/// Untrusted titles: invalid UTF-8, zero-width only, controls, overlong.
#[test]
fn untrusted_titles_never_panic() {
    let mut stack = TextStack::new();
    let long = "Ø".repeat(100_000);
    let inputs: [&[u8]; 6] = [
        b"\xff\xfe\xc3",
        "\u{200b}\u{200d}\u{feff}".as_bytes(),
        b"a\x00b\x1b[2Jc\nd",
        long.as_bytes(),
        "e\u{301}\u{301}\u{301}".as_bytes(),
        b"",
    ];
    for bytes in inputs {
        let text = ClientText::new(bytes);
        for focused in [true, false] {
            let line = titles::shape(&mut stack, &text, focused);
            for width in [0, 1, 50, 310, 2000] {
                let image = titles::draw(&mut stack, &line, focused, width, 1.0);
                assert_eq!(image.pixels.len(), (image.stride * image.height) as usize);
            }
        }
    }
}

/// The title of window 1 as a frame draws it.
fn drawn(h: &Harness) -> Option<titles::Drawn> {
    let v = h
        .server
        .state
        .visible_windows()
        .into_iter()
        .find(|v| v.id == WindowId(1))?;
    h.server
        .state
        .bar_title(v.id, v.outer, v.decoration, Scale::from(1.0))
        .map(|(d, _)| d)
}

#[test]
fn the_title_follows_the_client_while_dragged_snapped_maximized_and_fullscreen() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        // No title yet (and app_id is never used): an empty bar.
        w.toplevel.set_title(String::new());
        w.toplevel.set_app_id("org.example.app".into());
        floating(&server, 1);
        c.roundtrip();
        server.on_server(|h| {
            h.run_until("the empty title shaped", |h| {
                drawn(h);
                h.server.state.titles.is_current(WindowId(1))
            });
            assert!(drawn(h).is_none(), "app_id is not a fallback");
        });
        let set = |c: &mut TestClient, title: &str| {
            w.toplevel.set_title(title.into());
            c.roundtrip();
        };
        set(&mut c, "Files");
        server.on_server(|h| {
            h.run_until("drawn", |h| drawn(h).is_some());
            // Dragging by the title.
            h.pointer_to(300.0, 116.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(320.0, 136.0);
        });
        set(&mut c, "Files - dragged");
        server.on_server(|h| {
            let before = h.server.state.titles.shapings.get();
            h.run_until("reshaped while dragging", |h| {
                drawn(h);
                h.server.state.titles.shapings.get() > before
            });
            h.button(BTN_LEFT, ButtonState::Released);
            let layout = &mut h.server.state.windows.layout;
            assert!(layout.snap(WindowId(1), ZoneId::LEFT));
            h.server.state.arrange();
        });
        set(&mut c, "Files - snapped");
        server.on_server(|h| {
            let before = h.server.state.titles.shapings.get();
            h.run_until("reshaped while snapped", |h| {
                drawn(h);
                h.server.state.titles.shapings.get() > before
            });
            let layout = &mut h.server.state.windows.layout;
            assert!(layout.snap(WindowId(1), ZoneId::FULL));
            h.server.state.arrange();
            assert!(
                drawn(h).is_some(),
                "a maximized window keeps its bar and title"
            );
        });
        w.toplevel.set_fullscreen(None);
        set(&mut c, "Files - fullscreen");
        server.on_server(|h| {
            assert_eq!(
                h.server.state.windows.layout.fullscreen(),
                Some(WindowId(1))
            );
            assert!(drawn(h).is_none(), "fullscreen: no bar, no title");
        });
        w.toplevel.unset_fullscreen();
        set(&mut c, "");
        server.on_server(|h| {
            h.run_until("the empty title shows", |h| {
                h.step();
                drawn(h).is_none()
            });
        });
    });
}

/// The title area is part of the bar: a press there starts a move.
#[test]
fn the_title_area_is_the_drag_handle() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        w.toplevel.set_title("Files".into());
        floating(&server, 1);
        c.roundtrip();
        server.on_server(|h| {
            let area = Decoration::FULL.title_rect(FLOAT).expect("title area");
            h.pointer_to(f64::from(area.x + 2), f64::from(area.y + 10));
            h.button(BTN_LEFT, ButtonState::Pressed);
            assert_eq!(h.server.state.dragged_window(), Some(WindowId(1)));
            h.pointer_to(f64::from(area.x + 52), f64::from(area.y + 40));
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(FLOAT.x + 50, FLOAT.y + 30, FLOAT.w, FLOAT.h))
            );
        });
    });
}

/// CPU time of the calling thread, in clock ticks (Linux).
fn thread_ticks() -> u64 {
    let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat
        .rsplit(')')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    // utime and stime are fields 14 and 15; after the ")" they are at 11 and 12.
    fields
        .get(11)
        .and_then(|f| f.parse::<u64>().ok())
        .unwrap_or(0)
        + fields
            .get(12)
            .and_then(|f| f.parse::<u64>().ok())
            .unwrap_or(0)
}

/// A client changing its title 10 000 times a second: at most 20 shapings a second,
/// frames only for those, and the compositor's CPU time stays low.
#[test]
fn title_spam_is_coalesced() {
    let mut h = Harness::new();
    let socket = h.socket_path();
    let (tx, rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let (sent_tx, sent_rx) = std::sync::mpsc::channel();
    let client = std::thread::spawn(move || {
        let (mut c, w, _d) = decorated(&socket, None);
        w.toplevel.set_title("start".into());
        c.roundtrip();
        tx.send(()).unwrap();
        // Wait until the window floats (the server thread floats it).
        c.roundtrip();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let start = Instant::now();
        let mut n = 0u32;
        while start.elapsed().as_secs_f64() < 2.0 {
            for _ in 0..100 {
                w.toplevel.set_title(format!("Spam {n}"));
                n += 1;
            }
            let _ = c.conn.flush();
            let target = start + std::time::Duration::from_micros(u64::from(n) * 100);
            if let Some(wait) = target.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        w.toplevel.set_title("Spam final".into());
        c.roundtrip();
        sent_tx.send((n, start.elapsed())).unwrap();
        // Stay connected until the server checked the final title.
        let _ = done_rx.recv();
    });
    h.run_until("connected", |_| rx.try_recv().is_ok());
    h.chord(&[super::harness::keys::ALT, super::harness::keys::F]);
    h.server
        .state
        .windows
        .layout
        .set_floating_rect(WindowId(1), FLOAT);
    h.server.state.arrange();
    let (frames0, shapings0, ticks0, t0) = (
        h.frames,
        h.server.state.titles.shapings.get(),
        thread_ticks(),
        Instant::now(),
    );
    let mut result = None;
    h.run_until("the spam", |_| {
        result = sent_rx.try_recv().ok();
        result.is_some()
    });
    let (sent, took) = result.expect("sent");
    let elapsed = t0.elapsed().as_secs_f64();
    let shapings = h.server.state.titles.shapings.get() - shapings0;
    let frames = h.frames - frames0;
    let ticks = thread_ticks() - ticks0;
    let hz = 100.0; // CLK_TCK on Linux
    eprintln!(
        "title spam: {sent} titles in {took:?} ({:.0}/s); {shapings} shapings, {frames} frames in {elapsed:.2} s; server thread CPU {:.0}% of a core",
        f64::from(sent) / took.as_secs_f64(),
        100.0 * ticks as f64 / hz / elapsed
    );
    assert!(
        f64::from(sent) / took.as_secs_f64() > 9000.0,
        "the client spammed"
    );
    assert!(
        shapings as f64 <= 20.0 * elapsed + 3.0,
        "{shapings} shapings in {elapsed:.2} s"
    );
    assert!(
        frames as f64 <= 20.0 * elapsed + 10.0,
        "{frames} frames in {elapsed:.2} s"
    );
    // The last title wins (the timer brings it in).
    h.run_until("the final title", |h| {
        drawn(h);
        h.server.state.titles.is_current(WindowId(1))
    });
    done_tx.send(()).unwrap();
    h.run_client(client);
}

/// Bar redraw time (run in release with --ignored --nocapture): a new title (shape and
/// draw), a width change (cut and draw, no shaping), an unchanged title (cached).
#[test]
#[ignore = "measurement: cargo test --release -- --ignored --nocapture title_redraw_time"]
fn title_redraw_time() {
    let titles = titles::Titles::default();
    let median = |mut v: Vec<std::time::Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    let id = WindowId(1);
    let mut new_title = Vec::new();
    let mut resized = Vec::new();
    let mut unchanged = Vec::new();
    let mut now = Instant::now();
    for i in 0..200 {
        // 60 ms apart (beyond the 50 ms shaping interval).
        now += std::time::Duration::from_millis(60);
        titles.set_for_tests(
            id,
            ClientText::from(format!("Settings - Displays and Monitors {i}").as_str()),
        );
        let t = Instant::now();
        titles.prepare(id, 310, true, 1.0, now);
        new_title.push(t.elapsed());
        let t = Instant::now();
        titles.prepare(id, 250 + i % 50, true, 1.0, now);
        resized.push(t.elapsed());
        let t = Instant::now();
        titles.prepare(id, 250 + i % 50, true, 1.0, now);
        unchanged.push(t.elapsed());
    }
    eprintln!(
        "bar title, 13 px, scale 1: new title {:?}, width change {:?}, unchanged {:?} (medians of 200)",
        median(new_title),
        median(resized),
        median(unchanged)
    );
}
