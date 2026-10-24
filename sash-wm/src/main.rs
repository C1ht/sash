// SPDX-License-Identifier: Apache-2.0

mod backend;
mod clients;
mod data_device;
mod keybinds;
mod layout;
mod output;
mod render;
mod seat;
mod server;
mod shell;
mod state;
mod surface;
#[cfg(test)]
mod tests;
mod tiling;

use std::time::Instant;

use smithay::reexports::calloop::{EventLoop, ping};
use tracing_subscriber::EnvFilter;

use crate::server::Server;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return Err("no WAYLAND_DISPLAY or DISPLAY set; only nested mode exists so far".into());
    }

    let mut event_loop: EventLoop<Server> = EventLoop::try_new()?;
    let (redraw, redraw_requests) = ping::make_ping()?;
    let mut server = Server::new(&event_loop, redraw, start_time, backend::winit::MODIFIER)?;
    tracing::info!(socket = ?server.socket_name, "listening on wayland socket");

    backend::winit::start(&event_loop, &mut server.state, redraw_requests)?;

    event_loop.run(None, &mut server, Server::flush)?;
    Ok(())
}
