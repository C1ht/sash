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
