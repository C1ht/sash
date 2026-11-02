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
        renderer::{ImportDma, damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    input::pointer::CursorImageStatus,
    output::Output,
    reexports::{
        calloop::{EventLoop, ping::PingSource},
        winit::{dpi::LogicalSize, platform::wayland::WindowAttributesExtWayland, window::Window},
    },
    utils::{Physical, Size, Transform},
};

use crate::{dmabuf, keybinds::Modifier, output, render, server::Server, state::Sash};

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
    /// The cursor last applied to the host window: its generation, and whether it
    /// was a live cursor surface (a dying one falls back to the arrow).
    cursor_applied: Option<(u64, bool)>,
    /// Number of offscreen captures written (debug builds).
    #[cfg(debug_assertions)]
    captures: u32,
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
        cursor_applied: None,
        #[cfg(debug_assertions)]
        captures: 0,
    }));

    let offer = dmabuf::offer(nested.borrow_mut().backend.renderer());
    if let Some((main_device, formats)) = offer {
        let for_imports = nested.clone();
        state.enable_dmabuf(
            main_device,
            formats,
            Box::new(move |buffer| {
                let mut nested = for_imports.borrow_mut();
                nested
                    .backend
                    .renderer()
                    .import_dmabuf(buffer, None)
                    .is_ok()
            }),
        );
    }

    let handle = event_loop.handle();
    let for_redraws = nested.clone();
    handle
        .insert_source(redraw_requests, move |_, _, _| {
            for_redraws.borrow().backend.window().request_redraw();
        })
        .map_err(|err| err.error)?;
    #[cfg(debug_assertions)]
    {
        use calloop::signals::{Signal, Signals};
        let for_capture = nested.clone();
        handle
            .insert_source(Signals::new(&[Signal::SIGUSR1])?, move |_, _, server| {
                for_capture.borrow_mut().capture(&server.state);
            })
            .map_err(|err| err.error)?;
    }
    handle
        .insert_source(host_events, move |event, _, server| {
            nested.borrow_mut().handle(event, &mut server.state);
        })
        .map_err(|err| err.error)?;

    Ok(())
}

impl Nested {
    /// Writes the current output to $SASH_CAPTURE_DIR/sash-wm-capture-<n>.png.
    #[cfg(debug_assertions)]
    fn capture(&mut self, state: &Sash) {
        use crate::capture::{CAPTURE_DIR_VAR, write_png};
        use smithay::backend::renderer::gles::GlesTexture;
        let Some(dir) = std::env::var_os(CAPTURE_DIR_VAR) else {
            tracing::warn!("SIGUSR1: set {CAPTURE_DIR_VAR} to capture the output");
            return;
        };
        self.captures += 1;
        let path =
            std::path::Path::new(&dir).join(format!("sash-wm-capture-{}.png", self.captures));
        let size = self.backend.window_size();
        let start = std::time::Instant::now();
        match write_png::<_, GlesTexture>(self.backend.renderer(), state, size, &path) {
            Ok(()) => tracing::info!(
                path = %path.display(),
                w = size.w,
                h = size.h,
                ms = start.elapsed().as_millis() as u64,
                "captured output"
            ),
            Err(err) => tracing::error!(%err, "capture failed"),
        }
    }

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
                state.flush_pending_arrange();
                let input_since = state.windows.input_pending_since.take();
                record!(state, "frame_start");
                let times = match self.render(state) {
                    Ok(times) => times,
                    Err(err) => {
                        tracing::error!(%err, counters = ?self.counters, "rendering failed");
                        return;
                    }
                };
                self.counters.frames_rendered += 1;
                // From sash-wm handling the input (a drag motion, a keybind) to the frame
                // showing its effect being submitted, including the wait for the host.
                let input_to_frame_us = input_since.map(|t| t.elapsed().as_micros() as u64);
                tracing::debug!(
                    frame_time_us = times.total_us(),
                    render_us = times.render_us,
                    submit_us = times.submit_us,
                    input_to_frame_us,
                    counters = ?self.counters,
                    "frame"
                );
                if self.counters.frames_rendered == 1 {
                    tracing::info!(
                        startup_ms = state.started.elapsed().as_secs_f64() * 1000.0,
                        "first frame submitted"
                    );
                }
                #[cfg(feature = "debug-record")]
                self.record_frame(state, &times);
                state.frame_presented(&self.output);
            }
            WinitEvent::Input(event) => state.handle_input(event),
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            WinitEvent::Focus(_) => {}
        }
    }

    /// Debug recording: the frame's end and a screenshot if one is due. Nested, a named
    /// cursor is the host's (sash-wm draws only cursor surfaces).
    #[cfg(feature = "debug-record")]
    fn record_frame(&mut self, state: &mut Sash, times: &FrameTimes) {
        if state.recorder.is_none() {
            return;
        }
        let surface = state.cursor_surface().is_some();
        let pointer = state.seat.get_pointer().map(|p| p.current_location());
        if let Some(recorder) = state.recorder.as_mut() {
            crate::record::Counters::bump(&recorder.counters.frames_rendered);
            if surface && let Some(p) = pointer {
                recorder.cursor_drawn((p.x.round() as i32, p.y.round() as i32));
            }
            recorder.push(
                "frame_end",
                vec![
                    ("queued", true.into()),
                    ("render_us", times.render_us.into()),
                    ("submit_us", times.submit_us.into()),
                    ("cursor", if surface { "composited" } else { "host" }.into()),
                ],
            );
        }
        let size = self.backend.window_size();
        crate::record::after_frame::<_, smithay::backend::renderer::gles::GlesTexture>(
            state,
            self.backend.renderer(),
            size,
        );
    }

    /// Shows the cursor clients asked for on the host window: a named shape, or no
    /// host cursor while it is hidden or a surface sash-wm draws itself.
    fn apply_cursor(&mut self, state: &Sash) {
        let image = state.cursor.image();
        let applied = (
            state.cursor.generation(),
            matches!(image, CursorImageStatus::Surface(_)),
        );
        if self.cursor_applied == Some(applied) {
            return;
        }
        self.cursor_applied = Some(applied);
        let window = self.backend.window();
        match image {
            CursorImageStatus::Named(icon) => {
                window.set_cursor(icon);
                window.set_cursor_visible(true);
            }
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => {
                window.set_cursor_visible(false)
            }
        }
    }

    fn render(&mut self, state: &Sash) -> Result<FrameTimes, Box<dyn std::error::Error>> {
        self.apply_cursor(state);
        let start = Instant::now();
        let age = self.backend.buffer_age().unwrap_or(0);
        let damage = {
            let (renderer, mut framebuffer) = self.backend.bind()?;
            let mut elements = render::cursor_elements(state, renderer, 1.0);
            elements.extend(render::output_elements(state, renderer, 1.0));
            let result = self.damage.render_output(
                renderer,
                &mut framebuffer,
                age,
                &elements,
                render::CLEAR_COLOR,
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
