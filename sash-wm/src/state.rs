// SPDX-License-Identifier: Apache-2.0

use std::{ffi::OsString, time::Instant};

use smithay::{
    desktop::{Space, Window, utils::send_frames_surface_tree},
    input::{Seat, SeatState, keyboard::XkbConfig},
    output::Output,
    reexports::{
        calloop::{LoopHandle, LoopSignal, ping::Ping},
        wayland_server::DisplayHandle,
    },
    wayland::{
        compositor::CompositorState, cursor_shape::CursorShapeManagerState,
        presentation::PresentationState, selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState, shm::ShmState,
    },
};

use crate::{
    cursor::Cursor,
    dmabuf::Dmabufs,
    keybinds::{Keybinds, Modifier},
    popups::Popups,
    render::Overlay,
    server::Server,
    windows::Windows,
};

const KEY_REPEAT_DELAY_MS: i32 = 600;
const KEY_REPEAT_RATE: i32 = 25;

/// Smithay's bookkeeping for each global sash-wm advertises.
pub struct Protocols {
    pub compositor: CompositorState,
    pub shm: ShmState,
    pub xdg_shell: XdgShellState,
    pub data_device: DataDeviceState,
    pub seat: SeatState<Sash>,
    #[expect(
        dead_code,
        reason = "the global lives on the display; this keeps its id"
    )]
    pub cursor_shape: CursorShapeManagerState,
}

impl Protocols {
    /// Creates the globals. The output global is created by the backend with the output.
    fn advertise(display: &DisplayHandle) -> Self {
        Self {
            compositor: CompositorState::new::<Sash>(display),
            shm: ShmState::new::<Sash>(display, Vec::new()),
            xdg_shell: XdgShellState::new::<Sash>(display),
            data_device: DataDeviceState::new::<Sash>(display),
            seat: SeatState::new(),
            cursor_shape: CursorShapeManagerState::new::<Sash>(display),
        }
    }
}

/// The compositor state every protocol handler works on.
pub struct Sash {
    pub display_handle: DisplayHandle,
    pub protocols: Protocols,
    /// The only seat, with one keyboard and one pointer.
    pub seat: Seat<Sash>,

    /// Mapped windows at their tile positions, and the output. Used for output
    /// enter/leave and frame callbacks; drawing and hit-testing go through `windows`.
    pub space: Space<Window>,
    pub output: Option<Output>,
    pub windows: Windows,
    pub popups: Popups,
    pub dmabufs: Dmabufs,
    pub cursor: Cursor,
    /// Snap preview and fullscreen backdrop.
    pub overlay: Overlay,
    pub keybinds: Keybinds,

    /// Process start; the clock for frame callback timestamps and startup time.
    pub started: Instant,
    pub loop_signal: LoopSignal,
    /// Switches to another VT through the session (DRM backend); None when nested.
    pub vt_switch: Option<Box<dyn FnMut(i32)>>,
    /// wp_presentation, advertised by the DRM backend (real on-screen times).
    pub presentation: Option<PresentationState>,
    pub loop_handle: LoopHandle<'static, Server>,
    redraw: Ping,
    /// The backend draws the named arrow itself (DRM); nested, the host draws it.
    pub draws_named_cursor: bool,
    /// Frames requested because the cursor sash-wm draws moved.
    pub cursor_frames: crate::cursor::CursorFrames,
    /// Debug recording (feature `debug-record`), when SASH_DEBUG_RECORD is set.
    #[cfg(feature = "debug-record")]
    pub recorder: Option<crate::record::Recorder>,
}

impl Sash {
    pub fn new(
        display_handle: DisplayHandle,
        loop_signal: LoopSignal,
        loop_handle: LoopHandle<'static, Server>,
        redraw: Ping,
        started: Instant,
        modifier: Modifier,
        socket_name: OsString,
    ) -> Self {
        let mut protocols = Protocols::advertise(&display_handle);
        let mut seat = protocols.seat.new_wl_seat(&display_handle, "seat0");
        if let Err(err) =
            seat.add_keyboard(XkbConfig::default(), KEY_REPEAT_DELAY_MS, KEY_REPEAT_RATE)
        {
            tracing::error!(%err, "no keyboard: the default xkb keymap failed to load");
        }
        seat.add_pointer();

        Self {
            display_handle,
            protocols,
            seat,
            space: Space::default(),
            output: None,
            windows: Windows::new(),
            popups: Popups::default(),
            dmabufs: Dmabufs::default(),
            cursor: Cursor::default(),
            overlay: Overlay::new(),
            keybinds: Keybinds::new(modifier, socket_name),
            started,
            loop_signal,
            vt_switch: None,
            presentation: None,
            loop_handle,
            redraw,
            draws_named_cursor: false,
            cursor_frames: Default::default(),
            #[cfg(feature = "debug-record")]
            recorder: None,
        }
    }

    /// Advertises wp_presentation with the monotonic clock (DRM backend).
    pub fn enable_presentation(&mut self) {
        // CLOCK_MONOTONIC is clock id 1.
        self.presentation = Some(PresentationState::new::<Sash>(&self.display_handle, 1));
    }

    /// Asks the backend for a new frame. Requests made before the frame is drawn
    /// are merged into one.
    pub fn request_redraw(&self) {
        #[cfg(feature = "debug-record")]
        if let Some(recorder) = &self.recorder {
            crate::record::Counters::bump(&recorder.counters.redraws_requested);
        }
        self.redraw.ping();
    }

    /// Called by the backend when it starts drawing a frame.
    pub fn frame_started(&mut self) {
        self.cursor_frames.frame_started();
    }

    /// Called by the backend after a frame for `output` reached the screen.
    ///
    /// This is the only place clients get frame callbacks, so while the output is not
    /// presenting (for example a hidden nested window) they get none.
    pub fn frame_presented(&mut self, output: &Output) {
        let time = self.started.elapsed();
        for window in self.space.elements() {
            window.send_frame(output, time, None, |_, _| Some(output.clone()));
        }
        if let Some((surface, _)) = self.cursor_surface() {
            send_frames_surface_tree(&surface, output, time, None, |_, _| Some(output.clone()));
        }
        self.space.refresh();
    }
}

smithay::delegate_presentation!(Sash);
