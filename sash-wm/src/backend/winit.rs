// SPDX-License-Identifier: Apache-2.0

//! Nested backend: sash-wm runs as a window inside another Wayland or X11 session.

use std::time::Instant;

use smithay::{
    backend::{
        renderer::{Frame, Renderer, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    reexports::{
        calloop::EventLoop,
        winit::{dpi::LogicalSize, platform::wayland::WindowAttributesExtWayland, window::Window},
    },
    utils::{Rectangle, Transform},
};

use crate::state::Sash;

const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

/// Counts used to check that every resize leads to exactly one rendered frame.
#[derive(Debug, Default)]
struct Counters {
    resizes_received: u64,
    redraws_received: u64,
    frames_rendered: u64,
}

pub fn init(
    event_loop: &EventLoop<'static, Sash>,
    _state: &mut Sash,
) -> Result<(), Box<dyn std::error::Error>> {
    // `with_name` sets the Wayland app id and, on X11, WM_CLASS.
    let attributes = Window::default_attributes()
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_title("Sash")
        .with_name("sash-wm", "sash-wm");
    let (mut backend, winit) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    let mut counters = Counters::default();

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            tracing::trace!(?event, "winit event");
            match event {
                WinitEvent::Resized { size, .. } => {
                    counters.resizes_received += 1;
                    tracing::debug!(w = size.w, h = size.h, ?counters, "resize");
                    backend.window().request_redraw();
                }
                WinitEvent::Redraw => {
                    counters.redraws_received += 1;
                    let frame_start = Instant::now();
                    if let Err(err) = render(&mut backend) {
                        tracing::error!(%err, ?counters, "rendering failed");
                        return;
                    }
                    counters.frames_rendered += 1;
                    tracing::debug!(
                        frame_time_us = frame_start.elapsed().as_micros() as u64,
                        ?counters,
                        "frame"
                    );
                    if counters.frames_rendered == 1 {
                        tracing::info!(
                            startup_ms = state.start_time.elapsed().as_secs_f64() * 1000.0,
                            "first frame submitted"
                        );
                    }
                }
                WinitEvent::CloseRequested => state.loop_signal.stop(),
                _ => {}
            }
        })
        .map_err(|err| err.error)?;

    Ok(())
}

fn render(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
) -> Result<(), Box<dyn std::error::Error>> {
    let size = backend.window_size();
    let damage = Rectangle::from_size(size);
    {
        let (renderer, mut framebuffer) = backend.bind()?;
        let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
        frame.clear(CLEAR_COLOR.into(), &[damage])?;
        // Buffer swap in `submit` synchronizes with the GPU; the explicit sync point is unused.
        let _ = frame.finish()?;
    }
    backend.submit(Some(&[damage]))?;
    Ok(())
}
