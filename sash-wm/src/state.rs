// SPDX-License-Identifier: Apache-2.0

use std::{ffi::OsString, time::Instant};

use smithay::{
    desktop::{Space, Window},
    input::{Seat, SeatState, keyboard::XkbConfig},
    output::Output,
    reexports::{
        calloop::{LoopSignal, ping::Ping},
        wayland_server::DisplayHandle,
    },
    wayland::{
        compositor::CompositorState, selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState, shm::ShmState,
    },
};

use crate::{
    keybinds::{Keybinds, Modifier},
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
    pub keybinds: Keybinds,

    /// Process start; the clock for frame callback timestamps and startup time.
    pub started: Instant,
    pub loop_signal: LoopSignal,
    redraw: Ping,
}

impl Sash {
    pub fn new(
        display_handle: DisplayHandle,
        loop_signal: LoopSignal,
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
            keybinds: Keybinds::new(modifier, socket_name),
            started,
            loop_signal,
            redraw,
        }
    }

    /// Asks the backend for a new frame. Requests made before the frame is drawn
    /// are merged into one.
    pub fn request_redraw(&self) {
        self.redraw.ping();
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
        self.space.refresh();
    }
}
