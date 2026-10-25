// SPDX-License-Identifier: Apache-2.0

//! DRM/KMS backend: sash-wm directly on the hardware, started from a text console with
//! `--backend drm`.
//!
//! - Session: libseat (logind or seatd) opens the GPU and input devices; Ctrl+Alt+Fn
//!   switch VTs through it before any keybind sees the keys (seat.rs).
//! - One GPU (udev's primary GPU), one connector at its preferred mode (mode.rs).
//! - GBM + EGL + GLES render into a Smithay `DrmCompositor`, which uses atomic
//!   modesetting when the driver has it and puts the cursor on the hardware cursor
//!   plane when it fits (else it is drawn with everything else).
//! - Pacing (pacing.rs): render only after the previous page flip completed; frame
//!   callbacks and presentation-time feedback when a flip completes.
//! - Session pause and resume (session.rs): stop rendering, give up input and the
//!   device; take them back, reset the device state, redraw.
//! - Input (input.rs): libinput with fixed defaults; the keymap follows XKB_DEFAULT_*.
//!
//! Hotplug is out of scope: udev events are logged, nothing more.

mod input;
mod mode;
mod pacing;
mod session;

use std::{cell::RefCell, error::Error, path::PathBuf, rc::Rc, time::Duration};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
        },
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportDma, element::memory::MemoryRenderBuffer, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::utils::{
        OutputPresentationFeedback, surface_primary_scanout_output,
        take_presentation_feedback_surface_tree,
    },
    output::Output,
    reexports::{
        calloop::{EventLoop, LoopHandle, ping::PingSource},
        drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
    },
    utils::{Clock, DeviceFd, Monotonic},
    wayland::presentation::Refresh,
};

use self::{
    mode::{ConnectorInfo, ModeInfo},
    pacing::Pacer,
    session::{SessionTracker, Step},
};
use crate::{dmabuf, keybinds::Modifier, output, render, server::Server, state::Sash};

/// Keybind modifier on real hardware.
pub const MODIFIER: Modifier = Modifier::Super;

type Compositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

struct Drm {
    #[expect(
        dead_code,
        reason = "keeps the seat session open while the backend runs"
    )]
    session: LibSeatSession,
    device: DrmDevice,
    compositor: Compositor,
    renderer: GlesRenderer,
    output: Output,
    libinput: Libinput,
    pacer: Pacer,
    tracker: SessionTracker,
    devices: input::Devices,
    arrow: MemoryRenderBuffer,
    /// Duration of one refresh at the chosen mode.
    refresh: Duration,
}

/// Starts the DRM backend. Any error before the event loop runs drops what was opened
/// (the device and session are released) and is returned with a message.
pub fn start(
    event_loop: &EventLoop<'static, Server>,
    state: &mut Sash,
    redraw_requests: PingSource,
) -> Result<(), Box<dyn Error>> {
    let (mut session, session_events) = LibSeatSession::new()
        .map_err(|err| format!("could not open a seat session (libseat): {err}"))?;
    let seat = session.seat();
    let path = choose_gpu(&seat)?;
    tracing::info!(seat, gpu = %path.display(), "DRM backend");

    let fd = session
        .open(
            &path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|err| format!("could not open {}: {err}", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (mut device, device_events) =
        DrmDevice::new(fd.clone(), true).map_err(|err| format!("DRM device: {err}"))?;
    let gbm = GbmDevice::new(fd.clone()).map_err(|err| format!("GBM device: {err}"))?;
    // Safety: the display and context live as long as the renderer that owns them.
    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|err| format!("EGL: {err}"))?;
    let context = EGLContext::new(&display).map_err(|err| format!("EGL context: {err}"))?;
    let mut renderer =
        unsafe { GlesRenderer::new(context) }.map_err(|err| format!("GLES renderer: {err}"))?;

    let (connector, drm_mode, crtc) = choose_output(&device)?;
    let info = device.get_connector(connector, false)?;
    let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
    let size_mm = info.size().map_or((0, 0), |(w, h)| (w as i32, h as i32));
    let (width, height) = drm_mode.size();
    tracing::info!(
        connector = name,
        mode = format!("{width}x{height}@{}", drm_mode.vrefresh()),
        "output"
    );
    let surface = device
        .create_surface(crtc, drm_mode, &[connector])
        .map_err(|err| format!("DRM surface on {name}: {err}"))?;
    let output = output::create_physical(state, &name, size_mm, drm_mode.into());
    state.set_layout_area((width as i32, height as i32).into());

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    // No direct scan-out of client buffers yet: everything is composited.
    let exporter = GbmFramebufferExporter::new(gbm.clone(), None);
    let renderer_formats = renderer.egl_context().dmabuf_render_formats().clone();
    let compositor = DrmCompositor::new(
        &output,
        surface,
        None,
        allocator,
        exporter,
        [Fourcc::Xrgb8888, Fourcc::Argb8888],
        renderer_formats,
        device.cursor_size(),
        Some(gbm),
    )
    .map_err(|err| format!("DRM compositor: {err}"))?;

    let dmabuf_offer = dmabuf::offer(&mut renderer);
    state.enable_presentation();

    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| format!("libinput could not use seat {seat}"))?;
    let input_backend = LibinputInputBackend::new(libinput.clone());

    let refresh_mhz = drm_mode.vrefresh().max(1) as u64 * 1000;
    let drm = Rc::new(RefCell::new(Drm {
        session: session.clone(),
        device,
        compositor,
        renderer,
        output,
        libinput,
        pacer: Pacer::default(),
        tracker: SessionTracker::default(),
        devices: input::Devices::default(),
        arrow: render::arrow_buffer(),
        refresh: Duration::from_micros(1_000_000_000 / refresh_mhz),
    }));

    if let Some((main_device, formats)) = dmabuf_offer {
        let for_imports = drm.clone();
        state.enable_dmabuf(
            main_device,
            formats,
            Box::new(move |buffer| {
                let mut drm = for_imports.borrow_mut();
                drm.renderer.import_dmabuf(buffer, None).is_ok()
            }),
        );
    }

    let mut vt_session = session.clone();
    state.vt_switch = Some(Box::new(move |vt| {
        if let Err(err) = vt_session.change_vt(vt) {
            tracing::warn!(vt, %err, "VT switch failed");
        }
    }));

    insert_sources(
        &event_loop.handle(),
        &drm,
        redraw_requests,
        session_events,
        device_events,
        input_backend,
    )?;
    match UdevBackend::new(&seat) {
        Ok(udev) => {
            event_loop
                .handle()
                .insert_source(udev, |event, _, _| log_udev(event))
                .map_err(|err| err.error)?;
        }
        Err(err) => tracing::warn!(%err, "no udev monitor; hotplug events are not logged"),
    }

    // The first frame.
    drm.borrow_mut().render(state);
    Ok(())
}

/// The GPU to use: udev's primary GPU on the seat, else the first one. Only one GPU is
/// driven; others are listed and left alone.
fn choose_gpu(seat: &str) -> Result<PathBuf, Box<dyn Error>> {
    let all = all_gpus(seat).unwrap_or_default();
    if all.len() > 1 {
        tracing::warn!(?all, "several GPUs; only the primary one is used");
    }
    primary_gpu(seat)
        .ok()
        .flatten()
        .or_else(|| all.into_iter().next())
        .ok_or_else(|| format!("no GPU found on seat {seat}").into())
}

/// The connector, mode and CRTC to drive (see mode.rs for the choice).
fn choose_output(
    device: &DrmDevice,
) -> Result<
    (
        connector::Handle,
        smithay::reexports::drm::control::Mode,
        crtc::Handle,
    ),
    Box<dyn Error>,
> {
    let resources = device
        .resource_handles()
        .map_err(|err| format!("DRM resources: {err}"))?;
    let mut handles = Vec::new();
    let mut infos = Vec::new();
    for &handle in resources.connectors() {
        let Ok(info) = device.get_connector(handle, true) else {
            continue;
        };
        infos.push(ConnectorInfo {
            id: u32::from(handle),
            connected: info.state() == connector::State::Connected,
            modes: info
                .modes()
                .iter()
                .map(|m| ModeInfo {
                    width: m.size().0,
                    height: m.size().1,
                    refresh: m.vrefresh(),
                    preferred: m.mode_type().contains(ModeTypeFlags::PREFERRED),
                })
                .collect(),
        });
        handles.push((handle, info));
    }
    let (index, mode_index) = mode::choose(&infos).ok_or("no connected display")?;
    let (handle, info) = &handles[index];
    let drm_mode = info.modes()[mode_index];
    let crtc = info
        .encoders()
        .iter()
        .filter_map(|&encoder| device.get_encoder(encoder).ok())
        .flat_map(|encoder| resources.filter_crtcs(encoder.possible_crtcs()))
        .next()
        .ok_or("no CRTC for the connector")?;
    Ok((*handle, drm_mode, crtc))
}

fn insert_sources(
    handle: &LoopHandle<'static, Server>,
    drm: &Rc<RefCell<Drm>>,
    redraw_requests: PingSource,
    session_events: smithay::backend::session::libseat::LibSeatSessionNotifier,
    device_events: smithay::backend::drm::DrmDeviceNotifier,
    input_backend: LibinputInputBackend,
) -> Result<(), Box<dyn Error>> {
    let for_redraws = drm.clone();
    handle
        .insert_source(redraw_requests, move |_, _, server| {
            let mut drm = for_redraws.borrow_mut();
            if drm.pacer.want_frame() {
                drm.render(&mut server.state);
            }
        })
        .map_err(|err| err.error)?;

    let for_flips = drm.clone();
    handle
        .insert_source(device_events, move |event, metadata, server| match event {
            DrmEvent::VBlank(_) => {
                for_flips
                    .borrow_mut()
                    .flip_complete(&mut server.state, metadata.take());
            }
            DrmEvent::Error(err) => tracing::error!(%err, "DRM device error"),
        })
        .map_err(|err| err.error)?;

    let for_session = drm.clone();
    handle
        .insert_source(session_events, move |event, _, server| {
            let mut drm = for_session.borrow_mut();
            let steps = match event {
                SessionEvent::PauseSession => drm.tracker.pause(),
                SessionEvent::ActivateSession => drm.tracker.resume(),
            };
            tracing::info!(?event, ?steps, "session");
            for step in steps {
                drm.run(step, &mut server.state);
            }
        })
        .map_err(|err| err.error)?;

    let for_input = drm.clone();
    handle
        .insert_source(input_backend, move |event, _, server| match event {
            InputEvent::DeviceAdded { mut device } => {
                let name = device.name().to_string();
                if for_input.borrow_mut().devices.added(&mut device) {
                    tracing::info!(name, "input device added");
                }
            }
            InputEvent::DeviceRemoved { device } => {
                if for_input.borrow_mut().devices.removed(&device) {
                    tracing::info!(name = device.name(), "input device removed");
                }
            }
            event => server.state.handle_input(event),
        })
        .map_err(|err| err.error)?;
    Ok(())
}

fn log_udev(event: UdevEvent) {
    // Hotplug is out of scope: monitor and GPU changes are only logged.
    tracing::info!(?event, "udev event (hotplug is not handled)");
}

impl Drm {
    /// Renders a frame if anything changed and queues it for the next page flip.
    fn render(&mut self, state: &mut Sash) {
        if !self.pacer.is_active() {
            return;
        }
        state.flush_pending_arrange();
        let mut elements = render::cursor_elements(state, &mut self.renderer, 1.0);
        elements.extend(render::arrow_element(
            state,
            &mut self.renderer,
            &self.arrow,
        ));
        elements.extend(render::output_elements(state, &mut self.renderer, 1.0));
        let result = self.compositor.render_frame(
            &mut self.renderer,
            &elements,
            render::CLEAR_COLOR,
            FrameFlags::DEFAULT,
        );
        let queued = match result {
            Ok(frame) if frame.is_empty => false,
            Ok(_) => {
                let feedback = self.presentation_feedback(state);
                match self.compositor.queue_frame(Some(feedback)) {
                    Ok(()) => true,
                    Err(err) => {
                        tracing::warn!(%err, "queueing the frame failed");
                        false
                    }
                }
            }
            Err(err) => {
                tracing::warn!(%err, "rendering failed");
                false
            }
        };
        self.pacer.rendered(queued);
        if !queued {
            // Nothing new on screen: clients still get their frame callbacks.
            state.frame_presented(&self.output);
        }
    }

    /// Presentation-time feedback of every surface shown in this frame.
    fn presentation_feedback(&self, state: &Sash) -> OutputPresentationFeedback {
        let mut feedback = OutputPresentationFeedback::new(&self.output);
        for window in state.space.elements() {
            if let Some(toplevel) = window.toplevel() {
                take_presentation_feedback_surface_tree(
                    toplevel.wl_surface(),
                    &mut feedback,
                    surface_primary_scanout_output,
                    |_, _| wp_presentation_feedback::Kind::Vsync,
                );
            }
        }
        feedback
    }

    /// The queued frame is on screen.
    fn flip_complete(&mut self, state: &mut Sash, metadata: Option<DrmEventMetadata>) {
        match self.compositor.frame_submitted() {
            Ok(Some(Some(mut feedback))) => {
                let (time, sequence) = match metadata {
                    Some(DrmEventMetadata {
                        time: DrmEventTime::Monotonic(time),
                        sequence,
                    }) => (time, sequence as u64),
                    _ => (Duration::from(Clock::<Monotonic>::new().now()), 0),
                };
                feedback.presented::<_, Monotonic>(
                    time,
                    Refresh::Fixed(self.refresh),
                    sequence,
                    wp_presentation_feedback::Kind::Vsync
                        | wp_presentation_feedback::Kind::HwClock
                        | wp_presentation_feedback::Kind::HwCompletion,
                );
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "page flip completion failed"),
        }
        state.frame_presented(&self.output);
        if self.pacer.flip_complete() {
            self.render(state);
        }
    }

    fn run(&mut self, step: Step, state: &mut Sash) {
        match step {
            Step::StopRendering => self.pacer.pause(),
            Step::SuspendInput => self.libinput.suspend(),
            Step::PauseDrm => self.device.pause(),
            Step::ActivateDrm => {
                if let Err(err) = self.device.activate(true) {
                    tracing::error!(%err, "could not take the DRM device back");
                }
            }
            Step::ResetDrmState => {
                if let Err(err) = self.compositor.reset_state() {
                    tracing::warn!(%err, "DRM state reset failed");
                }
            }
            Step::ResumeInput => {
                if self.libinput.resume().is_err() {
                    tracing::error!("libinput could not resume");
                }
            }
            Step::RedrawAll => {
                self.compositor.reset_buffers();
                if self.pacer.resume() {
                    self.render(state);
                }
            }
        }
    }
}
