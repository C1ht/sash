// SPDX-License-Identifier: Apache-2.0

//! Tests that render with a real GPU (headless: an EGL device, no window, never the
//! host screen). Ignored by default; run with
//! `SASH_CAPTURE_DIR=<dir> cargo test -p sash-wm -- --ignored --nocapture gpu`.

use std::time::{Duration, Instant};

use smithay::{
    backend::input::{ButtonState, KeyState},
    backend::{
        allocator::Fourcc,
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::{
            Bind, Offscreen,
            damage::OutputDamageTracker,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    utils::{Size, Transform},
};

use super::{
    client::TestClient,
    harness::{
        Harness,
        keys::{ALT, F},
    },
};
use crate::{render, seat::BTN_LEFT};

const COLORS: [u32; 3] = [0xff_c0_39_2b, 0xff_27_ae_60, 0xff_8e_44_ad];

fn renderer() -> GlesRenderer {
    let device = EGLDevice::enumerate()
        .expect("EGL device enumeration")
        .find(|d| d.render_device_path().is_ok())
        .expect("a GPU render node");
    // Safety: the display and context live as long as the renderer that owns them.
    let display = unsafe { EGLDisplay::new(device) }.expect("EGL display");
    let context = EGLContext::new(&display).expect("EGL context");
    unsafe { GlesRenderer::new(context) }.expect("GLES renderer")
}

/// Three coloured windows, 1 | (2 / 3); window 3 floats and is being dragged with its
/// pointer in the left zone (button still held), so the snap preview shows.
fn drag_into_left_zone(harness: &mut Harness) {
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut clients = Vec::new();
        for color in COLORS {
            let mut c = TestClient::connect(&socket);
            let mut w = c.create_window_without_commit();
            w.color = color;
            w.surface.commit();
            c.dispatch_until("configure", |e| e.configures >= 1);
            c.ack_and_commit(&w);
            clients.push((c, w));
        }
        server.on_server(|h| {
            h.chord(&[ALT, F]); // window 3 floats at its tile (400, 300, 400, 300)
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(600.0, 450.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(2.0, 300.0);
        });
        // A floating window redraws at its new size after the configure.
        for (c, w) in &mut clients {
            c.roundtrip();
            c.ack_and_commit(w);
        }
        std::mem::forget(clients); // keep the connections for the rest of the test
    });
    for _ in 0..20 {
        harness.step();
    }
}

#[test]
#[ignore = "needs a GPU"]
#[cfg(debug_assertions)]
fn gpu_capture_snap_preview() {
    let dir = std::env::var_os(crate::capture::CAPTURE_DIR_VAR).expect("set SASH_CAPTURE_DIR");
    let mut harness = Harness::new();
    drag_into_left_zone(&mut harness);
    assert!(
        harness.server.state.overlay.preview().is_some(),
        "preview is showing"
    );
    let mut renderer = renderer();
    let path = std::path::Path::new(&dir).join("m5-snap-preview.png");
    crate::capture::write_png(
        &mut renderer,
        &harness.server.state,
        (800, 600).into(),
        &path,
    )
    .expect("capture");
    println!("wrote {}", path.display());
}

/// Full-frame render time of the same scene with and without the snap preview.
#[test]
#[ignore = "needs a GPU"]
fn gpu_overlay_cost() {
    let mut harness = Harness::new();
    drag_into_left_zone(&mut harness);
    let mut renderer = renderer();
    let size: Size<i32, smithay::utils::Physical> = (800, 600).into();
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (800, 600).into())
        .expect("offscreen buffer");
    let preview = harness.server.state.overlay_preview_for_bench();
    let mut measure = |with_preview: bool, renderer: &mut GlesRenderer| {
        harness
            .server
            .state
            .overlay
            .set_preview(with_preview.then_some(preview).flatten());
        let mut times = Vec::new();
        for _ in 0..300 {
            let start = Instant::now();
            let elements = render::output_elements(&harness.server.state, renderer, 1.0);
            let mut framebuffer = renderer.bind(&mut target).expect("bind");
            let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
            let result = damage
                .render_output(
                    renderer,
                    &mut framebuffer,
                    0,
                    &elements,
                    render::CLEAR_COLOR,
                )
                .expect("render");
            result.sync.wait().expect("GPU done");
            times.push(start.elapsed());
        }
        times.sort();
        times
    };
    let without = measure(false, &mut renderer);
    let with = measure(true, &mut renderer);
    let q = |t: &[Duration], p: f64| t[((t.len() - 1) as f64 * p) as usize].as_micros();
    for (name, t) in [("without preview", &without), ("with preview", &with)] {
        println!(
            "{name}: n={} p50={}us p95={}us p99={}us max={}us",
            t.len(),
            q(t, 0.5),
            q(t, 0.95),
            q(t, 0.99),
            t.last().unwrap().as_micros()
        );
    }
}

/// Prints what the GPU's EGL can import as dmabufs: formats with their modifiers, and
/// the EGL extensions dmabuf and sync depend on.
#[test]
#[ignore = "needs a GPU"]
fn gpu_dmabuf_formats() {
    use smithay::backend::renderer::ImportDma;
    let device = EGLDevice::enumerate()
        .expect("EGL device enumeration")
        .find(|d| d.render_device_path().is_ok())
        .expect("a GPU render node");
    println!("render node: {:?}", device.render_device_path());
    // Safety: as in `renderer`.
    let display = unsafe { EGLDisplay::new(device) }.expect("EGL display");
    for ext in display.extensions() {
        if ext.contains("dma_buf") || ext.contains("sync") || ext.contains("modifier") {
            println!("EGL extension: {ext}");
        }
    }
    let context = EGLContext::new(&display).expect("EGL context");
    let renderer = unsafe { GlesRenderer::new(context) }.expect("GLES renderer");
    let mut by_code = std::collections::BTreeMap::<String, Vec<String>>::new();
    for format in renderer.dmabuf_formats() {
        by_code
            .entry(format!("{:?}", format.code))
            .or_default()
            .push(format!("{:#x}", u64::from(format.modifier)));
    }
    println!("{} formats:", by_code.len());
    for (code, modifiers) in by_code {
        println!("  {code}: {}", modifiers.join(" "));
    }
}

/// A `size` dmabuf of one colour, made on the GPU, as a client would send it.
#[cfg(debug_assertions)]
fn colour_dmabuf(
    renderer: &mut GlesRenderer,
    size: (i32, i32),
    colour: [f32; 4],
) -> super::client::DmabufPlane {
    use smithay::backend::{
        allocator::Buffer,
        renderer::{Color32F, Frame, Renderer},
    };
    let dmabuf = crate::dmabuf::export_texture(renderer, Fourcc::Abgr8888, size.into(), |r, t| {
        let Ok(mut framebuffer) = r.bind(t) else {
            return false;
        };
        let physical: Size<i32, smithay::utils::Physical> = size.into();
        let Ok(mut frame) = r.render(&mut framebuffer, physical, Transform::Normal) else {
            return false;
        };
        let all = smithay::utils::Rectangle::from_size(physical);
        let [red, green, blue, alpha] = colour;
        frame
            .clear(Color32F::new(red, green, blue, alpha), &[all])
            .is_ok()
            && frame.finish().is_ok_and(|sync| sync.wait().is_ok())
    })
    .expect("export a texture as a dmabuf");
    assert_eq!(dmabuf.num_planes(), 1, "one-plane format");
    super::client::DmabufPlane {
        fd: dmabuf
            .handles()
            .next()
            .expect("a plane")
            .try_clone_to_owned()
            .expect("dup the plane fd"),
        offset: dmabuf.offsets().next().expect("offset"),
        stride: dmabuf.strides().next().expect("stride"),
        modifier: dmabuf.format().modifier.into(),
        fourcc: dmabuf.format().code as u32,
        size,
    }
}

/// Real dmabufs end to end: a tiled window and a popup whose buffers are dmabufs made
/// on the GPU, next to a shm window, rendered through the capture path and sampled.
/// Run: `SASH_CAPTURE_DIR=<dir> cargo test -p sash-wm -- --ignored --nocapture gpu_dmabuf`.
#[test]
#[ignore = "needs a GPU"]
#[cfg(debug_assertions)]
fn gpu_dmabuf_windows_and_popup() {
    use super::client::PopupSpec;
    use smithay::backend::renderer::ImportDma;
    let dir = std::env::var_os(crate::capture::CAPTURE_DIR_VAR).expect("set SASH_CAPTURE_DIR");
    let mut renderer = renderer();
    let red = colour_dmabuf(&mut renderer, (400, 600), [1.0, 0.0, 0.0, 1.0]);
    let blue = colour_dmabuf(&mut renderer, (50, 40), [0.0, 0.0, 1.0, 1.0]);
    let formats = renderer.dmabuf_formats().into_iter().collect();
    let mut harness = Harness::with_dmabuf_formats(formats);
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.create_window_without_commit();
        w1.surface.commit();
        c1.dispatch_until("configure", |e| e.configures >= 1);
        c1.ack_and_commit(&w1);
        let mut c2 = TestClient::connect(&socket);
        let mut w2 = c2.create_window_without_commit();
        w2.color = 0xff_00_ff_00; // green, wl_shm
        w2.surface.commit();
        c2.dispatch_until("configure", |e| e.configures >= 1);
        c2.ack_and_commit(&w2); // 1 | 2
        c1.roundtrip();
        // Window 1 shows the red dmabuf at its 400x600 tile.
        let serial = c1.events.configure_serial.expect("configured");
        w1.xdg_surface.ack_configure(serial);
        let buffer = c1.dmabuf_buffer(&red);
        w1.surface.attach(Some(&buffer), 0, 0);
        w1.surface.damage_buffer(0, 0, 400, 600);
        w1.surface.commit();
        c1.roundtrip();
        // A blue dmabuf popup on window 2 at (410, 20).
        let menu = c2.create_popup(&w2.xdg_surface, PopupSpec::default(), None);
        c2.dispatch_until("popup configure", |e| e.configures >= 2);
        let serial = c2.events.configure_serial.expect("popup configured");
        menu.xdg_surface.ack_configure(serial);
        let buffer = c2.dmabuf_buffer(&blue);
        menu.surface.attach(Some(&buffer), 0, 0);
        menu.surface.damage_buffer(0, 0, 50, 40);
        menu.surface.commit();
        c2.roundtrip();
        std::mem::forget((c1, c2, w1, w2, menu));
    });
    for _ in 0..5 {
        harness.step();
    }
    let path = std::path::Path::new(&dir).join("m8-dmabuf.png");
    crate::capture::write_png(
        &mut renderer,
        &harness.server.state,
        (800, 600).into(),
        &path,
    )
    .expect("capture");
    println!("wrote {}", path.display());
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&path).expect("png"),
    ));
    let mut reader = decoder.read_info().expect("png header");
    let mut pixels = vec![0; reader.output_buffer_size().expect("size")];
    reader.next_frame(&mut pixels).expect("png data");
    let at = |x: usize, y: usize| {
        let i = (y * 800 + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    assert_eq!(at(200, 300), [255, 0, 0], "dmabuf window");
    assert_eq!(at(600, 300), [0, 255, 0], "shm window");
    assert_eq!(at(430, 40), [0, 0, 255], "dmabuf popup");
    assert_eq!(
        at(405, 300),
        [0, 255, 0],
        "the dmabuf window is clipped to its tile"
    );
}

/// Renders the output and the cursor (as the DRM backend composes a frame) into an
/// offscreen buffer and returns its RGBA pixels.
#[cfg(debug_assertions)]
fn frame_with_cursor(renderer: &mut GlesRenderer, state: &crate::state::Sash) -> Vec<u8> {
    use smithay::backend::renderer::ExportMem;
    use smithay::utils::{Buffer, Physical, Rectangle};
    let size: Size<i32, Physical> = (800, 600).into();
    let buffer_size = Size::<i32, Buffer>::from((800, 600));
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buffer_size)
        .expect("offscreen buffer");
    let mut elements = render::cursor_elements(state, renderer, 1.0);
    elements.extend(render::output_elements(state, renderer, 1.0));
    let mapping = {
        let mut framebuffer = renderer.bind(&mut target).expect("bind");
        let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        damage
            .render_output(
                renderer,
                &mut framebuffer,
                0,
                &elements,
                render::CLEAR_COLOR,
            )
            .expect("render");
        renderer
            .copy_framebuffer(
                &framebuffer,
                Rectangle::from_size(buffer_size),
                Fourcc::Abgr8888,
            )
            .expect("copy")
    };
    renderer.map_texture(&mapping).expect("map").to_vec()
}

/// Renderer recovery (M11a): after the renderer is rebuilt (a new EGL context and
/// renderer on the same display, the old one dropped), a shm window, a dmabuf window and
/// a client cursor surface are drawn exactly as before, with no client committing
/// anything in between. Run: `cargo test -p sash-wm -- --ignored gpu_rebuild`.
#[test]
#[ignore = "needs a GPU"]
#[cfg(debug_assertions)]
fn gpu_rebuild_restores_windows_and_cursor_without_client_help() {
    use smithay::backend::renderer::ImportDma;
    let device = EGLDevice::enumerate()
        .expect("EGL device enumeration")
        .find(|d| d.render_device_path().is_ok())
        .expect("a GPU render node");
    // Safety: the display outlives every context made on it in this test.
    let display = unsafe { EGLDisplay::new(device) }.expect("EGL display");
    let new_renderer = || {
        let context = EGLContext::new(&display).expect("EGL context");
        unsafe { GlesRenderer::new(context) }.expect("GLES renderer")
    };
    // The client's GPU work happens on a renderer of its own.
    let mut client_gpu = new_renderer();
    let red = colour_dmabuf(&mut client_gpu, (400, 600), [1.0, 0.0, 0.0, 1.0]);
    let mut first = new_renderer();
    let formats = first.dmabuf_formats().into_iter().collect();
    let mut harness = Harness::with_dmabuf_formats(formats);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.create_window_without_commit();
        w1.surface.commit();
        c1.dispatch_until("configure", |e| e.configures >= 1);
        c1.ack_and_commit(&w1);
        let mut c2 = TestClient::connect(&socket);
        let mut w2 = c2.create_window_without_commit();
        w2.color = 0xff_00_ff_00; // green, wl_shm
        w2.surface.commit();
        c2.dispatch_until("configure", |e| e.configures >= 1);
        c2.ack_and_commit(&w2); // 1 | 2
        c1.roundtrip();
        let serial = c1.events.configure_serial.expect("configured");
        w1.xdg_surface.ack_configure(serial);
        let buffer = c1.dmabuf_buffer(&red);
        w1.surface.attach(Some(&buffer), 0, 0);
        w1.surface.damage_buffer(0, 0, 400, 600);
        w1.surface.commit();
        c1.roundtrip();
        // A white 16x16 cursor surface over window 1, hotspot at its top left.
        server.on_server(|h| h.pointer_to(100.0, 100.0));
        c1.roundtrip();
        let _cursor = c1.set_cursor_surface((16, 16), (0, 0));
        std::mem::forget((c1, c2, w1, w2, _cursor));
    });
    for _ in 0..5 {
        harness.step();
    }
    let state = &harness.server.state;
    let before = frame_with_cursor(&mut first, state);
    drop(first); // the old renderer, with every texture it imported, is gone
    let mut rebuilt = new_renderer();
    let after = frame_with_cursor(&mut rebuilt, state);
    let at = |pixels: &[u8], x: usize, y: usize| {
        let i = (y * 800 + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    for pixels in [&before, &after] {
        assert_eq!(at(pixels, 200, 300), [255, 0, 0], "dmabuf window");
        assert_eq!(at(pixels, 600, 300), [0, 255, 0], "shm window");
        assert_eq!(at(pixels, 105, 105), [255, 255, 255], "cursor surface");
    }
    assert!(before == after, "the rebuilt renderer draws the same frame");
}

/// Ten coloured windows, floating and overlapping in a cascade; with `server_side`
/// they negotiated decorations (bar and border), else they have the border only. The
/// pointer rests on window 10's bar; window 10 has focus.
fn ten_floating(harness: &mut Harness, server_side: bool) {
    ten_floating_on(harness, server_side, (800, 600));
}

/// `ten_floating` on an output of `size`: the same cascade scaled to it (at 800x600,
/// windows of 360x260 every 40 px right and 30 px down).
fn ten_floating_on(harness: &mut Harness, server_side: bool, (ow, oh): (i32, i32)) {
    harness.server.state.set_layout_area((ow, oh).into());
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut clients = Vec::new();
        for i in 0..10u32 {
            let mut c = TestClient::connect(&socket);
            let mut w = c.create_window_without_commit();
            w.color = 0xff00_0000 | (0x1f * (i + 1)) << 16 | 0x4080;
            if server_side {
                std::mem::forget(c.decorate(&w, None));
            }
            w.surface.commit();
            c.dispatch_until("configure", |e| e.configures >= 1);
            c.ack_and_commit(&w);
            clients.push((c, w));
        }
        server.on_server(move |h| {
            h.chord(&[ALT, crate::tests::harness::keys::SHIFT, F]); // float all
            for i in 0..10 {
                let rect = crate::layout::Rect::new(
                    20 + ow / 20 * i,
                    20 + oh / 20 * i,
                    ow * 9 / 20,
                    oh * 13 / 30,
                );
                h.server
                    .state
                    .windows
                    .layout
                    .set_floating_rect(crate::layout::WindowId(i as u64 + 1), rect);
            }
            h.server.state.focus_window(crate::layout::WindowId(10));
            h.server.state.arrange();
            h.pointer_to(f64::from(ow) * 0.7, f64::from(oh) * 0.5);
        });
        for (c, w) in &mut clients {
            c.roundtrip();
            c.ack_and_commit(w);
        }
        std::mem::forget(clients);
    });
    for _ in 0..20 {
        harness.step();
    }
}

fn percentiles(name: &str, mut t: Vec<Duration>) {
    t.sort();
    let q = |p: f64| t[((t.len() - 1) as f64 * p) as usize].as_micros();
    println!(
        "{name}: n={} p50={}us p95={}us p99={}us max={}us",
        t.len(),
        q(0.5),
        q(0.95),
        q(0.99),
        t.last().unwrap().as_micros()
    );
}

/// M11b measurements: full-frame render time with ten windows with bars (and with the
/// border only), frame times during a bar drag (damage tracked from frame to frame, as
/// the backends do), and pointer hit-testing on the decorations. With SASH_CAPTURE_DIR
/// set, also writes the decorated scene as a PNG (solid-colour test windows only).
#[test]
#[ignore = "needs a GPU"]
fn gpu_decorations_cost() {
    let size: Size<i32, smithay::utils::Physical> = (800, 600).into();
    let mut renderer = renderer();
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (800, 600).into())
        .expect("offscreen buffer");
    for server_side in [false, true] {
        let mut harness = Harness::new();
        ten_floating(&mut harness, server_side);
        let mut times = Vec::new();
        for _ in 0..300 {
            let start = Instant::now();
            let elements = render::output_elements(&harness.server.state, &mut renderer, 1.0);
            let mut framebuffer = renderer.bind(&mut target).expect("bind");
            let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
            let result = damage
                .render_output(
                    &mut renderer,
                    &mut framebuffer,
                    0,
                    &elements,
                    render::CLEAR_COLOR,
                )
                .expect("render");
            result.sync.wait().expect("GPU done");
            times.push(start.elapsed());
        }
        let name = if server_side {
            "full frame, 10 windows with bars"
        } else {
            "full frame, 10 windows, border only"
        };
        percentiles(name, times);
        if !server_side {
            continue;
        }
        #[cfg(debug_assertions)]
        if let Some(dir) = std::env::var_os(crate::capture::CAPTURE_DIR_VAR) {
            let path = std::path::Path::new(&dir).join("m11b-ten-bars.png");
            crate::capture::write_png(
                &mut renderer,
                &harness.server.state,
                (800, 600).into(),
                &path,
            )
            .expect("capture");
            println!("wrote {}", path.display());
        }
        // Hit-testing: what is under the pointer, decorations included, at points
        // spread over the output.
        let state = &harness.server.state;
        let mut hits = Vec::new();
        let mut found = 0;
        for i in 0..20_000u32 {
            let at = (((i * 37) % 800) as f64 + 0.5, ((i * 53) % 600) as f64 + 0.5).into();
            let start = Instant::now();
            let decoration = state.decoration_at(at);
            let surface = state.surface_at(at);
            hits.push(start.elapsed());
            found += decoration.is_some() as u32 + surface.is_some() as u32;
        }
        assert!(found > 0);
        percentiles("hit test (decoration_at + surface_at)", hits);
        // A bar drag: window 10 grabbed by its bar and moved in a circle, one frame
        // per motion, the damage tracker kept.
        harness.button(BTN_LEFT, ButtonState::Pressed);
        assert!(harness.pointer_grabbed(), "a bar drag");
        let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        let mut frames = Vec::new();
        for step in 0..300 {
            let t = step as f64 / 60.0;
            let x = 560.0 - 100.0 + 100.0 * (2.0 * std::f64::consts::PI * t).cos();
            let y = 300.0 + 100.0 * (2.0 * std::f64::consts::PI * t).sin();
            let start = Instant::now();
            harness.pointer_to(x, y);
            harness.server.state.flush_pending_arrange();
            let elements = render::output_elements(&harness.server.state, &mut renderer, 1.0);
            let mut framebuffer = renderer.bind(&mut target).expect("bind");
            let result = damage
                .render_output(
                    &mut renderer,
                    &mut framebuffer,
                    0,
                    &elements,
                    render::CLEAR_COLOR,
                )
                .expect("render");
            result.sync.wait().expect("GPU done");
            frames.push(start.elapsed());
        }
        harness.button(BTN_LEFT, ButtonState::Released);
        percentiles("bar drag frame (motion, configure, render)", frames);
    }
}

/// Two floating windows with server-side decorations and rounded corners (M15b step
/// 2): window 1 a green wl_shm buffer (focused), window 2 a red dmabuf. Their outer
/// rectangles are `ROUNDED_RECTS`; titles are empty and the pointer is away from both.
/// With `abgr`, window 1's buffer is wl_shm ABGR8888 (the same green).
#[cfg(debug_assertions)]
const ROUNDED_RECTS: [(i32, i32, i32, i32); 2] = [(40, 40, 302, 235), (400, 300, 302, 235)];

#[cfg(debug_assertions)]
fn rounded_scene(renderer: &mut GlesRenderer, abgr: bool) -> Harness {
    use smithay::backend::renderer::ImportDma;
    // Client size: outer minus 1 px borders and the 32 px bar.
    let red = colour_dmabuf(renderer, (300, 201), [1.0, 0.0, 0.0, 1.0]);
    let formats = renderer.dmabuf_formats().into_iter().collect();
    let mut harness = Harness::with_dmabuf_formats(formats);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        c1.abgr_buffers = abgr;
        let mut w1 = c1.create_window_without_commit();
        w1.color = 0xff_00_ff_00;
        w1.toplevel.set_title(String::new());
        std::mem::forget(c1.decorate(&w1, None));
        w1.surface.commit();
        c1.dispatch_until("configure", |e| e.configures >= 1);
        c1.ack_and_commit(&w1);
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.create_window_without_commit();
        w2.toplevel.set_title(String::new());
        std::mem::forget(c2.decorate(&w2, None));
        w2.surface.commit();
        c2.dispatch_until("configure", |e| e.configures >= 1);
        c2.ack_and_commit(&w2);
        server.on_server(|h| {
            h.chord(&[ALT, crate::tests::harness::keys::SHIFT, F]); // float all
            let layout = &mut h.server.state.windows.layout;
            layout.set_radius(crate::layout::shape::RADIUS);
            for (i, (x, y, w, hh)) in ROUNDED_RECTS.into_iter().enumerate() {
                layout.set_floating_rect(
                    crate::layout::WindowId(i as u64 + 1),
                    crate::layout::Rect::new(x, y, w, hh),
                );
            }
            h.server.state.focus_window(crate::layout::WindowId(1));
            h.server.state.arrange();
            h.pointer_to(790.0, 10.0);
        });
        c1.roundtrip();
        c1.ack_and_commit(&w1);
        c2.roundtrip();
        let serial = c2.events.serials[&wayland_client::Proxy::id(&w2.xdg_surface)];
        w2.xdg_surface.ack_configure(serial);
        let buffer = c2.dmabuf_buffer(&red);
        w2.surface.attach(Some(&buffer), 0, 0);
        w2.surface.damage_buffer(0, 0, 300, 201);
        w2.surface.commit();
        c2.roundtrip();
        std::mem::forget((c1, c2, w1, w2));
    });
    for _ in 0..10 {
        harness.step();
    }
    harness
}

/// Renders the output at `scale` into an offscreen buffer: RGBA rows.
#[cfg(debug_assertions)]
fn frame_at_scale(
    renderer: &mut GlesRenderer,
    state: &crate::state::Sash,
    scale: f64,
) -> (usize, usize, Vec<u8>) {
    use smithay::backend::renderer::ExportMem;
    use smithay::utils::{Buffer, Physical, Rectangle};
    let (w, h) = (
        (800.0 * scale).round() as i32,
        (600.0 * scale).round() as i32,
    );
    let size: Size<i32, Physical> = (w, h).into();
    let buffer_size = Size::<i32, Buffer>::from((w, h));
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buffer_size)
        .expect("offscreen buffer");
    let elements = render::output_elements(state, renderer, scale);
    let mapping = {
        let mut framebuffer = renderer.bind(&mut target).expect("bind");
        let mut damage = OutputDamageTracker::new(size, scale, Transform::Normal);
        damage
            .render_output(
                renderer,
                &mut framebuffer,
                0,
                &elements,
                render::CLEAR_COLOR,
            )
            .expect("render");
        renderer
            .copy_framebuffer(
                &framebuffer,
                Rectangle::from_size(buffer_size),
                Fourcc::Abgr8888,
            )
            .expect("copy")
    };
    (
        w as usize,
        h as usize,
        renderer.map_texture(&mapping).expect("map").to_vec(),
    )
}

/// What the CPU reference (layout::shape) says a pixel of window `id` looks like: the
/// background, the client's colour masked by the shape inside the client rectangle,
/// the bar clipped by the shape, the ring on top. None where something else is drawn
/// (the buttons).
#[cfg(debug_assertions)]
fn reference_pixel(
    state: &crate::state::Sash,
    id: crate::layout::WindowId,
    content: [f32; 3],
    background: [f32; 3],
    scale: f64,
    (x, y): (usize, usize),
) -> [f32; 3] {
    use crate::decorations::{BAR_FILL, BORDER_FOCUSED, BORDER_UNFOCUSED};
    let layout = &state.windows.layout;
    let shape = layout.shape(id, scale as f32).expect("a rounded window");
    let outer = layout.shown_rect(id).expect("shown");
    let decoration = layout.decoration(id);
    let phys = |r: crate::layout::Rect| {
        let x0 = (r.x as f64 * scale).round();
        let y0 = (r.y as f64 * scale).round();
        (
            x0,
            y0,
            x0 + (r.w as f64 * scale).round(),
            y0 + (r.h as f64 * scale).round(),
        )
    };
    let inside =
        |r: (f64, f64, f64, f64), px: f64, py: f64| px >= r.0 && px < r.2 && py >= r.1 && py < r.3;
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let cov = shape.coverage(px, py);
    let mut c = background;
    let blend = |c: &mut [f32; 3], over: [f32; 3], a: f32| {
        for i in 0..3 {
            c[i] = over[i] * a + c[i] * (1.0 - a);
        }
    };
    if inside(phys(decoration.inner(outer)), px as f64, py as f64) {
        blend(&mut c, content, cov);
    }
    if let Some(bar) = decoration.bar_rect(outer)
        && inside(phys(bar), px as f64, py as f64)
    {
        blend(
            &mut c,
            [BAR_FILL[0], BAR_FILL[1], BAR_FILL[2]].map(|v| v * 255.0),
            cov,
        );
    }
    let ring = if layout.focused() == Some(id) {
        BORDER_FOCUSED
    } else {
        BORDER_UNFOCUSED
    };
    let rc = shape.ring_coverage(
        px,
        py,
        crate::layout::decor::BORDER as f32 * scale as f32,
        decoration.border,
    );
    blend(&mut c, [ring[0], ring[1], ring[2]].map(|v| v * 255.0), rc);
    c
}

/// Rounded corners on the GPU against the CPU reference (M15b step 2), at scales 1,
/// 1.5 and 2: the three corners without buttons of a shm and a dmabuf window, pixel by
/// pixel within a tolerance; the cut-off corner shows the background. Then the same
/// frame from a rebuilt renderer: the masks survive a rebuild. With SASH_CAPTURE_DIR
/// set, writes the frames (solid-colour test windows only).
/// Run: `cargo test -p sash-wm -- --ignored --nocapture gpu_rounded`.
#[test]
#[ignore = "needs a GPU"]
#[cfg(debug_assertions)]
fn gpu_rounded_corners_match_the_cpu_reference() {
    let device = EGLDevice::enumerate()
        .expect("EGL device enumeration")
        .find(|d| d.render_device_path().is_ok())
        .expect("a GPU render node");
    // Safety: the display outlives every context made on it in this test.
    let display = unsafe { EGLDisplay::new(device) }.expect("EGL display");
    let new_renderer = || {
        let context = EGLContext::new(&display).expect("EGL context");
        unsafe { GlesRenderer::new(context) }.expect("GLES renderer")
    };
    let mut client_gpu = new_renderer();
    let harness = rounded_scene(&mut client_gpu, false);
    let state = &harness.server.state;
    let mut renderer = new_renderer();
    const TOLERANCE: f32 = 3.0;
    for scale in [1.0, 1.5, 2.0] {
        let (w, _h, pixels) = frame_at_scale(&mut renderer, state, scale);
        let at = |x: usize, y: usize| {
            let i = (y * w + x) * 4;
            [pixels[i] as f32, pixels[i + 1] as f32, pixels[i + 2] as f32]
        };
        let background = at(2, 2);
        let mut worst = 0.0f32;
        let mut compared = 0;
        for (i, content) in [[0.0, 255.0, 0.0], [255.0, 0.0, 0.0]]
            .into_iter()
            .enumerate()
        {
            let id = crate::layout::WindowId(i as u64 + 1);
            let (ox, oy, ow, oh) = ROUNDED_RECTS[i];
            let s = |v: i32| (v as f64 * scale).round() as usize;
            let boxed = s(crate::layout::shape::RADIUS + 3);
            // Top right, bottom right, bottom left (top left holds the buttons).
            let corners = [
                (s(ox + ow) - boxed, s(oy)),
                (s(ox + ow) - boxed, s(oy + oh) - boxed),
                (s(ox), s(oy + oh) - boxed),
            ];
            for (cx, cy) in corners {
                for y in cy..cy + boxed {
                    for x in cx..cx + boxed {
                        let want = reference_pixel(state, id, content, background, scale, (x, y));
                        let got = at(x, y);
                        let diff = (0..3).map(|k| (want[k] - got[k]).abs()).fold(0.0, f32::max);
                        worst = worst.max(diff);
                        compared += 1;
                        assert!(
                            diff <= TOLERANCE,
                            "x{scale}, window {}: pixel {x},{y} is {got:?}, the reference {want:?}",
                            i + 1
                        );
                    }
                }
            }
            // The outermost corner pixel is cut off: background.
            assert_eq!(
                at(s(ox + ow) - 1, s(oy + oh) - 1),
                background,
                "x{scale}: cut-off corner"
            );
        }
        println!("x{scale}: {compared} pixels compared, worst difference {worst}");
        if let Some(dir) = std::env::var_os(crate::capture::CAPTURE_DIR_VAR) {
            let png = crate::capture::encode_png(&pixels, w as u32, (pixels.len() / 4 / w) as u32)
                .expect("png");
            let path = std::path::Path::new(&dir).join(format!("m15b-rounded-x{scale}.png"));
            std::fs::write(&path, png).expect("write");
            println!("wrote {}", path.display());
        }
    }
    let before = frame_at_scale(&mut renderer, state, 1.0);
    drop(renderer);
    let mut rebuilt = new_renderer();
    let after = frame_at_scale(&mut rebuilt, state, 1.0);
    assert!(
        before == after,
        "a rebuilt renderer draws the same rounded frame"
    );
}

/// M15b step 2 measurement: full-frame render time with ten floating windows with bars
/// (overlapping, all corners rounded), square (radius 0) and rounded (the constant),
/// on the headless GPU path; and the frames after a pointer motion over a bar (damage
/// tracked from frame to frame, as the backends do).
/// Run: `cargo test -p sash-wm -- --ignored --nocapture gpu_rounded_cost`.
#[test]
#[ignore = "needs a GPU"]
fn gpu_rounded_cost() {
    let size: Size<i32, smithay::utils::Physical> = (800, 600).into();
    let mut renderer = renderer();
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (800, 600).into())
        .expect("offscreen buffer");
    let mut harness = Harness::new();
    ten_floating(&mut harness, true);
    // Alternating, twice each, so warm-up does not favour either; the first rounded
    // frame compiles the shaders (its max).
    let rounded = crate::layout::shape::RADIUS;
    for radius in [0, rounded, 0, rounded] {
        harness.server.state.windows.layout.set_radius(radius);
        let mut full = Vec::new();
        for _ in 0..300 {
            let start = Instant::now();
            let elements = render::output_elements(&harness.server.state, &mut renderer, 1.0);
            let mut framebuffer = renderer.bind(&mut target).expect("bind");
            let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
            let result = damage
                .render_output(
                    &mut renderer,
                    &mut framebuffer,
                    0,
                    &elements,
                    render::CLEAR_COLOR,
                )
                .expect("render");
            result.sync.wait().expect("GPU done");
            full.push(start.elapsed());
        }
        percentiles(
            &format!("full frame, 10 windows with bars, radius {radius}"),
            full,
        );
        // Damage-tracked frames while the hovered button changes (a pointer moving
        // over window 10's bar), so only the decorations are redrawn.
        let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        let mut tracked = Vec::new();
        for i in 0..300 {
            // Window 10's close button, then its bar.
            let x = if i % 2 == 0 { 396.0 } else { 560.0 };
            harness.server.state.decoration_hover((x, 306.0).into());
            let start = Instant::now();
            let elements = render::output_elements(&harness.server.state, &mut renderer, 1.0);
            let mut framebuffer = renderer.bind(&mut target).expect("bind");
            let result = damage
                .render_output(
                    &mut renderer,
                    &mut framebuffer,
                    0,
                    &elements,
                    render::CLEAR_COLOR,
                )
                .expect("render");
            result.sync.wait().expect("GPU done");
            tracked.push(start.elapsed());
        }
        percentiles(
            &format!("hover frames, damage tracked, radius {radius}"),
            tracked,
        );
    }
}

/// After M15b (owner): full-frame render time with ten windows with bars, square and
/// rounded, at the owner's 5120x1440, at 3840x1080 and at 1920x1080, against the
/// 4.17 ms budget of a 240 Hz frame. Every frame is drawn whole (a new damage tracker
/// each time) and waited for on the GPU.
/// Run: `cargo test --release -p sash-wm -- --ignored --nocapture gpu_rounded_cost_by_resolution`.
#[test]
#[ignore = "needs a GPU"]
fn gpu_rounded_cost_by_resolution() {
    let mut renderer = renderer();
    for (w, h) in [(5120, 1440), (3840, 1080), (1920, 1080)] {
        let size: Size<i32, smithay::utils::Physical> = (w, h).into();
        let mut target: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (w, h).into())
            .expect("offscreen buffer");
        let mut harness = Harness::new();
        ten_floating_on(&mut harness, true, (w, h));
        // Alternating, twice each; the first rounded run also compiles the shaders.
        let rounded = crate::layout::shape::RADIUS;
        for radius in [0, rounded, 0, rounded] {
            harness.server.state.windows.layout.set_radius(radius);
            let mut times = Vec::new();
            for _ in 0..300 {
                let start = Instant::now();
                let elements = render::output_elements(&harness.server.state, &mut renderer, 1.0);
                let mut framebuffer = renderer.bind(&mut target).expect("bind");
                let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
                let result = damage
                    .render_output(
                        &mut renderer,
                        &mut framebuffer,
                        0,
                        &elements,
                        render::CLEAR_COLOR,
                    )
                    .expect("render");
                result.sync.wait().expect("GPU done");
                times.push(start.elapsed());
            }
            percentiles(
                &format!("{w}x{h}, full frame, 10 windows with bars, radius {radius}"),
                times,
            );
        }
    }
}

/// wl_shm ABGR8888 (sash-ui's byte order, M17 E1) through the real renderer: the
/// rounded scene with window 1's buffer in ABGR8888 renders exactly like the same scene
/// in ARGB8888, at scales 1, 1.5 and 2: imported, masked by the rounded-corner shader,
/// next to a dmabuf window in the same frame. A colour swap would turn the green
/// window's pixels into another colour.
/// Run: `cargo test -p sash-wm -- --ignored --nocapture gpu_abgr`.
#[test]
#[ignore = "needs a GPU"]
#[cfg(debug_assertions)]
fn gpu_abgr8888_renders_like_argb8888() {
    let mut client_gpu = renderer();
    let argb = rounded_scene(&mut client_gpu, false);
    let abgr = rounded_scene(&mut client_gpu, true);
    let mut gpu = renderer();
    for scale in [1.0, 1.5, 2.0] {
        let (w, h, a) = frame_at_scale(&mut gpu, &argb.server.state, scale);
        let (_, _, b) = frame_at_scale(&mut gpu, &abgr.server.state, scale);
        let differing = a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count();
        println!("ABGR8888 vs ARGB8888 at scale {scale}: {w}x{h}, {differing} pixels differ");
        assert_eq!(differing, 0, "scale {scale}");
        // The shm window's centre is green in both (not swapped to blue or red).
        let (cx, cy) = (
            ((ROUNDED_RECTS[0].0 + ROUNDED_RECTS[0].2 / 2) as f64 * scale) as usize,
            ((ROUNDED_RECTS[0].1 + ROUNDED_RECTS[0].3 / 2) as f64 * scale) as usize,
        );
        let i = (cy * w + cx) * 4;
        assert_eq!(
            &b[i..i + 4],
            &[0, 255, 0, 255],
            "scale {scale}: centre of window 1"
        );
        let _ = h;
    }
}

/// A sash-ui surface for the upload measurement: an opaque fill and a 64x64 square.
struct UploadView {
    square: sash_ui::Rect,
}

impl sash_ui::View for UploadView {
    fn draw(&mut self, canvas: &mut sash_ui::Canvas) {
        let (w, h) = canvas.size();
        canvas.fill(
            sash_ui::Rect::sized(w as i32, h as i32),
            sash_ui::Color::rgb(0x2C2C2E),
        );
        canvas.fill(self.square, sash_ui::Color::rgb(0x0A84FF));
    }
}

thread_local! {
    /// The renderer and damage tracker of the upload measurement, on the server thread.
    static UPLOAD_GPU: std::cell::RefCell<Option<(GlesRenderer, GlesTexture, OutputDamageTracker)>> =
        const { std::cell::RefCell::new(None) };
}

/// M17 E1: what a sash-ui surface's wl_shm ABGR8888 upload costs sash-wm on a
/// 5120x1440 output, for a 5120x64 bottom panel and a full-screen 5120x1440 surface,
/// each with full damage on every frame and with a 64x64 change. Per frame: the client
/// draws (CPU, timed by sash-ui) and commits; sash-wm builds the render elements (which
/// imports the buffer: a full texture upload, or only the damaged rectangles) and
/// waits for the GPU (glFinish), then renders the frame with a damage tracker kept
/// from frame to frame (as the backends do) and waits again.
/// Run: `cargo test --release -p sash-wm -- --ignored --nocapture gpu_shm_upload_cost`.
#[test]
#[ignore = "needs a GPU"]
fn gpu_shm_upload_cost() {
    use sash_ui::{Edges, Layer, Panel, PanelSpec, Rect as UiRect};
    const OUT: (i32, i32) = (5120, 1440);
    let all = Edges {
        top: true,
        bottom: true,
        left: true,
        right: true,
    };
    let cases = [
        ("panel 5120x64", Layer::Top, Edges::BOTTOM_BAR, (0, 64), 64),
        ("full screen 5120x1440", Layer::Background, all, (0, 0), 0),
    ];
    for (name, layer, anchor, size, zone) in cases {
        for full in [true, false] {
            let mut harness = Harness::new();
            harness.server.state.set_layout_area(OUT.into());
            UPLOAD_GPU.with(|gpu| {
                let mut renderer = renderer();
                let target: GlesTexture = renderer
                    .create_buffer(Fourcc::Abgr8888, OUT.into())
                    .expect("offscreen buffer");
                let tracker = OutputDamageTracker::new(OUT, 1.0, Transform::Normal);
                *gpu.borrow_mut() = Some((renderer, target, tracker));
            });
            let stream = harness.server.state.privileged_connection_for_tests();
            let spec = PanelSpec {
                layer,
                anchor,
                size,
                exclusive_zone: zone,
                margin: 0,
                namespace: "sash-upload".into(),
                keyboard: sash_ui::Keyboard::None,
            };
            let label = format!(
                "{name}, {}",
                if full { "full damage" } else { "64x64 damage" }
            );
            harness.run_with_remote(move |server| {
                let view = UploadView {
                    square: UiRect::new(0, 0, 64, 64),
                };
                let mut panel = Panel::connect(stream, spec, view).expect("panel");
                let (tx, rx) = std::sync::mpsc::channel::<(Duration, Duration)>();
                let mut draws = Vec::new();
                let mut uploads = Vec::new();
                let mut frames = Vec::new();
                for i in 0..120u64 {
                    // The first commit may already have happened while connecting.
                    let target = if i == 0 { 1 } else { panel.stats().commits + 1 };
                    if i > 0 {
                        let old = panel.view().square;
                        let x = (old.x + 64) % (OUT.0 - 64);
                        panel.view_mut().square = UiRect::new(x, 0, 64, 64);
                        if full {
                            panel.damage_all();
                        } else {
                            panel.damage(old);
                            panel.damage(panel.view().square);
                        }
                    }
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while panel.stats().commits < target || panel.stats().frame_callbacks < target {
                        assert!(
                            Instant::now() < deadline,
                            "{label}: frame {i}: no commit: {:?}",
                            panel.stats()
                        );
                        panel
                            .dispatch(Some(Duration::from_millis(1)))
                            .expect("panel");
                    }
                    let sent = tx.clone();
                    server.on_server(move |h| {
                        UPLOAD_GPU.with(|gpu| {
                            let mut gpu = gpu.borrow_mut();
                            let (renderer, target, tracker) = gpu.as_mut().expect("gpu");
                            let start = Instant::now();
                            let elements = render::output_elements(&h.server.state, renderer, 1.0);
                            renderer
                                .with_context(|gl| unsafe { gl.Finish() })
                                .expect("finish");
                            let upload = start.elapsed();
                            let mut framebuffer = renderer.bind(target).expect("bind");
                            let result = tracker
                                .render_output(
                                    renderer,
                                    &mut framebuffer,
                                    0,
                                    &elements,
                                    render::CLEAR_COLOR,
                                )
                                .expect("render");
                            result.sync.wait().expect("GPU done");
                            sent.send((upload, start.elapsed())).expect("times");
                        });
                    });
                    let (upload, frame) = rx.recv().expect("times");
                    // The first frame creates the textures; it is reported apart.
                    if i == 0 {
                        println!(
                            "{label}: first frame: draw {}us, import {}us, frame {}us",
                            panel.stats().last_draw.as_micros(),
                            upload.as_micros(),
                            frame.as_micros()
                        );
                        continue;
                    }
                    draws.push(panel.stats().last_draw);
                    uploads.push(upload);
                    frames.push(frame);
                }
                percentiles(&format!("{label}: client draw (CPU)"), draws);
                percentiles(&format!("{label}: sash-wm import + finish"), uploads);
                percentiles(
                    &format!("{label}: sash-wm import + render + finish"),
                    frames,
                );
            });
            UPLOAD_GPU.with(|gpu| *gpu.borrow_mut() = None);
        }
    }
}

/// sash-ui's test panel (M17 E1) through the real renderer: the bottom 48 rows of the
/// frame are exactly the panel's own buffer (opaque, scale 1: the GPU copies what the
/// CPU drew, ABGR8888 to the screen without a swap or a blend).
/// Run: `cargo test -p sash-wm -- --ignored --nocapture gpu_panel`.
#[test]
#[ignore = "needs a GPU"]
#[cfg(debug_assertions)]
fn gpu_panel_shows_its_own_pixels() {
    use super::panel::{HEIGHT, TestPanel, pump, spec};
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    let expected = harness.run_with_remote(move |_| {
        let mut panel = sash_ui::Panel::connect(stream, spec(), TestPanel::new()).expect("panel");
        pump(&mut panel, "the first frame callback", |p| {
            p.stats().frame_callbacks == 1
        });
        let expected = panel.view_mut().expected();
        // The connection stays open while the frame is rendered.
        std::mem::forget(panel);
        expected
    });
    for _ in 0..5 {
        harness.step();
    }
    let mut gpu = renderer();
    let (w, h, frame) = frame_at_scale(&mut gpu, &harness.server.state, 1.0);
    let rows = &frame[(h - HEIGHT as usize) * w * 4..];
    let differing = rows
        .chunks(4)
        .zip(expected.chunks(4))
        .filter(|(a, b)| a != b)
        .count();
    println!(
        "panel on the GPU: {} pixels, {differing} differ from its buffer",
        rows.len() / 4
    );
    assert_eq!(rows.len(), expected.len());
    assert_eq!(differing, 0);
}

/// The E3 contact sheet: every Tabler glyph sash-ui embeds, as icon buttons in a
/// test panel, captured through sash-wm's offscreen path (owner, M17 E3).
/// Run: `SASH_CAPTURE_DIR=<dir> cargo test -p sash-wm -- --ignored --nocapture gpu_capture_icon`.
#[test]
#[ignore = "needs a GPU"]
#[cfg(debug_assertions)]
fn gpu_capture_icon_contact_sheet() {
    use std::{cell::RefCell, rc::Rc};

    use sash_ui::{
        Edges, IconLoader, Keyboard, Layer, Node, Panel, PanelSpec, Size, Surface, Symbol, Theme,
        Ui,
        icon::SymbolIcon,
        tree::{Align, Insets},
    };

    let dir = std::env::var_os(crate::capture::CAPTURE_DIR_VAR).expect("set SASH_CAPTURE_DIR");
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |_| {
        let loader = Rc::new(RefCell::new(IconLoader::new(vec![], vec![], "none", false)));
        let mut ui = Ui::new(
            Node::column()
                .padding(Insets::all(4))
                .width(Size::Flex(1))
                .height(Size::Flex(1)),
            Theme::dark(),
            Surface::Surface,
        );
        let root = ui.root();
        for row in Symbol::ALL.chunks(10) {
            let r = ui.add(
                root,
                Node::row()
                    .spacing(8)
                    .padding(Insets::all(4))
                    .align(Align::Center)
                    .group(),
            );
            for &symbol in row {
                ui.add(
                    r,
                    Node::icon_button(SymbolIcon::new(loader.clone(), symbol), symbol.label())
                        .icon_size(24),
                );
            }
        }
        let spec = PanelSpec {
            layer: Layer::Top,
            anchor: Edges::BOTTOM_BAR,
            size: (0, 88),
            exclusive_zone: 88,
            margin: 0,
            namespace: "sash-ui icon contact sheet (tests only)".into(),
            keyboard: Keyboard::None,
        };
        let mut panel = Panel::connect(stream, spec, ui).expect("panel");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while panel.stats().frame_callbacks == 0 {
            assert!(std::time::Instant::now() < deadline, "no frame");
            panel
                .dispatch(Some(std::time::Duration::from_millis(5)))
                .expect("panel");
        }
        // The connection stays open while the frame is captured.
        std::mem::forget(panel);
    });
    for _ in 0..5 {
        harness.step();
    }
    let mut renderer = renderer();
    let path = std::path::Path::new(&dir).join("m17-e3-icon-contact-sheet.png");
    crate::capture::write_png(
        &mut renderer,
        &harness.server.state,
        (800, 600).into(),
        &path,
    )
    .expect("capture");
    println!("wrote {}", path.display());
}
