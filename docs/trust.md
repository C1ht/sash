<!-- SPDX-License-Identifier: Apache-2.0 -->

# Privileged clients

Rules: CLAUDE.md, "Trust and spawning (M13a, owner)". Built in M13a: the mechanism
only. No real privileged protocol exists yet; the tests use a stand-in.

## What is privileged

Layer-shell, session lock, virtual keyboard, input method and screen capture (none of
them built yet). Their globals are advertised only to privileged connections; every
other client does not see them in its registry, and a bind by a guessed name is a
protocol error for that client only. Idle-inhibit is open to every client.

## How a first-party client is started

1. sash-wm creates a connected socket pair.
2. Its own end becomes a client marked privileged, with the program's file name and
   process id (for logs only).
3. The program is started with the other end as `WAYLAND_SOCKET` (the descriptor is
   inherited only by that program; sash-wm closes its copy at once) and
   `WAYLAND_DISPLAY` set to sash-wm's public socket.
4. The client library (libwayland, or wayland-client in Rust) uses `WAYLAND_SOCKET`,
   removes it from the environment and marks the descriptor close-on-exec. A child the
   program starts can therefore only connect through the public socket, unprivileged.

The socket is the proof. A process id, an executable path, an app id, a title or a
layer namespace never grants anything: a process running the same binary that connects
to the public socket is an ordinary client.

The privilege ends when the connection closes. Restarting a client means a new start
by sash-wm, with a new socket pair. Which programs sash-wm starts (fixed first-party
binaries from a fixed path, for example the dock and launcher in M16) is decided with
them; nothing is started yet.

Cost (recorded by the owner): no third-party bars, launchers or wallpaper tools.

## What is logged

| When | Log (info) | Recorder event (debug-record builds) |
|---|---|---|
| a privileged client is started | `started a privileged client` with program and pid | `client_spawned` (program file name, pid) |
| its connection closes | `privileged connection closed` with program and pid | `privileged_closed` (program, pid) |
| the process exits | `privileged client exited` with its status (debug) | none |
| the start fails | the error is returned to the caller, nothing is added | none |

No arguments and no environment are logged or recorded.

## Tests

`sash-wm/src/tests/trust.rs` runs real processes: the test binary starts itself as the
helper (`helper_process`), through the spawner or directly. The stand-in privileged
global (`trust::stand_in`, interface ext_foreign_toplevel_list_v1, listing nothing)
exists only in test builds; tools/check-release.sh proves it is in the test binary and
in no sash-wm binary.
