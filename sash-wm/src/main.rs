// SPDX-License-Identifier: Apache-2.0

mod backend;
mod state;

use std::time::Instant;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing_subscriber::EnvFilter;

use crate::state::Sash;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return Err("no WAYLAND_DISPLAY or DISPLAY set; only nested mode exists so far".into());
    }

    let mut event_loop: EventLoop<Sash> = EventLoop::try_new()?;
    let display: Display<Sash> = Display::new()?;
    let mut state = Sash::new(&event_loop, display, start_time)?;
    tracing::info!(socket = ?state.socket_name, "listening on wayland socket");

    backend::winit::init(&event_loop, &mut state)?;

    event_loop.run(None, &mut state, |state| {
        let _ = state.display_handle.flush_clients();
    })?;

    Ok(())
}
