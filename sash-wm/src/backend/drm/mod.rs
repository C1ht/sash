// SPDX-License-Identifier: Apache-2.0

//! DRM/KMS backend: sash-wm directly on the hardware, started from a text console (or
//! with `--backend drm`).
//!
//! - Session: libseat (logind or seatd) opens the GPU and input devices; Ctrl+Alt+Fn
//!   switch VTs through it before any keybind sees the keys (seat.rs).
//! - One GPU (udev's primary GPU), one connector, its native mode (mode.rs). Every
//!   candidate mode is validated with an atomic TEST_ONLY commit before anything is
//!   committed; the device is opened without disabling outputs, so if no mode passes
//!   the display is left untouched.
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
mod recheck;
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
            DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventMetadata, DrmEventTime,
            compositor::{DrmCompositor, FrameError, FrameFlags},
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
        drm::control::{Device as ControlDevice, Mode, ModeFlags, ModeTypeFlags, connector, crtc},
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
use super::recovery::{self, Failure, Recovery, Watchdog};
#[cfg(debug_assertions)]
use super::recovery::{Faults, Injected};
use crate::{dmabuf, keybinds::Modifier, output, render, server::Server, state::Sash};

#[cfg(debug_assertions)]
pub use self::mode::ModeRequest;

/// Start options of the DRM backend.
#[derive(Debug, Default)]
pub struct Options {
    /// Debug builds: `--mode WxH@R` instead of the selection rule.
    #[cfg(debug_assertions)]
    pub mode: Option<ModeRequest>,
}

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
    /// When the connector is re-checked after udev events (merged, input first).
    recheck: recheck::Recheck,
    /// Frames whose page flip was refused because DRM master was already gone (the
    /// session pausing), since the last session event.
    flips_refused: u64,
    /// Timing points since the last resume (recorder and debug log only).
    resume: Option<ResumeTrace>,
    /// Renderer recovery (CLAUDE.md): the failure episode, the flip watchdog, armed
    /// timers, and the EGL display a rebuilt renderer is made on.
    recovery: Recovery,
    watchdog: Watchdog,
    retry_armed: bool,
    watchdog_armed: bool,
    egl_display: EGLDisplay,
    /// Debug builds: injected renderer faults (SASH_DEBUG_FAIL_SWAPS and friends).
    #[cfg(debug_assertions)]
    render_faults: Faults,
    /// For timers that call back into the backend.
    this: Weak<RefCell<Drm>>,
    /// Debug builds: test commits made to fail (fault injection).
    #[cfg(debug_assertions)]
    faults: mode::ModeFaults,
}

/// The firsts after a resume, for timing points (recorder and debug log only).
struct ResumeTrace {
    at: Instant,
    frame_started: bool,
    flipped: bool,
    input_back: bool,
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
    options: Options,
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
    // Outputs are not disabled on open: nothing is committed until a mode has passed
    // its test commit.
    let (mut device, device_events) =
        DrmDevice::new(fd.clone(), false).map_err(|err| format!("DRM device: {err}"))?;
    let gbm = GbmDevice::new(fd.clone()).map_err(|err| format!("GBM device: {err}"))?;
    // Safety: the display and context live as long as the renderer that owns them.
    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|err| format!("EGL: {err}"))?;
    let context = EGLContext::new(&display).map_err(|err| format!("EGL context: {err}"))?;
    let mut renderer =
        unsafe { GlesRenderer::new(context) }.map_err(|err| format!("GLES renderer: {err}"))?;

    let OutputChoice {
        connector,
        info,
        crtc,
        candidates,
    } = choose_output(&device, &options)?;
    let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
    let size_mm = info.size().map_or((0, 0), |(w, h)| (w as i32, h as i32));
    tracing::info!(
        connector = name,
        size_mm = ?size_mm,
        candidates = ?candidates.iter().map(mode_name).collect::<Vec<_>>(),
        "mode candidates"
    );
    let surface = device
        .create_surface(crtc, candidates[0], &[connector])
        .map_err(|err| format!("DRM surface on {name}: {err}"))?;
    #[cfg(debug_assertions)]
    let faults = mode_faults_from_env()?;
    let Some(drm_mode) = validate(&candidates, mode_name, |mode| {
        #[cfg(debug_assertions)]
        injected_fault(&faults, &mode)?;
        surface.use_mode(mode).map_err(|err| err.to_string())
    }) else {
        // Nothing was committed. Pausing the device first makes dropping the surface and
        // the device commit nothing either: the display stays as it was.
        device.pause();
        drop(surface);
        return Err(format!(
            "no mode of {name} passed its test commit ({} tried, see the log); the display \
             was left untouched",
            candidates.len()
        )
        .into());
    };
    // A mode passed: now every output is turned off, and only ours comes back.
    device
        .reset_state()
        .map_err(|err| format!("DRM state reset: {err}"))?;
    if let Err(err) = surface.use_vrr(false) {
        tracing::debug!(%err, "VRR could not be turned off (no VRR property?)");
    }
    let (width, height) = drm_mode.size();
    tracing::info!(
        connector = name,
        mode = mode_name(&drm_mode),
        timings = ?drm_mode,
        "output"
    );
    let output = output::create_physical(state, &name, size_mm, drm_mode.into());
    // sash-wm draws the arrow here (cursor plane or composited).
    state.draws_named_cursor = true;
    // The theme's default shape is loaded at startup; every other one on first use.
    match state
        .cursor_theme
        .image(smithay::input::pointer::CursorIcon::Default)
    {
        Some(image) => tracing::info!(frames = image.frames.len(), "cursor theme loaded"),
        None => tracing::warn!("no cursor theme: the built-in arrow is drawn"),
    }
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
            recheck: recheck::Recheck::default(),
            flips_refused: 0,
            resume: None,
            recovery: Recovery::default(),
            watchdog: Watchdog::default(),
            retry_armed: false,
            watchdog_armed: false,
            egl_display: display.clone(),
            #[cfg(debug_assertions)]
            render_faults: Faults::from_env(|name| std::env::var(name).ok()),
            this: this.clone(),
            #[cfg(debug_assertions)]
            faults,
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
                        && let Some(at) = drm.recheck.changed(now())
                    {
                        drm.arm_recheck(at, &mut server.state);
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

/// Recovery failed: sash-wm exits cleanly (the session gives the display and the
/// console back, as on every exit) with `reason` as its error.
fn give_up(state: &mut Sash, reason: &str) {
    tracing::error!(reason, "the renderer could not be recovered; exiting");
    record!(state, "renderer_gave_up");
    state.fatal = Some(format!("the renderer could not be recovered: {reason}"));
    state.loop_signal.stop();
}

/// A commit refused because this process is no longer DRM master (EACCES): the
/// session is pausing.
fn master_gone(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(nix::errno::Errno::EACCES as i32)
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

/// "WxH@R" with the exact refresh rate, for logs.
fn mode_name(mode: &Mode) -> String {
    let (width, height) = mode.size();
    let hz = exact_refresh_mhz(mode) as f64 / 1000.0;
    format!("{width}x{height}@{hz:.3}")
}

/// The exact refresh rate in mHz: pixel clock / (htotal x vtotal); the kernel's
/// rounded rate if the timings are empty.
fn exact_refresh_mhz(mode: &Mode) -> u32 {
    let pixels = mode.hsync().2 as u64 * mode.vsync().2 as u64;
    if pixels == 0 {
        return mode.vrefresh() * 1000;
    }
    (mode.clock() as u64 * 1_000_000 / pixels) as u32
}

/// Tries the candidates in order with `test` (an atomic TEST_ONLY commit) and returns
/// the first that passes. Logs every candidate and why it was rejected.
fn validate<M: Copy, E: std::fmt::Display>(
    candidates: &[M],
    name: impl Fn(&M) -> String,
    mut test: impl FnMut(M) -> Result<(), E>,
) -> Option<M> {
    for (n, &mode) in candidates.iter().enumerate() {
        match test(mode) {
            Ok(()) => {
                tracing::info!(
                    n,
                    mode = name(&mode),
                    "mode candidate passed its test commit"
                );
                return Some(mode);
            }
            Err(err) => tracing::warn!(
                n,
                mode = name(&mode),
                reason = %err,
                "mode candidate rejected"
            ),
        }
    }
    None
}

/// The connector to drive, its info, a CRTC for it and the modes to try in order (see
/// mode.rs; with `--mode` in debug builds, only the requested one).
fn choose_output(
    device: &DrmDevice,
    #[cfg_attr(not(debug_assertions), expect(unused_variables))] options: &Options,
) -> Result<OutputChoice, Box<dyn Error>> {
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
    let index = mode::choose_connector(&infos).ok_or("no connected display")?;
    let (handle, info) = handles.swap_remove(index);
    let connector = &infos[index];
    let order = mode::candidates(connector);
    #[cfg(debug_assertions)]
    let order = match options.mode {
        Some(request) => {
            let found = mode::requested(connector, request);
            if found.is_empty() {
                let offered: Vec<String> = info.modes().iter().map(mode_name).collect();
                return Err(format!(
                    "--mode {}x{}@{} is not offered; offered: {}",
                    request.width,
                    request.height,
                    request.refresh,
                    offered.join(", ")
                )
                .into());
            }
            tracing::info!(?request, "--mode: only the requested mode is tried");
            found
        }
        None => order,
    };
    if order.is_empty() {
        return Err("the display offers no usable (non-interlaced) mode".into());
    }
    let candidates = order.iter().map(|&i| info.modes()[i]).collect();
    let crtc = info
        .encoders()
        .iter()
        .filter_map(|&encoder| device.get_encoder(encoder).ok())
        .flat_map(|encoder| resources.filter_crtcs(encoder.possible_crtcs()))
        .next()
        .ok_or("no CRTC for the connector")?;
    Ok(OutputChoice {
        connector: handle,
        info,
        crtc,
        candidates,
    })
}

/// What `choose_output` found.
struct OutputChoice {
    connector: connector::Handle,
    info: connector::Info,
    crtc: crtc::Handle,
    /// The modes to test, in order.
    candidates: Vec<Mode>,
}

/// A libinput device's capabilities, for the debug recorder.
#[cfg(feature = "debug-record")]
fn device_capabilities(device: &smithay::reexports::input::Device) -> Vec<crate::record::Json> {
    use smithay::reexports::input::DeviceCapability as Cap;
    [
        (Cap::Keyboard, "keyboard"),
        (Cap::Pointer, "pointer"),
        (Cap::Touch, "touch"),
        (Cap::TabletTool, "tablet_tool"),
        (Cap::TabletPad, "tablet_pad"),
        (Cap::Gesture, "gesture"),
        (Cap::Switch, "switch"),
    ]
    .into_iter()
    .filter(|(cap, _)| device.has_capability(*cap))
    .map(|(_, name)| name.into())
    .collect()
}

/// A connector as mode.rs sees it.
fn connector_info(handle: connector::Handle, info: &connector::Info) -> ConnectorInfo {
    ConnectorInfo {
        id: u32::from(handle),
        connected: info.state() == connector::State::Connected,
        size_mm: info.size().unwrap_or((0, 0)),
        modes: info.modes().iter().map(mode_info).collect(),
    }
}

/// A mode as mode.rs sees it.
fn mode_info(m: &Mode) -> ModeInfo {
    ModeInfo {
        width: m.size().0,
        height: m.size().1,
        refresh_mhz: exact_refresh_mhz(m),
        preferred: m.mode_type().contains(ModeTypeFlags::PREFERRED),
        interlaced: m.flags().contains(ModeFlags::INTERLACE),
    }
}

/// Debug builds: the fault injection asked for in the environment.
#[cfg(debug_assertions)]
fn mode_faults_from_env() -> Result<mode::ModeFaults, String> {
    let var = |name| std::env::var(name).ok();
    let faults = mode::ModeFaults::parse(
        var("SASH_DEBUG_REJECT_MODES").as_deref(),
        var("SASH_DEBUG_REJECT_ALL_MODES").as_deref(),
    )?;
    if !faults.is_empty() {
        tracing::warn!(?faults, "fault injection: these test commits will fail");
    }
    Ok(faults)
}

/// Debug builds: fails the test commit of `mode` if the fault injection says so.
#[cfg(debug_assertions)]
fn injected_fault(faults: &mode::ModeFaults, mode: &Mode) -> Result<(), String> {
    if faults.rejects(&mode_info(mode)) {
        return Err(
            "fault injection (SASH_DEBUG_REJECT_MODES / SASH_DEBUG_REJECT_ALL_MODES)".into(),
        );
    }
    Ok(())
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
                SessionEvent::PauseSession => {
                    drm.resume = None;
                    drm.tracker.pause()
                }
                SessionEvent::ActivateSession => {
                    drm.resume = Some(ResumeTrace {
                        at: Instant::now(),
                        frame_started: false,
                        flipped: false,
                        input_back: false,
                    });
                    drm.timing_point(&mut server.state, "resume start", None);
                    drm.recheck.resumed(now());
                    drm.tracker.resume()
                }
            };
            let flips_refused = std::mem::take(&mut drm.flips_refused);
            tracing::info!(?event, ?steps, flips_refused, "session");
            for step in steps {
                let start = Instant::now();
                drm.run(step, &mut server.state);
                if drm.resume.is_some() {
                    let name = match step {
                        Step::ActivateDrm => "activate(true)",
                        Step::ResetDrmState => "DRM state reset",
                        Step::ResumeInput => "libinput restored",
                        Step::RedrawAll => "redraw (includes the first frame)",
                        _ => "pause step",
                    };
                    drm.timing_point(&mut server.state, name, Some(start.elapsed()));
                }
            }
        })
        .map_err(|err| err.error)?;

    let for_input = drm.clone();
    handle
        .insert_source(input_backend, move |event, _, server| {
            input_event(&for_input, event, server);
            // A connector re-check that waited for the input devices after a resume
            // runs once they are back.
            let mut drm = for_input.borrow_mut();
            if let Some(resume) = drm.resume.as_mut()
                && !std::mem::replace(&mut resume.input_back, true)
            {
                drm.timing_point(&mut server.state, "first input event", None);
            }
            if drm.recheck.input_back(now()) {
                drm.connector_changed(&mut server.state);
            }
        })
        .map_err(|err| err.error)?;
    Ok(())
}

/// One libinput event: device bookkeeping, or input for the compositor.
fn input_event(
    drm: &Rc<RefCell<Drm>>,
    event: InputEvent<LibinputInputBackend>,
    server: &mut Server,
) {
    match event {
        InputEvent::DeviceAdded { mut device } => {
            let name = device.name().to_string();
            if drm.borrow_mut().devices.added(&mut device) {
                tracing::info!(name, "input device added");
            }
            record!(
                server.state,
                "input_device_added",
                "name" => name,
                "capabilities" => device_capabilities(&device),
            );
        }
        InputEvent::DeviceRemoved { device } => {
            if drm.borrow_mut().devices.removed(&device) {
                tracing::info!(name = device.name(), "input device removed");
            }
            record!(server.state, "input_device_removed", "name" => device.name());
        }
        event => server.state.handle_input(event),
    }
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
        // The pacer's schedule counts from the start of rendering, not its end.
        let started = now();
        state.frame_started();
        record!(state, "frame_start");
        let first_after_resume = self
            .resume
            .as_mut()
            .is_some_and(|r| !std::mem::replace(&mut r.frame_started, true));
        if first_after_resume {
            self.timing_point(state, "first frame start", None);
        }
        let mut elements = render::cursor_elements(state, &mut self.renderer, 1.0);
        elements.extend(render::arrow_element(
            state,
            &mut self.renderer,
            &self.arrow,
        ));
        #[cfg(feature = "debug-record")]
        let cursor_shown = !elements.is_empty();
        elements.extend(render::output_elements(state, &mut self.renderer, 1.0));
        let result = self.compositor.render_frame(
            &mut self.renderer,
            &elements,
            render::CLEAR_COLOR,
            FrameFlags::DEFAULT,
        );
        let drawn = start.elapsed();
        #[cfg(feature = "debug-record")]
        let cursor_on_plane = result.as_ref().is_ok_and(|f| f.cursor_element.is_some());
        #[cfg(feature = "debug-record")]
        let cursor = match (cursor_shown, cursor_on_plane) {
            (false, _) => "hidden",
            (true, true) => "plane",
            (true, false) => "composited",
        };
        let outcome: Result<bool, Failure> = match result {
            Ok(frame) if frame.is_empty => Ok(false),
            Ok(_) => match self.injected_fault() {
                Some(failure) => Err(failure),
                None => {
                    let feedback = self.presentation_feedback(state);
                    match self.compositor.queue_frame(Some(feedback)) {
                        Ok(()) => Ok(true),
                        Err(FrameError::DrmError(DrmError::Access(access)))
                            if master_gone(&access.source) =>
                        {
                            // The session is pausing and DRM master is already gone: an
                            // expected race, counted and logged with the session event.
                            self.flips_refused += 1;
                            tracing::debug!(err = %access.source, "page flip refused: DRM master gone");
                            Ok(false)
                        }
                        Err(FrameError::DrmError(DrmError::DeviceInactive)) => Ok(false),
                        Err(err) => {
                            tracing::warn!(%err, "queueing the frame failed");
                            Err(Failure::Frame)
                        }
                    }
                }
            },
            Err(err) => {
                tracing::warn!(%err, "rendering failed");
                Err(if recovery::context_lost(&err) {
                    Failure::ContextLost
                } else {
                    Failure::Frame
                })
            }
        };
        let queued = outcome == Ok(true);
        if queued {
            self.watchdog.queued(now());
            self.arm_watchdog(state);
        }
        let committed = start.elapsed().saturating_sub(drawn);
        if first_after_resume {
            self.timing_point(state, "first frame drawn", Some(drawn));
            self.timing_point(state, "first frame committed (modeset)", Some(committed));
        }
        self.pacer.rendered(queued, started);
        state.schedule_cursor_animation();
        #[cfg(feature = "debug-record")]
        self.record_frame(state, queued, cursor, start.elapsed(), (drawn, committed));
        if queued {
            self.queued = Some(QueuedFrame {
                render_us: start.elapsed().as_micros() as u64,
                input_since: state.windows.input_pending_since.take(),
            });
        } else {
            self.idle_frame_callbacks(state);
        }
        if let Err(failure) = outcome {
            self.renderer_failed(failure, state);
        }
    }

    /// Debug builds: the fault injected into this frame, if any.
    fn injected_fault(&mut self) -> Option<Failure> {
        #[cfg(debug_assertions)]
        match self.render_faults.swap() {
            Some(Injected::Fail) => return Some(Failure::Frame),
            Some(Injected::ContextLost) => return Some(Failure::ContextLost),
            None => {}
        }
        None
    }

    /// A frame failed: retry later, rebuild the renderer, or give up (recovery.rs).
    fn renderer_failed(&mut self, failure: Failure, state: &mut Sash) {
        let next = self.recovery.failed(failure, now());
        tracing::warn!(?failure, ?next, "renderer failure");
        record!(
            state,
            "renderer_failure",
            "failure" => format!("{failure:?}"),
            "next" => format!("{next:?}"),
        );
        match next {
            recovery::Next::RetryIn(delay) => self.arm_retry(delay, state),
            recovery::Next::RebuildThenRetryIn(delay) => match self.rebuild_renderer() {
                Ok(()) => {
                    tracing::warn!("renderer rebuilt");
                    record!(state, "renderer_rebuilt");
                    self.arm_retry(delay, state);
                }
                Err(err) => give_up(state, &format!("rebuilding the renderer failed: {err}")),
            },
            recovery::Next::GiveUp(reason) => give_up(state, reason),
        }
    }

    /// A new EGL context and renderer on the same display; the compositor's buffers are
    /// reset, so the next frame is drawn in full. Windows' buffers and the cursor are
    /// imported into the new renderer as they are drawn: no client has to commit.
    fn rebuild_renderer(&mut self) -> Result<(), String> {
        let context = EGLContext::new(&self.egl_display).map_err(|e| e.to_string())?;
        let renderer = unsafe { GlesRenderer::new(context) }.map_err(|e| e.to_string())?;
        self.renderer = renderer;
        self.compositor.reset_buffers();
        Ok(())
    }

    /// The failed frame is tried again after `delay` (a timer, never a frame callback),
    /// drawn in full. While the session is paused the resume redraws instead.
    fn arm_retry(&mut self, delay: Duration, state: &mut Sash) {
        if self.retry_armed {
            return;
        }
        let this = self.this.clone();
        let inserted =
            state
                .loop_handle
                .insert_source(Timer::from_duration(delay), move |_, _, server| {
                    if let Some(drm) = this.upgrade() {
                        let mut drm = drm.borrow_mut();
                        drm.retry_armed = false;
                        if drm.pacer.is_active() {
                            tracing::info!("renderer retry");
                            record!(server.state, "renderer_retry");
                            drm.compositor.reset_buffers();
                            drm.render(&mut server.state);
                        }
                    }
                    TimeoutAction::Drop
                });
        self.retry_armed = inserted.is_ok();
    }

    /// Checks the flip watchdog while a frame waits for its flip.
    fn arm_watchdog(&mut self, state: &mut Sash) {
        if self.watchdog_armed {
            return;
        }
        let this = self.this.clone();
        let inserted = state.loop_handle.insert_source(
            Timer::from_duration(recovery::WATCHDOG / 4),
            move |_, _, server| {
                let Some(drm) = this.upgrade() else {
                    return TimeoutAction::Drop;
                };
                let mut drm = drm.borrow_mut();
                let expected = drm.pacer.is_active();
                if drm.watchdog.fired(now(), expected) {
                    drm.watchdog_armed = false;
                    tracing::warn!("no page flip for 2 s");
                    record!(server.state, "flip_watchdog");
                    drm.renderer_failed(Failure::Watchdog, &mut server.state);
                    return TimeoutAction::Drop;
                }
                if drm.watchdog.waiting() {
                    TimeoutAction::ToDuration(recovery::WATCHDOG / 4)
                } else {
                    drm.watchdog_armed = false;
                    TimeoutAction::Drop
                }
            },
        );
        self.watchdog_armed = inserted.is_ok();
    }

    /// Debug recording: the frame's end (where the cursor went), the counters, and a
    /// screenshot if one is due.
    #[cfg(feature = "debug-record")]
    fn record_frame(
        &mut self,
        state: &mut Sash,
        queued: bool,
        cursor: &'static str,
        took: Duration,
        (drawn, committed): (Duration, Duration),
    ) {
        if state.recorder.is_none() {
            return;
        }
        let pointer = state.seat.get_pointer().map(|p| p.current_location());
        if let Some(recorder) = state.recorder.as_mut() {
            if queued {
                crate::record::Counters::bump(&recorder.counters.frames_rendered);
                if let Some(p) = pointer {
                    recorder.cursor_drawn((p.x.round() as i32, p.y.round() as i32));
                }
                recorder.cursor_path(cursor);
            }
            recorder.push(
                "frame_end",
                vec![
                    ("queued", queued.into()),
                    ("render_us", (took.as_micros() as u64).into()),
                    ("draw_us", (drawn.as_micros() as u64).into()),
                    ("commit_us", (committed.as_micros() as u64).into()),
                    ("cursor", cursor.into()),
                    ("unpaced", self.pacer.is_unpaced().into()),
                ],
            );
        }
        let (w, h) = self.mode.size();
        crate::record::after_frame::<_, smithay::backend::renderer::gles::GlesTexture>(
            state,
            &mut self.renderer,
            (w as i32, h as i32).into(),
        );
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
        // Debug builds: flips swallowed on request (the watchdog's test).
        #[cfg(debug_assertions)]
        if self.render_faults.flips_stopped() {
            return;
        }
        self.watchdog.flipped();
        if self.recovery.active() {
            tracing::info!("renderer recovered");
            record!(state, "renderer_recovered");
        }
        self.recovery.succeeded();
        if let Some(resume) = self.resume.as_mut()
            && !std::mem::replace(&mut resume.flipped, true)
        {
            self.timing_point(state, "first flip", None);
        }
        #[cfg(feature = "debug-record")]
        let metadata_present = metadata.is_some();
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
        #[cfg(feature = "debug-record")]
        if let Some(recorder) = &state.recorder {
            crate::record::Counters::bump(&recorder.counters.flips_completed);
        }
        record!(
            state,
            "flip_complete",
            "kernel_time_us" => time.as_micros() as u64,
            "sequence" => sequence,
            "kernel_timestamp" => metadata_present,
        );
        let next = self.pacer.flip_complete(time, now());
        match self.pacer.take_change() {
            Some(true) => tracing::info!(
                refresh_us = self.refresh.as_micros() as u64,
                "page flips complete without vblank pacing; rendering is capped at the refresh rate"
            ),
            Some(false) => tracing::info!(
                refresh_us = self.refresh.as_micros() as u64,
                "modeset or resume: flip pacing is classified again from the start (cap off)"
            ),
            None => {}
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
    /// A timing point (recorder event `resume_step` and a debug line): the time since
    /// the last resume, if any, and how long the step took, if it is one.
    #[cfg_attr(not(feature = "debug-record"), expect(unused_variables))]
    fn timing_point(&self, state: &mut Sash, step: &'static str, took: Option<Duration>) {
        let since = self
            .resume
            .as_ref()
            .map(|r| r.at.elapsed().as_micros() as u64);
        let took = took.map(|t| t.as_micros() as u64);
        tracing::debug!(step, since_resume_us = ?since, took_us = ?took, "timing");
        record!(
            state,
            "resume_step",
            "step" => step,
            "since_resume_us" => since,
            "took_us" => took,
        );
    }

    /// Arms the timer for a merged connector re-check (recheck.rs) at `at`.
    fn arm_recheck(&mut self, at: Duration, state: &mut Sash) {
        let this = self.this.clone();
        let timer = Timer::from_duration(at.saturating_sub(now()));
        let inserted = state.loop_handle.insert_source(timer, move |_, _, server| {
            if let Some(drm) = this.upgrade() {
                let mut drm = drm.borrow_mut();
                match drm.recheck.fired(now()) {
                    recheck::Fired::Run => drm.connector_changed(&mut server.state),
                    recheck::Fired::WaitUntil(at) => drm.arm_recheck(at, &mut server.state),
                    recheck::Fired::Nothing => {}
                }
            }
            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            // No timer: re-check now rather than not at all.
            tracing::warn!(%err, "no timer for the connector re-check");
            if self.recheck.fired(at) == recheck::Fired::Run {
                self.connector_changed(state);
            }
        }
    }

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
        let probe = Instant::now();
        let info = self.device.get_connector(self.connector, true);
        self.timing_point(state, "connector re-check", Some(probe.elapsed()));
        let info = match info {
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
                    let order = mode::candidates(&connector_info(self.connector, &info));
                    let candidates: Vec<Mode> = order.iter().map(|&i| info.modes()[i]).collect();
                    if !self.switch_mode(&candidates, state) {
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

    /// The previous mode is gone: the first candidate that passes its test commit is used
    /// from the next frame on; the compositor, the output clients see, the layout area
    /// (the normal output-resize path) and the pacing follow. Returns false if none
    /// passes (the display stays dark).
    fn switch_mode(&mut self, candidates: &[Mode], state: &mut Sash) -> bool {
        tracing::info!(
            candidates = ?candidates.iter().map(mode_name).collect::<Vec<_>>(),
            "the display no longer offers the mode in use; mode candidates"
        );
        let Some(mode) = validate(candidates, mode_name, |mode| {
            #[cfg(debug_assertions)]
            injected_fault(&self.faults, &mode)?;
            self.compositor
                .use_mode(mode)
                .map_err(|err| err.to_string())
        }) else {
            tracing::warn!("no mode passed its test commit; the display stays dark");
            return false;
        };
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
            Step::StopRendering => {
                self.pacer.pause();
                self.watchdog.reset();
            }
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

#[cfg(test)]
mod tests {
    use super::{master_gone, validate};

    /// The race at a session pause (stage C run 4): "Permission denied (os error 13)"
    /// is the expected refusal; other errors are not.
    #[test]
    fn a_flip_refused_with_eacces_is_the_pause_race() {
        use std::io::Error;
        assert!(master_gone(&Error::from_raw_os_error(13)));
        assert!(
            !master_gone(&Error::from_raw_os_error(16)),
            "EBUSY is a real error"
        );
        assert!(
            !master_gone(&Error::from_raw_os_error(22)),
            "EINVAL is a real error"
        );
        assert!(!master_gone(&Error::other("not an OS error")));
    }

    #[test]
    fn validation_takes_the_first_candidate_that_passes_in_order() {
        let name = |m: &u32| m.to_string();
        let mut tried = Vec::new();
        // The driver refuses 240 and 120 (e.g. link bandwidth); 60 passes.
        let passed = validate(&[240, 120, 60, 30], name, |m| {
            tried.push(m);
            if m > 60 {
                Err("test commit failed")
            } else {
                Ok(())
            }
        });
        assert_eq!(passed, Some(60));
        assert_eq!(tried, vec![240, 120, 60], "stops at the first that passes");
        // Nothing passes: None (the caller exits and leaves the display untouched).
        assert_eq!(
            validate(&[240, 120], name, |_| Err("test commit failed")),
            None
        );
        assert_eq!(validate(&[], name, |_: u32| Ok::<(), &str>(())), None);
    }
}
