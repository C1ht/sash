// SPDX-License-Identifier: Apache-2.0

//! Timing and memory, as measured in M14 step 1, on the real crate. Ignored: run with
//! `tools/text-goldens.sh --bench` (release build, trixie fonts).

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use sash_text::{ClientText, Style, TextStack};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
// SAFETY: forwards to the system allocator and only counts bytes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        LIVE.fetch_add(n, Ordering::Relaxed);
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

const TITLE: &str = "Settings - Displays and Monitors (5120x14"; // 40 characters

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn title(stack: &mut TextStack, text: &str, scale: f32) {
    let line = stack.line(&ClientText::from(text), Style::default(), None);
    std::hint::black_box(stack.draw(&line, scale, [255, 255, 255, 255]));
}

#[test]
#[ignore = "measurement: tools/text-goldens.sh --bench"]
fn timing_and_memory() {
    let start = Instant::now();
    let mut stack = TextStack::new();
    title(&mut stack, TITLE, 1.0);
    eprintln!(
        "first use (new stack to the first drawn title): {:?}",
        start.elapsed()
    );
    let loads: Vec<Duration> = (0..20)
        .map(|_| {
            let t = Instant::now();
            let mut s = TextStack::new();
            // Shaping text in every fallback script loads every fallback file.
            let line = s.line(&"a ש م न 設 😀 ❤".into(), Style::default(), None);
            std::hint::black_box(line);
            t.elapsed()
        })
        .collect();
    eprintln!(
        "load Manrope + every fallback file (first line needing them): median {:?}",
        median(loads)
    );
    eprintln!("fonts: {:?}", TextStack::new().fonts());
    for scale in [1.0f32, 1.5, 2.0] {
        let cold = median(
            (0..50)
                .map(|_| {
                    let mut s = TextStack::new();
                    title(&mut s, "x", scale); // fonts parsed, cache nearly empty
                    let t = Instant::now();
                    title(&mut s, TITLE, scale);
                    t.elapsed()
                })
                .collect(),
        );
        title(&mut stack, TITLE, scale);
        let warm = median(
            (0..200)
                .map(|_| {
                    let t = Instant::now();
                    title(&mut stack, TITLE, scale);
                    t.elapsed()
                })
                .collect(),
        );
        eprintln!(
            "title 40 chars, 13 px, scale {scale}: cold {cold:?}, warm {warm:?} (shape + draw)"
        );
    }
    let words = [
        "Files",
        "Text Editor",
        "Settings",
        "Terminal",
        "Mail",
        "Calendar",
    ];
    for scale in [1.0f32, 1.5, 2.0] {
        let mut s = TextStack::new();
        let before = LIVE.load(Ordering::Relaxed);
        for i in 0..1000 {
            title(
                &mut s,
                &format!("{} - document {i:04} ({}) æøå é", words[i % 6], i * 7919),
                scale,
            );
        }
        eprintln!(
            "1000 distinct Latin titles at scale {scale}: cache {} KiB, heap +{} KiB",
            s.cache_bytes() / 1024,
            (LIVE.load(Ordering::Relaxed) - before) / 1024
        );
    }
    for scale in [1.0f32, 2.0] {
        let mut s = TextStack::new();
        let before = LIVE.load(Ordering::Relaxed);
        let mut c = 0x4E00u32;
        for _ in 0..1000 {
            let t: String = (0..20)
                .filter_map(|_| {
                    c += 1;
                    char::from_u32(c)
                })
                .collect();
            title(&mut s, &t, scale);
        }
        eprintln!(
            "1000 titles of 20 distinct CJK glyphs at scale {scale}: cache {} KiB (cap 4096), heap +{} KiB",
            s.cache_bytes() / 1024,
            (LIVE.load(Ordering::Relaxed) - before) / 1024
        );
    }
}
