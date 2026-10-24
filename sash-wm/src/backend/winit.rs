// SPDX-License-Identifier: Apache-2.0

//! Nested backend: sash-wm runs as a window inside another Wayland or X11 session.
//!
//! Frames are drawn only when the host asks for one (winit's redraw event). winit
//! holds redraws back until the host's frame callback for the previous frame
//! arrives, so a hidden window draws nothing and clients get no frame callbacks.

use std::{cell::RefCell, rc::Rc, time::Instant};

use smithay::{
    backend::{
        egl::ffi::egl,
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::Output,
    reexports::{
        calloop::{EventLoop, ping::PingSource},
        winit::{dpi::LogicalSize, platform::wayland::WindowAttributesExtWayland, window::Window},
    },
    utils::{Physical, Size, Transform},
};

use crate::{keybinds::Modifier, output, render, server::Server, state::Sash};

const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

/// Keybind modifier when nested: the host compositor usually captures Super.
pub const MODIFIER: Modifier = Modifier::Alt;

/// Counts used to check that every resize leads to a rendered frame.
#[derive(Debug, Default)]
struct Counters {
    resizes_received: u64,
    redraws_received: u64,
    frames_rendered: u64,
}

struct Nested {
    backend: WinitGraphicsBackend<GlesRenderer>,
    output: Output,
    damage: OutputDamageTracker,
    counters: Counters,
}

/// The GL framebuffer of the winit window is upside down relative to the output.
fn damage_tracker(size: Size<i32, Physical>) -> OutputDamageTracker {
    OutputDamageTracker::new(size, 1.0, Transform::Flipped180)
}

/// Sets the EGL swap interval of the window surface to 0.
///
/// Smithay leaves it at EGL's default of 1, which makes every buffer swap wait for
/// the host's frame callback inside EGL, on top of winit's own frame callback
/// throttling. Measured on Hyprland: 30 fps instead of 60, with the event loop blocked
/// ~17 ms per frame. Frame pacing comes from winit's redraw events alone.
fn disable_swap_wait(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
) -> Result<(), Box<dyn std::error::Error>> {
    let context = backend.renderer().egl_context();
    let display = context.display().get_display_handle();
    let context = context.get_context_handle();
    let surface = backend.egl_surface().get_surface_handle();
    // Safety: all handles come from the live backend, and the swap interval applies to
    // the surface current on this thread, which MakeCurrent sets just before.
    let ok = unsafe {
        egl::MakeCurrent(display.handle, surface, surface, context) == egl::TRUE
            && egl::SwapInterval(display.handle, 0) == egl::TRUE
    };
    if !ok {
        // Safety: plain error query on this thread.
        let error = unsafe { egl::GetError() };
        return Err(
            format!("setting the EGL swap interval to 0 failed: EGL error {error:#x}").into(),
        );
    }
    Ok(())
}

pub fn start(
    event_loop: &EventLoop<'static, Server>,
    state: &mut Sash,
    redraw_requests: PingSource,
) -> Result<(), Box<dyn std::error::Error>> {
    // `with_name` sets the Wayland app id and, on X11, WM_CLASS.
    let attributes = Window::default_attributes()
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_title("Sash")
        .with_name("sash-wm", "sash-wm");
    let (mut backend, host_events) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    disable_swap_wait(&mut backend)?;

    let size = backend.window_size();
    let output = output::create(state, "nested", size);
    state.set_layout_area(size.to_logical(1));
    let nested = Rc::new(RefCell::new(Nested {
        output,
        damage: damage_tracker(size),
        backend,
        counters: Counters::default(),
    }));

    let handle = event_loop.handle();
    let for_redraws = nested.clone();
    handle
        .insert_source(redraw_requests, move |_, _, _| {
            for_redraws.borrow().backend.window().request_redraw();
        })
        .map_err(|err| err.error)?;
    handle
        .insert_source(host_events, move |event, _, server| {
            nested.borrow_mut().handle(event, &mut server.state);
        })
        .map_err(|err| err.error)?;

    Ok(())
}

impl Nested {
    fn handle(&mut self, event: WinitEvent, state: &mut Sash) {
        tracing::trace!(?event, "winit event");
        match event {
            WinitEvent::Resized { size, .. } => {
                self.counters.resizes_received += 1;
                tracing::debug!(w = size.w, h = size.h, counters = ?self.counters, "resize");
                output::set_size(&self.output, size);
                state.set_layout_area(size.to_logical(1));
                self.damage = damage_tracker(size);
                self.backend.window().request_redraw();
            }
            WinitEvent::Redraw => {
                self.counters.redraws_received += 1;
                let times = match self.render(state) {
                    Ok(times) => times,
                    Err(err) => {
                        tracing::error!(%err, counters = ?self.counters, "rendering failed");
                        return;
                    }
                };
                self.counters.frames_rendered += 1;
                tracing::debug!(
                    frame_time_us = times.total_us(),
                    render_us = times.render_us,
                    submit_us = times.submit_us,
                    counters = ?self.counters,
                    "frame"
                );
                if self.counters.frames_rendered == 1 {
                    tracing::info!(
                        startup_ms = state.started.elapsed().as_secs_f64() * 1000.0,
                        "first frame submitted"
                    );
                }
                state.frame_presented(&self.output);
            }
            WinitEvent::Input(event) => state.handle_input(event),
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            WinitEvent::Focus(_) => {}
        }
    }

    fn render(&mut self, state: &Sash) -> Result<FrameTimes, Box<dyn std::error::Error>> {
        let start = Instant::now();
        let age = self.backend.buffer_age().unwrap_or(0);
        let damage = {
            let (renderer, mut framebuffer) = self.backend.bind()?;
            let elements = render::tile_elements(state, renderer, 1.0);
            let result = self.damage.render_output(
                renderer,
                &mut framebuffer,
                age,
                &elements,
                CLEAR_COLOR,
            )?;
            result.damage.cloned()
        };
        let rendered = Instant::now();
        self.backend.submit(damage.as_deref())?;
        Ok(FrameTimes {
            render_us: (rendered - start).as_micros() as u64,
            submit_us: rendered.elapsed().as_micros() as u64,
        })
    }
}

/// Time spent drawing a frame and handing it to the host (buffer swap).
struct FrameTimes {
    render_us: u64,
    submit_us: u64,
}

impl FrameTimes {
    fn total_us(&self) -> u64 {
        self.render_us + self.submit_us
    }
}
