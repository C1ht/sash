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
    /// Debug builds: `--mode WxH@R`, the DRM mode to use instead of the selection rule.
    #[cfg(debug_assertions)]
    mode: Option<backend::drm::ModeRequest>,
}

#[cfg(debug_assertions)]
const USAGE: &str = "usage: sash-wm [--backend winit|drm] [--exit-after <seconds>] [--mode WxH@R]";
#[cfg(not(debug_assertions))]
const USAGE: &str = "usage: sash-wm [--backend winit|drm]";

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        backend: None,
        exit_after: None,
        #[cfg(debug_assertions)]
        mode: None,
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
            #[cfg(debug_assertions)]
            "--mode" => {
                let request = args
                    .next()
                    .and_then(|text| backend::drm::ModeRequest::parse(&text))
                    .ok_or_else(|| format!("--mode takes WxH@R, e.g. 5120x1440@60\n{USAGE}"))?;
                parsed.mode = Some(request);
            }
            "--help" | "-h" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(parsed)
}

/// The backend when `--backend` is not given: nested (winit) inside a Wayland or X11
/// session; DRM in a local console session, where logind sets XDG_SESSION_ID, XDG_SEAT
/// and XDG_VTNR (a remote login has no seat or VT). Empty variables count as unset.
const REMOTE_LOGIN: &str = "DRM needs a local console session; this looks like a remote login";

fn choose_backend(
    explicit: Option<Backend>,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Backend, String> {
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    match explicit {
        Some(backend) => Ok(backend),
        None if set("WAYLAND_DISPLAY") || set("DISPLAY") => Ok(Backend::Winit),
        None if set("XDG_SESSION_ID") && set("XDG_SEAT") && set("XDG_VTNR") => Ok(Backend::Drm),
        None => Err(REMOTE_LOGIN.into()),
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
        Backend::Drm => {
            let options = backend::drm::Options {
                #[cfg(debug_assertions)]
                mode: args.mode,
            };
            backend::drm::start(&event_loop, &mut server.state, redraw_requests, options)?
        }
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
    fn mode_flag_in_debug_builds_only() {
        let parsed = parse(&["--backend", "drm", "--mode", "5120x1440@60"]);
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                parsed.unwrap().mode,
                Some(backend::drm::ModeRequest {
                    width: 5120,
                    height: 1440,
                    refresh: 60
                })
            );
            assert!(parse(&["--mode", "5120x1440"]).is_err());
            assert!(parse(&["--mode"]).is_err());
        }
        #[cfg(not(debug_assertions))]
        assert!(parsed.unwrap_err().starts_with("unknown argument --mode"));
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
        // A local console session (logind sets all three): DRM.
        const CONSOLE: &[(&str, &str)] = &[
            ("XDG_SESSION_ID", "3"),
            ("XDG_SEAT", "seat0"),
            ("XDG_VTNR", "3"),
        ];
        assert_eq!(auto(CONSOLE), Ok(Backend::Drm));
        assert_eq!(
            auto(&[
                ("WAYLAND_DISPLAY", ""),
                ("DISPLAY", ""),
                ("XDG_SESSION_ID", "3"),
                ("XDG_SEAT", "seat0"),
                ("XDG_VTNR", "3"),
            ]),
            Ok(Backend::Drm),
            "empty display variables count as unset"
        );
        // A remote login (SSH: a session, but no seat and no VT), or no session at all:
        // the remote-login message.
        let remote = Err(REMOTE_LOGIN.to_string());
        assert_eq!(auto(&[("XDG_SESSION_ID", "3")]), remote);
        assert_eq!(
            auto(&[("XDG_SESSION_ID", "3"), ("XDG_SEAT", "seat0")]),
            remote
        );
        assert_eq!(auto(&[("XDG_SESSION_ID", "3"), ("XDG_VTNR", "3")]), remote);
        assert_eq!(
            auto(&[
                ("XDG_SESSION_ID", "3"),
                ("XDG_SEAT", "seat0"),
                ("XDG_VTNR", "")
            ]),
            remote
        );
        assert_eq!(auto(&[]), remote);
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
