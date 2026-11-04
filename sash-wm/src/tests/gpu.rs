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
    crate::capture::write_png::<_, GlesTexture>(
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
    crate::capture::write_png::<_, GlesTexture>(
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
        server.on_server(|h| {
            h.chord(&[ALT, crate::tests::harness::keys::SHIFT, F]); // float all
            for i in 0..10 {
                let rect = crate::layout::Rect::new(20 + 40 * i, 20 + 30 * i, 360, 260);
                h.server
                    .state
                    .windows
                    .layout
                    .set_floating_rect(crate::layout::WindowId(i as u64 + 1), rect);
            }
            h.server.state.focus_window(crate::layout::WindowId(10));
            h.server.state.arrange();
            h.pointer_to(560.0, 300.0);
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
            crate::capture::write_png::<_, GlesTexture>(
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
