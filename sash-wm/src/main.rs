// SPDX-License-Identifier: Apache-2.0

mod backend;
#[cfg(debug_assertions)]
mod capture;
mod clients;
mod cursor;
mod data_device;
mod dmabuf;
mod grab;
mod keybinds;
mod layout;
mod output;
mod popups;
mod render;
pub mod seat;
mod server;
mod shell;
mod state;
mod surface;
#[cfg(debug_assertions)]
mod synthetic;
#[cfg(test)]
mod tests;
mod windows;

use std::time::{Duration, Instant};

use smithay::reexports::calloop::{
    EventLoop, ping,
    signals::{Signal, Signals},
    timer::{TimeoutAction, Timer},
};
use tracing_subscriber::EnvFilter;

use crate::server::Server;

/// Which backend runs: nested in another session (default), or on the hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Winit,
    Drm,
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    backend: Backend,
    /// Debug builds: quit after this long (a watchdog for the first DRM runs).
    exit_after: Option<Duration>,
}

const USAGE: &str = "usage: sash-wm [--backend winit|drm] [--exit-after <seconds>]";

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        backend: Backend::Winit,
        exit_after: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--backend" => {
                parsed.backend = match args.next().as_deref() {
                    Some("winit") => Backend::Winit,
                    Some("drm") => Backend::Drm,
                    _ => return Err(format!("--backend takes winit or drm\n{USAGE}")),
                }
            }
            "--exit-after" if cfg!(debug_assertions) => {
                let seconds: u64 = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("--exit-after takes a number of seconds\n{USAGE}"))?;
                parsed.exit_after = Some(Duration::from_secs(seconds));
            }
            "--exit-after" => return Err("--exit-after exists in debug builds only".into()),
            "--help" | "-h" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(parsed)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = parse_args(std::env::args().skip(1))?;
    if args.backend == Backend::Winit
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
        && std::env::var_os("DISPLAY").is_none()
    {
        return Err(
            "no WAYLAND_DISPLAY or DISPLAY set; on a text console use --backend drm".into(),
        );
    }

    let mut event_loop: EventLoop<Server> = EventLoop::try_new()?;
    let (redraw, redraw_requests) = ping::make_ping()?;
    let modifier = match args.backend {
        Backend::Winit => backend::winit::MODIFIER,
        Backend::Drm => backend::drm::MODIFIER,
    };
    let mut server = Server::new(&event_loop, redraw, start_time, modifier)?;
    tracing::info!(socket = ?server.socket_name, "listening on wayland socket");

    match args.backend {
        Backend::Winit => backend::winit::start(&event_loop, &mut server.state, redraw_requests)?,
        Backend::Drm => backend::drm::start(&event_loop, &mut server.state, redraw_requests)?,
    }

    // SIGTERM and SIGINT end the event loop, so everything is dropped in order and the
    // session gives the display and input back.
    event_loop
        .handle()
        .insert_source(
            Signals::new(&[Signal::SIGTERM, Signal::SIGINT])?,
            |event, _, server| {
                tracing::info!(signal = ?event.signal(), "quitting");
                server.state.loop_signal.stop();
            },
        )
        .map_err(|err| err.error)?;
    if let Some(after) = args.exit_after {
        tracing::info!(
            seconds = after.as_secs(),
            "--exit-after: quitting after the delay"
        );
        event_loop
            .handle()
            .insert_source(Timer::from_duration(after), |_, _, server| {
                tracing::info!("--exit-after reached; quitting");
                server.state.loop_signal.stop();
                TimeoutAction::Drop
            })
            .map_err(|err| err.error)?;
    }

    #[cfg(debug_assertions)]
    {
        let path = synthetic::listen(&event_loop, &server.socket_name)?;
        tracing::info!(path = %path.display(), "synthetic input socket (debug build)");
    }
    #[cfg(not(debug_assertions))]
    {
        use nix::sys::signal::{SigHandler, Signal, signal};
        // Safety: installs SIG_IGN, which runs no code in the signal context. Debug builds
        // use SIGUSR1 for the offscreen capture; release builds ignore it.
        unsafe { signal(Signal::SIGUSR1, SigHandler::SigIgn) }?;
    }

    event_loop.run(None, &mut server, Server::flush)?;
    tracing::info!("sash-wm stopped");
    Ok(())
}

#[cfg(test)]
mod args_tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn backends_and_watchdog() {
        assert_eq!(parse(&[]).unwrap().backend, Backend::Winit);
        assert_eq!(parse(&["--backend", "drm"]).unwrap().backend, Backend::Drm);
        assert!(parse(&["--backend", "x11"]).is_err());
        assert!(parse(&["--backend"]).is_err());
        assert!(parse(&["--frobnicate"]).is_err());
        let watchdog = parse(&["--backend", "drm", "--exit-after", "20"]);
        if cfg!(debug_assertions) {
            assert_eq!(watchdog.unwrap().exit_after, Some(Duration::from_secs(20)));
            assert!(parse(&["--exit-after", "0"]).is_err());
            assert!(parse(&["--exit-after", "soon"]).is_err());
        } else {
            assert!(watchdog.is_err(), "debug builds only");
        }
    }
}
