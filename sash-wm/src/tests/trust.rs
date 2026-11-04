// SPDX-License-Identifier: Apache-2.0

//! Privileged clients end to end (M13a): real processes started by sash-wm's spawner
//! (this test binary again, running `helper_process`) and normal clients, against the
//! stand-in privileged global (`trust::stand_in`, ext_foreign_toplevel_list_v1).

use std::{
    ffi::OsString,
    io::Write,
    os::{fd::FromRawFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use wayland_client::Connection;

use super::{client::TestClient, harness::Harness};

const STAND_IN: &str = "ext_foreign_toplevel_list_v1";

/// What a helper process does (SASH_TRUST_HELPER).
fn helper(mode: &str, out: &Path, harness: &Harness) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args([
            "--exact",
            "tests::trust::helper_process",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
            "-q",
        ])
        .env("SASH_TRUST_HELPER", mode)
        .env("SASH_TRUST_OUT", out)
        .env("SASH_TRUST_DISPLAY", &harness.server.socket_name)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// A fresh directory under the system temp directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sash-trust-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The interfaces a helper listed, once its file is complete (ends with "end").
fn read_list(path: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    (lines.last().map(String::as_str) == Some("end")).then(|| lines[..lines.len() - 1].to_vec())
}

fn wait_for_list(h: &mut Harness, path: &Path) -> Vec<String> {
    let mut list = None;
    h.run_until("a helper's global list", |_| {
        list = read_list(path);
        list.is_some()
    });
    list.expect("listed")
}

fn write_list(out: &Path, client: &TestClient) {
    let mut file = std::fs::File::create(out).expect("helper output");
    for (_, interface) in &client.events.globals {
        writeln!(file, "{interface}").expect("write");
    }
    writeln!(file, "end").expect("write");
}

/// The helper, run as its own process by the tests below (ignored otherwise). It never
/// connects anywhere but the harness: to WAYLAND_SOCKET when it was started with one,
/// else to the harness's public socket only.
#[test]
#[ignore = "a helper process the trust tests start"]
fn helper_process() {
    let Some(mode) = std::env::var_os("SASH_TRUST_HELPER") else {
        return;
    };
    let out = PathBuf::from(std::env::var_os("SASH_TRUST_OUT").expect("SASH_TRUST_OUT"));
    let display = std::env::var_os("SASH_TRUST_DISPLAY").expect("SASH_TRUST_DISPLAY");
    let from_env = || {
        assert!(
            std::env::var_os("WAYLAND_SOCKET").is_some(),
            "started without a socket"
        );
        TestClient::from_connection(Connection::connect_to_env().expect("WAYLAND_SOCKET"))
    };
    match mode.to_str().expect("mode") {
        // Lists what a privileged connection sees, then exits.
        "spawned" => write_list(&out, &from_env()),
        // Lists, then starts a child that inherits everything an ordinary child
        // would (the environment, after the client library removed WAYLAND_SOCKET).
        "fork" => {
            let client = from_env();
            write_list(&out, &client);
            let mut child = Command::new(std::env::current_exe().expect("exe"));
            child
                .args(std::env::args_os().skip(1).collect::<Vec<OsString>>())
                .env("SASH_TRUST_HELPER", "child")
                .env("SASH_TRUST_OUT", out.with_extension("child"));
            let status = child.status().expect("child");
            assert!(status.success());
            drop(client);
        }
        // An ordinary child: connects the usual way (WAYLAND_DISPLAY, which must be the
        // harness's socket, never the host's).
        "child" => {
            assert!(
                std::env::var_os("WAYLAND_SOCKET").is_none(),
                "WAYLAND_SOCKET inherited"
            );
            assert_eq!(std::env::var_os("WAYLAND_DISPLAY"), Some(display));
            let client = TestClient::from_connection(Connection::connect_to_env().expect("env"));
            write_list(&out, &client);
        }
        // Connects, then dies without closing anything.
        "abort" => {
            let client = from_env();
            write_list(&out, &client);
            std::process::abort();
        }
        // Writes garbage on the privileged socket instead of the protocol.
        "garbage" => {
            let fd: i32 = std::env::var("WAYLAND_SOCKET")
                .expect("fd")
                .parse()
                .expect("fd");
            // Safety: the fd is the socket sash-wm passed and nothing else owns it.
            let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
            let _ = stream.write_all(&[0xff; 64]);
            std::fs::write(&out, "end\n").expect("write");
        }
        // Passes its own connection's descriptor to wl_shm.create_pool.
        "own-fd-shm" => {
            let mut client = from_env();
            client.create_pool_from_own_connection();
            assert!(client.try_roundtrip().is_err(), "a socket was mapped");
            std::fs::write(&out, "end\n").expect("write");
        }
        // Passes its own connection's descriptor to wl_data_offer.receive, which sends
        // it on to the selection's source.
        "own-fd-receive" => {
            let mut client = from_env();
            client.data_device();
            let _window = client.map_window();
            client.dispatch_until("the selection", |e| e.data.selection_offer().is_some());
            let offer = client.events.data.selection_offer().expect("offer");
            client.receive_into_own_connection(&offer, "text/plain");
            std::fs::write(&out, "end\n").expect("write");
            // Wait until sash-wm ends the connection.
            while client.try_roundtrip().is_ok() {}
        }
        other => panic!("unknown helper mode {other}"),
    }
}

#[test]
fn a_normal_client_sees_no_privileged_global_and_a_guessed_bind_fails() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let client = TestClient::connect(&socket);
        let visible: Vec<u32> = client.events.globals.iter().map(|(n, _)| *n).collect();
        assert!(!client.events.globals.is_empty());
        assert!(
            client.events.globals.iter().all(|(_, i)| i != STAND_IN),
            "{:?}",
            client.events.globals
        );
        // Guess every name that was not listed (the stand-in's is one of them): each
        // bind is refused and only that client is disconnected.
        let highest = visible.iter().copied().max().unwrap_or(0);
        for name in (1..=highest + 8).filter(|n| !visible.contains(n)) {
            let mut guesser = TestClient::connect(&socket);
            guesser.bind_guessed_stand_in(name);
            assert!(
                guesser.try_roundtrip().is_err(),
                "a bind of name {name} was accepted"
            );
        }
        // The server is fine: a new client connects and works.
        let mut after = TestClient::connect(&socket);
        let _ = after.map_window();
    });
}

#[test]
fn a_spawned_client_sees_it_and_loses_it_when_its_connection_closes() {
    let dir = Scratch::new();
    let mut harness = Harness::new();
    let out = dir.file("spawned");
    let command = helper("spawned", &out, &harness);
    let pid = harness
        .server
        .state
        .spawn_privileged(command)
        .expect("spawn");
    assert!(pid > 0);
    assert_eq!(
        harness.server.state.trust.open, 1,
        "privileged from the start"
    );
    let list = wait_for_list(&mut harness, &out);
    assert!(list.iter().any(|i| i == STAND_IN), "{list:?}");
    // The helper exits: its connection, and with it the privilege, is gone.
    harness.run_until("the privileged connection closes", |h| {
        h.server.state.trust.open == 0
    });
    // Restart: a new start gets a new privileged connection.
    let out2 = dir.file("restarted");
    let command = helper("spawned", &out2, &harness);
    harness
        .server
        .state
        .spawn_privileged(command)
        .expect("restart");
    let list = wait_for_list(&mut harness, &out2);
    assert!(list.iter().any(|i| i == STAND_IN), "{list:?}");
    harness.run_until("closed again", |h| h.server.state.trust.open == 0);
}

#[test]
fn a_spawned_clients_child_is_unprivileged() {
    let dir = Scratch::new();
    let mut harness = Harness::new();
    let out = dir.file("fork");
    let command = helper("fork", &out, &harness);
    harness
        .server
        .state
        .spawn_privileged(command)
        .expect("spawn");
    let parent = wait_for_list(&mut harness, &out);
    let child = wait_for_list(&mut harness, &out.with_extension("child"));
    assert!(parent.iter().any(|i| i == STAND_IN), "{parent:?}");
    assert!(child.iter().all(|i| i != STAND_IN), "{child:?}");
    assert!(!child.is_empty(), "the child is a normal client");
    harness.run_until("the parent's connection closes", |h| {
        h.server.state.trust.open == 0
    });
}

/// A process running the very same program, connected through the public socket, and a
/// client claiming a first-party name, stay unprivileged: only the socket counts.
#[test]
fn claims_and_paths_give_no_privilege() {
    let dir = Scratch::new();
    let mut harness = Harness::new();
    let out = dir.file("public");
    let mut command = helper("child", &out, &harness);
    command.env("WAYLAND_DISPLAY", &harness.server.socket_name);
    let mut process = command.spawn().expect("start the same binary");
    let list = wait_for_list(&mut harness, &out);
    assert!(list.iter().all(|i| i != STAND_IN), "{list:?}");
    let _ = process.wait();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let mut client = TestClient::connect(&socket);
        let w = client.create_window_without_commit();
        w.toplevel.set_app_id("sash-shell".into());
        w.toplevel.set_title("Settings (privileged)".into());
        w.surface.commit();
        client.roundtrip();
        let again = TestClient::connect(&socket);
        assert!(again.events.globals.iter().all(|(_, i)| i != STAND_IN));
        assert!(client.events.globals.iter().all(|(_, i)| i != STAND_IN));
    });
    assert_eq!(harness.server.state.trust.open, 0);
}

/// Failures: a program that does not exist, a client that dies, one that writes garbage
/// on its socket. Nothing is left behind and the server carries on.
#[test]
fn the_server_survives_every_failure() {
    let dir = Scratch::new();
    let mut harness = Harness::new();
    let missing = Command::new("/nonexistent/sash-first-party-client");
    assert!(harness.server.state.spawn_privileged(missing).is_err());
    assert_eq!(harness.server.state.trust.open, 0, "nothing added");
    for mode in ["abort", "garbage"] {
        let out = dir.file(mode);
        let command = helper(mode, &out, &harness);
        harness
            .server
            .state
            .spawn_privileged(command)
            .expect("spawn");
        wait_for_list(&mut harness, &out);
        harness.run_until("its connection closes", |h| h.server.state.trust.open == 0);
    }
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let mut client = TestClient::connect(&socket);
        let _ = client.map_window();
    });
}

/// Privilege belongs to the descriptor, and sash-wm never turns a descriptor it
/// receives into a connection: a privileged client passing its own connection to
/// requests that take an fd adds no connection. Through wl_data_offer.receive the
/// descriptor reaches another client (the selection's source), which can then write
/// on the privileged connection: here garbage, which ends it (docs/trust.md).
#[test]
fn a_privileged_clients_own_descriptor_never_becomes_a_connection() {
    let dir = Scratch::new();
    let mut harness = Harness::new();
    let out = dir.file("shm");
    let command = helper("own-fd-shm", &out, &harness);
    harness
        .server
        .state
        .spawn_privileged(command)
        .expect("spawn");
    wait_for_list(&mut harness, &out);
    harness.run_until("the privileged connection closes", |h| {
        h.server.state.trust.open == 0
    });
    assert_eq!(harness.server.connections.open(), 0, "no connection added");

    let out = dir.file("receive");
    let command = helper("own-fd-receive", &out, &harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut source = TestClient::connect(&socket);
        let _window = source.map_window();
        let copied = source.copy(&["text/plain"], b"not a wayland message".to_vec());
        server.on_server(move |h| {
            h.server.state.spawn_privileged(command).expect("spawn");
        });
        source.dispatch_until("the send event", |e| {
            !e.data.source(&copied).sends.is_empty()
        });
        source.join_writers();
        server.on_server(|h| {
            h.run_until("the privileged connection ends", |h| {
                h.server.state.trust.open == 0
            });
            assert_eq!(
                h.server.connections.open(),
                1,
                "only the source is connected"
            );
        });
    });
    wait_for_list(&mut harness, &out);
    harness.run_until("every connection closes", |h| {
        h.server.connections.open() == 0
    });
}
