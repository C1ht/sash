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
by sash-wm, with a new socket pair.

## Who holds the privilege

Privilege belongs to the connection's file descriptor, and so to whoever holds it.
A privileged program can hand its descriptor to another process (by passing it over a
socket, or by letting a child inherit it on purpose), and that process can then speak
on the privileged connection. sash-wm cannot tell the difference and does not try.
Therefore:

- Only trusted code is ever spawned. The programs sash-wm may start are a fixed list
  of absolute paths owned by root, compiled into sash-wm. A program is never looked
  up in `PATH` and never taken from the environment or a file. (The list is added in
  M16 with the first program, the dock and the launcher; it is empty now.)
- sash-wm never turns a descriptor it receives in a request into a connection. A
  privileged client that passes its own connection's descriptor to a request taking
  an fd gains nothing: `wl_shm.create_pool` fails to map it (a protocol error for
  that client), and `wl_data_offer.receive` hands it to the selection's source like
  any pipe, which can then write on the privileged connection. A first-party client
  must never do that; the test `a_privileged_clients_own_descriptor_never_becomes_a_connection`
  shows both paths and that no connection is added.

Cost (recorded by the owner): no third-party bars, launchers or wallpaper tools.

## What is logged

| When | Log (info) | Recorder event (debug-record builds) |
|---|---|---|
| a privileged client is started | `started a privileged client` with program and pid | `client_spawned` (program file name, pid) |
| its connection closes | `privileged connection closed` with program and pid | `privileged_closed` (program, pid) |
| the process exits | `privileged client exited` with its status (debug) | none |
| the start fails | the error is returned to the caller, nothing is added | none |

No arguments and no environment are logged or recorded.

## Resource limits (every client)

Constants in `sash-wm/src/limits.rs`, each logged when hit:

| Limit | Value | When hit | Log |
|---|---|---|---|
| open connections on the public socket | 256 | the new connection is closed at once | warn on the first refusal, debug after; info with the count when accepting again |
| global binds per connection | 1024 | the client is disconnected with `wl_display.error` no_memory | warn |
| unread events per client, beyond the socket's own buffer | 4096 bytes (wayland-backend's fixed value; changing it needs wayland-sys, a new crate) | the library cuts the client off | warn (`client cut off`): sash-wm keeps a duplicate of each connection and sees the client's end still open |

The cut-off log also fires for a client that sent malformed data, which the library
ends the same way. Objects a live connection creates other than binds (surfaces,
buffers, regions) are not counted: wayland-backend 0.3 has no hook for it. They are
freed when the connection closes. Privileged connections count toward the bind and
event limits, not toward the connection cap.

## Tests

`sash-wm/src/tests/trust.rs` runs real processes: the test binary starts itself as the
helper (`helper_process`), through the spawner or directly. The stand-in privileged
global (`trust::stand_in`, interface ext_foreign_toplevel_list_v1, listing nothing)
exists only in test builds; tools/check-release.sh proves it is in the test binary and
in no sash-wm binary.
