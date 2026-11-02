// SPDX-License-Identifier: Apache-2.0

//! DRM/KMS backend: sash-wm directly on the hardware, started from a text console (or
//! with `--backend drm`).
//!
//! - Session: libseat (logind or seatd) opens the GPU and input devices; Ctrl+Alt+Fn
//!   switch VTs through it before any keybind sees the keys (seat.rs).
//! - One GPU (udev's primary GPU), one connector at its preferred mode (mode.rs).
//! - GBM + EGL + GLES render into a Smithay `DrmCompositor`, which uses atomic
//!   modesetting when the driver has it and puts the cursor on the hardware cursor
//!   plane when it fits (else it is drawn with everything else).
//! - Pacing (pacing.rs): render only after the previous page flip completed; frame
//!   callbacks and presentation-time feedback when a flip completes; capped at the
//!   refresh rate when flips are not paced by vblank.
//! - Session pause and resume (session.rs): stop rendering, give up input and the
//!   device; take them back, reset the device state, redraw.
//! - Input (input.rs): libinput with fixed defaults; the keymap follows XKB_DEFAULT_*.
//!
//! Hotplug is out of scope, except for the connector in use: on a udev change event it
//! is re-probed. Disconnected, rendering stops (windows stay); connected again, the
//! CRTC is cleared and the next frame is a full modeset, at the same mode if the display
//! still offers it, else at the one mode.rs chooses.

mod input;
mod mode;
mod pacing;
mod session;

use std::{
    cell::{Cell, RefCell},
    error::Error,
    path::PathBuf,
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

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
        session::{
            Event as SessionEvent, Session,
            libseat::{LibSeatSession, LibSeatSessionNotifier},
        },
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::utils::{
        OutputPresentationFeedback, surface_primary_scanout_output,
        take_presentation_feedback_surface_tree,
    },
    output::Output,
    reexports::{
        calloop::{
            Dispatcher, EventLoop, LoopHandle,
            ping::PingSource,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as ControlDevice, Mode, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
    },
    utils::{Clock, DeviceFd, Monotonic},
    wayland::presentation::Refresh,
};

use self::{
    mode::{ConnectorInfo, ModeInfo},
    pacing::{Next, Pacer},
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
    /// The connector in use and its mode.
    connector: connector::Handle,
    mode: Mode,
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
    /// The frame waiting for its page flip: how long it took to render, and since when
    /// input it shows was waiting.
    queued: Option<QueuedFrame>,
    /// Frame callbacks for an empty frame are scheduled.
    idle_callbacks: Rc<Cell<bool>>,
    /// A timer for the refresh-rate cap is armed (unpaced flips).
    cap_timer: bool,
    /// The connector came back: the next frame is a full modeset.
    modeset_pending: bool,
    /// For timers that call back into the backend.
    this: Weak<RefCell<Drm>>,
}

struct QueuedFrame {
    render_us: u64,
    input_since: Option<Instant>,
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
    let session_events = wait_until_active(&session, session_events)?;
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
        timings = ?drm_mode,
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

    let refresh = refresh_period(&drm_mode);
    let drm = Rc::new_cyclic(|this| {
        RefCell::new(Drm {
            session: session.clone(),
            device,
            connector,
            mode: drm_mode,
            compositor,
            renderer,
            output,
            libinput,
            pacer: Pacer::new(refresh),
            tracker: SessionTracker::default(),
            devices: input::Devices::default(),
            arrow: render::arrow_buffer(),
            refresh,
            queued: None,
            idle_callbacks: Rc::new(Cell::new(false)),
            cap_timer: false,
            modeset_pending: false,
            this: this.clone(),
        })
    });

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
            let for_udev = drm.clone();
            event_loop
                .handle()
                .insert_source(udev, move |event, _, server| {
                    tracing::info!(?event, "udev event");
                    let mut drm = for_udev.borrow_mut();
                    if let UdevEvent::Changed { device_id } = event
                        && device_id == drm.device.device_id()
                    {
                        drm.connector_changed(&mut server.state);
                    }
                })
                .map_err(|err| err.error)?;
        }
        Err(err) => tracing::warn!(%err, "no udev monitor; a disconnect is not noticed"),
    }

    // The first frame.
    drm.borrow_mut().render(state);
    Ok(())
}

/// One refresh period at `mode`, from its exact timings (the kernel's vrefresh is
/// rounded).
fn refresh_period(mode: &Mode) -> Duration {
    let (_, _, htotal) = mode.hsync();
    let (_, _, vtotal) = mode.vsync();
    let pixels = htotal as u64 * vtotal as u64;
    let clock_hz = mode.clock() as u64 * 1000;
    if pixels == 0 || clock_hz == 0 {
        // No usable timings: the rounded rate, at least 1 Hz.
        return Duration::from_secs(1) / mode.vrefresh().max(1);
    }
    Duration::from_nanos(pixels * 1_000_000_000 / clock_hz)
}

/// The monotonic clock, as the pacer and presentation-time use it.
fn now() -> Duration {
    Duration::from(Clock::<Monotonic>::new().now())
}

/// Two modes with the same timings (names and type flags aside).
fn same_timings(a: &Mode, b: &Mode) -> bool {
    a.size() == b.size()
        && a.clock() == b.clock()
        && a.hsync() == b.hsync()
        && a.vsync() == b.vsync()
}

/// How long sash-wm waits for its session to become the active one.
const SESSION_WAIT: Duration = Duration::from_secs(10);

/// Waits until the session is the active one on its seat (its VT is in front). Started
/// on a VT that is not in front yet, sash-wm must not touch the GPU before: it would not
/// be DRM master. Returns the notifier for the main event loop.
fn wait_until_active(
    session: &LibSeatSession,
    notifier: LibSeatSessionNotifier,
) -> Result<LibSeatSessionNotifier, Box<dyn Error>> {
    if session.is_active() {
        return Ok(notifier);
    }
    tracing::info!("waiting for the session to become active (its VT in front)");
    let mut event_loop: EventLoop<bool> = EventLoop::try_new()?;
    let dispatcher = Dispatcher::new(notifier, |event, _, active: &mut bool| {
        if matches!(event, SessionEvent::ActivateSession) {
            *active = true;
        }
    });
    let token = event_loop
        .handle()
        .register_dispatcher(dispatcher.clone())
        .map_err(|err| format!("session events: {err}"))?;
    let deadline = Instant::now() + SESSION_WAIT;
    let mut active = false;
    while !active && Instant::now() < deadline {
        event_loop.dispatch(Some(Duration::from_millis(100)), &mut active)?;
    }
    event_loop.handle().remove(token);
    drop(event_loop);
    if !active {
        return Err(format!(
            "the session did not become active within {} s",
            SESSION_WAIT.as_secs()
        )
        .into());
    }
    Ok(dispatcher.into_source_inner())
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
        infos.push(connector_info(handle, &info));
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

/// A connector as mode.rs sees it.
fn connector_info(handle: connector::Handle, info: &connector::Info) -> ConnectorInfo {
    ConnectorInfo {
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
    }
}

fn insert_sources(
    handle: &LoopHandle<'static, Server>,
    drm: &Rc<RefCell<Drm>>,
    redraw_requests: PingSource,
    session_events: LibSeatSessionNotifier,
    device_events: smithay::backend::drm::DrmDeviceNotifier,
    input_backend: LibinputInputBackend,
) -> Result<(), Box<dyn Error>> {
    let for_redraws = drm.clone();
    handle
        .insert_source(redraw_requests, move |_, _, server| {
            let mut drm = for_redraws.borrow_mut();
            let next = drm.pacer.want_frame(now());
            drm.follow(next, &mut server.state);
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

impl Drm {
    /// Renders a frame if anything changed and queues it for the next page flip.
    fn render(&mut self, state: &mut Sash) {
        if !self.pacer.is_active() {
            return;
        }
        if std::mem::take(&mut self.modeset_pending) {
            self.force_modeset();
        }
        state.flush_pending_arrange();
        let start = Instant::now();
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
        self.pacer.rendered(queued, now());
        if queued {
            self.queued = Some(QueuedFrame {
                render_us: start.elapsed().as_micros() as u64,
                input_since: state.windows.input_pending_since.take(),
            });
        } else {
            self.idle_frame_callbacks(state);
        }
    }

    /// Nothing new went on screen: clients still get their frame callbacks, one refresh
    /// later (sending them at once would let a client that commits on every callback
    /// spin).
    fn idle_frame_callbacks(&self, state: &mut Sash) {
        if self.idle_callbacks.replace(true) {
            return;
        }
        let pending = self.idle_callbacks.clone();
        let output = self.output.clone();
        let timer = Timer::from_duration(self.refresh);
        let inserted = state.loop_handle.insert_source(timer, move |_, _, server| {
            pending.set(false);
            server.state.frame_presented(&output);
            TimeoutAction::Drop
        });
        if inserted.is_err() {
            self.idle_callbacks.set(false);
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
        let (time, sequence) = match metadata {
            Some(DrmEventMetadata {
                time: DrmEventTime::Monotonic(time),
                sequence,
            }) => (time, sequence as u64),
            _ => (now(), 0),
        };
        match self.compositor.frame_submitted() {
            Ok(Some(Some(mut feedback))) => {
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
        if let Some(frame) = self.queued.take() {
            // The frame is on screen now: from the input it shows to here is what the
            // user waited.
            let input_to_frame_us = frame.input_since.map(|t| t.elapsed().as_micros() as u64);
            tracing::debug!(
                frame_time_us = frame.render_us,
                render_us = frame.render_us,
                input_to_frame_us,
                "frame"
            );
        }
        state.frame_presented(&self.output);
        let unpaced = self.pacer.is_unpaced();
        let next = self.pacer.flip_complete(time, now());
        if !unpaced && self.pacer.is_unpaced() {
            tracing::info!(
                refresh_us = self.refresh.as_micros() as u64,
                "page flips complete without vblank pacing; rendering is capped at the refresh rate"
            );
        }
        self.follow(next, state);
    }

    /// Does what the pacer says: render now, at a later time (a timer), or nothing.
    fn follow(&mut self, next: Next, state: &mut Sash) {
        match next {
            Next::Render => self.render(state),
            Next::RenderAt(at) => self.arm_cap_timer(at, state),
            Next::Wait => {}
        }
    }

    /// Renders at `at` (the refresh-rate cap). One timer at a time.
    fn arm_cap_timer(&mut self, at: Duration, state: &mut Sash) {
        if self.cap_timer {
            return;
        }
        let this = self.this.clone();
        let timer = Timer::from_duration(at.saturating_sub(now()));
        let inserted = state.loop_handle.insert_source(timer, move |_, _, server| {
            if let Some(drm) = this.upgrade() {
                let mut drm = drm.borrow_mut();
                drm.cap_timer = false;
                let next = drm.pacer.timer_fired(now());
                drm.follow(next, &mut server.state);
            }
            TimeoutAction::Drop
        });
        match inserted {
            Ok(_) => self.cap_timer = true,
            // No timer: render now rather than not at all.
            Err(_) => self.render(state),
        }
    }

    /// A udev change event on the DRM device: the connector in use is probed again.
    /// Disconnected: rendering stops (windows and layout stay). Connected again: the next
    /// frame is a full modeset, at the same mode if the display still offers it, else at
    /// the one mode.rs chooses (the output and layout follow its size); then everything
    /// is redrawn.
    fn connector_changed(&mut self, state: &mut Sash) {
        let info = match self.device.get_connector(self.connector, true) {
            Ok(info) => info,
            Err(err) => {
                tracing::warn!(%err, "could not probe the connector after a udev event");
                return;
            }
        };
        let was_connected = self.pacer.is_connected();
        let connected = info.state() == connector::State::Connected;
        let offers_mode = info.modes().iter().any(|m| same_timings(m, &self.mode));
        tracing::info!(
            state = ?info.state(),
            modes = info.modes().len(),
            offers_mode,
            was_connected,
            "connector probed"
        );
        if connected && !offers_mode {
            // What the display offers at the same size, to see why it does not match.
            for mode in info.modes().iter().filter(|m| m.size() == self.mode.size()) {
                tracing::info!(timings = ?mode, "offered at the same size");
            }
        }
        match (was_connected, connected) {
            (true, false) => {
                tracing::info!("display disconnected: rendering stops, windows are kept");
                self.pacer.set_connected(false);
                self.queued = None;
            }
            (false, true) => {
                if !offers_mode {
                    let infos = [connector_info(self.connector, &info)];
                    let Some((_, index)) = mode::choose(&infos) else {
                        tracing::warn!("the display offers no usable mode; it stays dark");
                        return;
                    };
                    if !self.switch_mode(info.modes()[index], state) {
                        return;
                    }
                }
                tracing::info!(
                    mode = format!(
                        "{}x{}@{}",
                        self.mode.size().0,
                        self.mode.size().1,
                        self.mode.vrefresh()
                    ),
                    "display connected again: full modeset"
                );
                self.modeset_pending = true;
                if self.pacer.set_connected(true) {
                    self.render(state);
                }
            }
            _ => {}
        }
    }

    /// Uses `mode` from the next frame on: the compositor, the output clients see, the
    /// layout area and the pacing follow. Returns false if the CRTC cannot use it.
    fn switch_mode(&mut self, mode: Mode, state: &mut Sash) -> bool {
        if let Err(err) = self.compositor.use_mode(mode) {
            tracing::warn!(%err, timings = ?mode, "the new mode cannot be used; the display stays dark");
            return false;
        }
        tracing::info!(from = ?self.mode, to = ?mode, "the display no longer offers the mode in use; switching");
        self.mode = mode;
        self.refresh = refresh_period(&mode);
        self.pacer.set_refresh(self.refresh);
        output::set_mode(&self.output, mode.into());
        let (width, height) = mode.size();
        state.set_layout_area((width as i32, height as i32).into());
        true
    }

    /// Clears the CRTC, so that the next frame commits a full modeset (a plain page flip
    /// would leave a reconnected display dark, e.g. with a bad link status).
    fn force_modeset(&mut self) {
        if let Err(err) = self.compositor.surface().clear() {
            tracing::warn!(%err, "clearing the CRTC before the modeset failed");
        }
        if let Err(err) = self.compositor.reset_state() {
            tracing::warn!(%err, "DRM state reset failed");
        }
        self.compositor.reset_buffers();
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
