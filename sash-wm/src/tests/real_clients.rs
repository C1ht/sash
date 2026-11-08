// SPDX-License-Identifier: Apache-2.0

//! Real clients and the activated state (owner, M19a): what foot and Firefox do when
//! focus moves. Run by tools/measure/run-activated.sh, which starts this ignored test
//! (the headless harness: nothing on any display) and the clients through
//! tools/measure/common.sh (a throwaway config; Firefox only through start_firefox).
//!
//! Three kinds of change, so each effect can be told apart:
//! - pointer focus (keyboard focus and the activated state move together);
//! - the activated state cleared while the window keeps the keyboard;
//! - the keyboard taken away while the window stays activated.
//!
//! After each, the clients' buffers (every surface of each window) are written as PNGs
//! to SASH_REAL_CLIENTS_OUT, with the number of pixels that changed.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::SERIAL_COUNTER,
    wayland::{
        compositor::{TraversalAction, with_surface_tree_downward},
        shm::with_buffer_contents,
    },
};

use super::harness::Harness;
use crate::layout::{WindowId, gaps::Gaps};

/// Every surface of window `id` with a shm buffer: (surface index, width, height, RGB).
fn pixels(h: &Harness, id: u64) -> Vec<(usize, i32, i32, Vec<u8>)> {
    let Some(managed) = h.server.state.windows.by_id(WindowId(id)) else {
        return Vec::new();
    };
    let Some(toplevel) = managed.window.toplevel() else {
        return Vec::new();
    };
    let mut surfaces: Vec<WlSurface> = Vec::new();
    with_surface_tree_downward(
        toplevel.wl_surface(),
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |surface, _, _| surfaces.push(surface.clone()),
        |_, _, _| true,
    );
    let mut out = Vec::new();
    for (i, surface) in surfaces.iter().enumerate() {
        let Some(buffer) = with_renderer_surface_state(surface, |s| s.buffer().cloned()).flatten()
        else {
            continue;
        };
        let read = with_buffer_contents(&buffer, |ptr, len, data| {
            // Safety: the pool is mapped for `len` bytes while the closure runs.
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
            let mut rgb = Vec::with_capacity((data.width * data.height * 3) as usize);
            for y in 0..data.height as usize {
                let row = data.offset as usize + y * data.stride as usize;
                for x in 0..data.width as usize {
                    let p = &bytes[row + x * 4..row + x * 4 + 4];
                    // ARGB8888 / XRGB8888, little endian: B, G, R, A.
                    rgb.extend_from_slice(&[p[2], p[1], p[0]]);
                }
            }
            (data.width, data.height, rgb)
        });
        if let Ok((w, hh, rgb)) = read {
            out.push((i, w, hh, rgb));
        } else {
            println!("window {id} surface {i}: not a shm buffer (dmabuf?), not read");
        }
    }
    out
}

fn write_png(path: &PathBuf, w: i32, h: i32, rgb: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(rgb)
        .unwrap();
}

fn run_for(h: &mut Harness, time: Duration) {
    let end = Instant::now() + time;
    while Instant::now() < end {
        h.step();
    }
}

/// Sets or clears the activated state of window `id` alone (no keyboard change).
fn set_activated(h: &mut Harness, id: u64, on: bool) {
    let managed = h.server.state.windows.by_id(WindowId(id)).unwrap();
    let toplevel = managed.window.toplevel().unwrap().clone();
    toplevel.with_pending_state(|state| {
        if on {
            state.states.set(xdg_toplevel::State::Activated);
        } else {
            state.states.unset(xdg_toplevel::State::Activated);
        }
    });
    toplevel.send_pending_configure();
    h.server.flush();
}

#[test]
#[ignore = "run by tools/measure/run-activated.sh with real clients"]
fn real_clients_and_the_activated_state() {
    let out = PathBuf::from(std::env::var("SASH_REAL_CLIENTS_OUT").expect("SASH_REAL_CLIENTS_OUT"));
    let socket_file = std::env::var("SASH_REAL_CLIENTS_SOCKET").expect("SASH_REAL_CLIENTS_SOCKET");
    let mut h = Harness::new();
    h.server.state.focus.follows_pointer = true;
    h.server.state.windows.layout.set_gaps(Gaps::DEFAULT);
    h.server.state.set_layout_area((1600, 900).into());
    h.timeout = Duration::from_secs(90);
    std::fs::write(
        &socket_file,
        h.server.socket_name.to_string_lossy().as_bytes(),
    )
    .unwrap();
    h.run_until("two real windows", |h| h.mapped_windows() >= 2);
    run_for(&mut h, Duration::from_secs(4));
    let ids: Vec<u64> = h
        .server
        .state
        .windows
        .all()
        .filter(|m| m.mapped)
        .map(|m| m.id.0)
        .collect();
    println!("windows: {ids:?}");
    let mut last: BTreeMap<(u64, usize), Vec<u8>> = BTreeMap::new();
    let mut snapshot = |h: &Harness, phase: &str| {
        for &id in &ids {
            for (i, w, hh, rgb) in pixels(h, id) {
                let changed = last
                    .get(&(id, i))
                    .filter(|old| old.len() == rgb.len())
                    .map(|old| {
                        old.chunks(3)
                            .zip(rgb.chunks(3))
                            .filter(|(a, b)| a != b)
                            .count()
                    });
                println!(
                    "{phase}: window {id} surface {i} {w}x{hh}: {}",
                    match changed {
                        Some(n) => format!("{n} pixels changed"),
                        None => "first picture or new size".into(),
                    }
                );
                write_png(&out.join(format!("{phase}-w{id}-s{i}.png")), w, hh, &rgb);
                last.insert((id, i), rgb);
            }
        }
    };
    let focused = |h: &Harness| h.server.state.windows.layout.focused().map(|w| w.0);
    snapshot(&h, "0-start");
    println!("start: focus {:?}", focused(&h));
    for (n, &id) in ids.iter().enumerate() {
        let r = h.rect_of(id).unwrap();
        let (x, y) = (f64::from(r.x + r.w / 2), f64::from(r.y + r.h / 2));
        // A real motion into the window from the outer gap.
        h.pointer_to(5.0, 5.0);
        h.pointer_to(x, y);
        run_for(&mut h, Duration::from_millis(1500));
        println!("pointer into {id}: focus {:?}", focused(&h));
        snapshot(&h, &format!("{}a-pointer-focus-w{id}", n + 1));
        set_activated(&mut h, id, false);
        run_for(&mut h, Duration::from_millis(1500));
        snapshot(
            &h,
            &format!("{}b-activated-cleared-keyboard-kept-w{id}", n + 1),
        );
        set_activated(&mut h, id, true);
        let keyboard = h.server.state.seat.get_keyboard().unwrap();
        keyboard.set_focus(
            &mut h.server.state,
            None::<WlSurface>,
            SERIAL_COUNTER.next_serial(),
        );
        h.server.flush();
        run_for(&mut h, Duration::from_millis(1500));
        snapshot(
            &h,
            &format!("{}c-keyboard-left-activated-kept-w{id}", n + 1),
        );
        h.server.state.sync_keyboard_focus();
        h.server.flush();
        run_for(&mut h, Duration::from_millis(1000));
    }
}
