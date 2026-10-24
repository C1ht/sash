// SPDX-License-Identifier: Apache-2.0

//! Debug builds only: synthetic pointer and keyboard input for scripted runs. Commands
//! arrive on a unix socket, `$XDG_RUNTIME_DIR/sash-wm-input-<wayland socket>.sock`
//! (mode 0600), one per line, and go to the same handlers the backend uses:
//!
//! ```text
//! motion <x> <y>                                  pointer to (x, y), output coordinates
//! button <left|right|middle|code> <press|release> pointer button
//! key <evdev code> <press|release>                key (e.g. 56 = left Alt)
//! ```
//!
//! Each command is answered with `ok` or `error: <reason>` on its own line.

use std::{
    ffi::OsStr,
    io::{self, BufRead, BufReader, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    time::Instant,
};

use smithay::{
    backend::input::{ButtonState, KeyState},
    input::keyboard::Keycode,
    reexports::calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, generic::Generic},
};

use crate::{
    seat::{BTN_LEFT, BTN_RIGHT},
    server::Server,
    state::Sash,
};

const BTN_MIDDLE: u32 = 0x112;

/// Opens the input socket next to the Wayland socket. Returns its path.
pub fn listen(event_loop: &EventLoop<'static, Server>, socket_name: &OsStr) -> io::Result<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))?;
    let path = PathBuf::from(runtime_dir).join(format!(
        "sash-wm-input-{}.sock",
        socket_name.to_string_lossy()
    ));
    // A leftover from an earlier run with the same Wayland socket name (which is locked
    // while in use, so no live instance owns this path).
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let started = Instant::now();
    let handle = event_loop.handle();
    event_loop
        .handle()
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, _| {
                while let Ok((stream, _)) = listener.accept() {
                    if let Err(err) = add_connection(&handle, stream, started) {
                        tracing::warn!(%err, "synthetic input connection failed");
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| err.error)?;
    Ok(path)
}

fn add_connection(
    handle: &LoopHandle<'static, Server>,
    stream: UnixStream,
    started: Instant,
) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    handle
        .insert_source(
            Generic::new(reader.get_ref().try_clone()?, Interest::READ, Mode::Level),
            move |_, _, server| {
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => return Ok(PostAction::Remove),
                        Ok(_) if !line.ends_with('\n') => return Ok(PostAction::Continue),
                        Ok(_) => {
                            let time = started.elapsed().as_millis() as u32;
                            let reply = match run(&mut server.state, line.trim(), time) {
                                Ok(()) => "ok\n".to_string(),
                                Err(err) => format!("error: {err}\n"),
                            };
                            server.flush();
                            if writer.write_all(reply.as_bytes()).is_err() {
                                return Ok(PostAction::Remove);
                            }
                        }
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                            return Ok(PostAction::Continue);
                        }
                        Err(_) => return Ok(PostAction::Remove),
                    }
                }
            },
        )
        .map_err(|err| err.error)?;
    Ok(())
}

/// Parses and runs one command.
fn run(state: &mut Sash, command: &str, time: u32) -> Result<(), String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let number = |s: &str| s.parse::<f64>().map_err(|_| format!("not a number: {s}"));
    let pressed = |s: &str| match s {
        "press" => Ok(true),
        "release" => Ok(false),
        other => Err(format!("expected press or release, got {other}")),
    };
    match words.as_slice() {
        ["motion", x, y] => {
            state.pointer_moved_to((number(x)?, number(y)?).into(), time);
            Ok(())
        }
        ["button", button, action] => {
            let code = match *button {
                "left" => BTN_LEFT,
                "right" => BTN_RIGHT,
                "middle" => BTN_MIDDLE,
                other => other
                    .parse()
                    .map_err(|_| format!("unknown button {other}"))?,
            };
            let action = if pressed(action)? {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            state.pointer_button(code, action, time);
            Ok(())
        }
        ["key", code, action] => {
            let code: u32 = code
                .parse()
                .map_err(|_| format!("not a key code: {code}"))?;
            let action = if pressed(action)? {
                KeyState::Pressed
            } else {
                KeyState::Released
            };
            state.key(Keycode::new(code + 8), action, time);
            Ok(())
        }
        _ => Err(format!("unknown command: {command}")),
    }
}
