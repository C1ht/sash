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

/// Which backend runs: nested in another session, or on the hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Winit,
    Drm,
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    /// `--backend`; without it, `choose_backend` decides.
    backend: Option<Backend>,
    /// Debug builds: quit after this long (a watchdog for the first DRM runs).
    exit_after: Option<Duration>,
}

const USAGE: &str = "usage: sash-wm [--backend winit|drm] [--exit-after <seconds>]";

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        backend: None,
        exit_after: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--backend" => {
                parsed.backend = match args.next().as_deref() {
                    Some("winit") => Some(Backend::Winit),
                    Some("drm") => Some(Backend::Drm),
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

/// The backend when `--backend` is not given: nested (winit) inside a Wayland or X11
/// session, DRM on a console with a login session (logind sets XDG_SESSION_ID). Empty
/// variables count as unset.
fn choose_backend(
    explicit: Option<Backend>,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Backend, String> {
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    match explicit {
        Some(backend) => Ok(backend),
        None if set("WAYLAND_DISPLAY") || set("DISPLAY") => Ok(Backend::Winit),
        None if set("XDG_SESSION_ID") => Ok(Backend::Drm),
        None => Err(
            "no WAYLAND_DISPLAY or DISPLAY (nothing to run nested in) and no \
             login session (XDG_SESSION_ID) for the DRM backend; log in on a text \
             console, or choose with --backend winit|drm"
                .into(),
        ),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = parse_args(std::env::args().skip(1))?;
    let backend = choose_backend(args.backend, |name| std::env::var_os(name))?;
    tracing::info!(
        ?backend,
        chosen_by = if args.backend.is_some() {
            "--backend"
        } else {
            "environment"
        },
        "backend"
    );

    let mut event_loop: EventLoop<Server> = EventLoop::try_new()?;
    // SIGTERM, SIGINT and SIGHUP (the terminal went away) end the event loop, so
    // everything is dropped in order and the session gives the display and input back.
    // Installed before the backend opens anything: a signal during startup is then a
    // clean, logged exit too.
    event_loop
        .handle()
        .insert_source(
            Signals::new(&[Signal::SIGTERM, Signal::SIGINT, Signal::SIGHUP])?,
            |event, _, server| {
                tracing::info!(signal = ?event.signal(), "quitting");
                server.state.loop_signal.stop();
            },
        )
        .map_err(|err| err.error)?;
    let (redraw, redraw_requests) = ping::make_ping()?;
    let modifier = match backend {
        Backend::Winit => backend::winit::MODIFIER,
        Backend::Drm => backend::drm::MODIFIER,
    };
    let mut server = Server::new(&event_loop, redraw, start_time, modifier)?;
    tracing::info!(socket = ?server.socket_name, "listening on wayland socket");

    match backend {
        Backend::Winit => backend::winit::start(&event_loop, &mut server.state, redraw_requests)?,
        Backend::Drm => backend::drm::start(&event_loop, &mut server.state, redraw_requests)?,
    }

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
        assert_eq!(parse(&[]).unwrap().backend, None);
        assert_eq!(
            parse(&["--backend", "drm"]).unwrap().backend,
            Some(Backend::Drm)
        );
        assert_eq!(
            parse(&["--backend", "winit"]).unwrap().backend,
            Some(Backend::Winit)
        );
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

    #[test]
    fn backend_from_the_environment() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        let auto = |vars| choose_backend(None, env(vars));
        // Inside a Wayland or X11 session: nested.
        assert_eq!(
            auto(&[("WAYLAND_DISPLAY", "wayland-1")]),
            Ok(Backend::Winit)
        );
        assert_eq!(auto(&[("DISPLAY", ":0")]), Ok(Backend::Winit));
        assert_eq!(
            auto(&[("WAYLAND_DISPLAY", "wayland-1"), ("XDG_SESSION_ID", "3")]),
            Ok(Backend::Winit),
            "a desktop session also has a login session"
        );
        // A text console with a login session: DRM.
        assert_eq!(auto(&[("XDG_SESSION_ID", "3")]), Ok(Backend::Drm));
        assert_eq!(
            auto(&[
                ("WAYLAND_DISPLAY", ""),
                ("DISPLAY", ""),
                ("XDG_SESSION_ID", "3")
            ]),
            Ok(Backend::Drm),
            "empty variables count as unset"
        );
        // Neither: an error with a message.
        assert!(auto(&[]).is_err());
        assert!(auto(&[("XDG_SESSION_ID", "")]).is_err());
        // --backend overrides the environment either way.
        assert_eq!(
            choose_backend(Some(Backend::Drm), env(&[("WAYLAND_DISPLAY", "wayland-1")])),
            Ok(Backend::Drm)
        );
        assert_eq!(
            choose_backend(Some(Backend::Winit), env(&[])),
            Ok(Backend::Winit)
        );
    }
}
