// SPDX-License-Identifier: Apache-2.0

//! Window titles in the bars (M15, owner).
//!
//! A title comes from xdg_toplevel.set_title and is untrusted: it is held only as
//! sash-text's `ClientText` (no Debug or Display), never logged or recorded. app_id is
//! never used instead; a window without a title has an empty bar.
//!
//! Shaping is limited per window: a new title is shaped at most `SHAPINGS_PER_SECOND`
//! times a second (the rest are coalesced, the last title wins; a timer brings the last
//! one in), and only while a frame is drawn, so at most once per frame. A focus change
//! reshapes the shaped text at the other weight at once (it is the compositor's, not
//! the client's). Cutting to the bar's width (a live resize) reuses the shaped line
//! (`Line::fit`); glyphs come from sash-text's cache.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    time::{Duration, Instant},
};

use sash_text::{ClientText, Image, Line, Style, TextStack};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Physical, Point, Scale, Transform},
    wayland::{
        compositor,
        shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData},
    },
};

use crate::{
    layout::{Rect, WindowId, decor::Decoration},
    state::Sash,
};

/// Weight and white's alpha of a focused window's title, and of an unfocused one's
/// (owner, M15): focus shows by weight and brightness as well as the border.
pub const FOCUSED: (u16, f32) = (500, 0.92);
pub const UNFOCUSED: (u16, f32) = (400, 0.55);
/// New titles shaped per second and window, at most.
pub const SHAPINGS_PER_SECOND: u32 = 20;
const SHAPING_INTERVAL: Duration = Duration::from_millis(1000 / SHAPINGS_PER_SECOND as u64);

/// The title size in logical pixels: sash-text's UI size (13). Debug builds only:
/// SASH_DEBUG_FONT_PX (6 to 48) to compare sizes by eye on hardware; no setting.
pub fn title_px() -> f32 {
    #[cfg(debug_assertions)]
    {
        static PX: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
        let px = PX.get_or_init(|| {
            let px = std::env::var("SASH_DEBUG_FONT_PX")
                .ok()?
                .parse::<f32>()
                .ok()
                .filter(|px| (6.0..=48.0).contains(px))?;
            tracing::info!(px, "SASH_DEBUG_FONT_PX: title size");
            Some(px)
        });
        if let Some(px) = *px {
            return px;
        }
    }
    sash_text::UI_SIZE
}

/// A title drawn in white for the bar: `width` logical pixels at most (cut with an
/// ellipsis), at `scale`. Also what the golden tests draw.
pub fn draw(stack: &mut TextStack, line: &Line, focused: bool, width: i32, scale: f32) -> Image {
    let alpha = if focused { FOCUSED.1 } else { UNFOCUSED.1 };
    let line = line.fit(Some(width.max(0) as f32));
    stack.draw(&line, scale, [255, 255, 255, (alpha * 255.0).round() as u8])
}

/// Shapes a title for the bar.
pub fn shape(stack: &mut TextStack, text: &ClientText, focused: bool) -> Line {
    let weight = if focused { FOCUSED.0 } else { UNFOCUSED.0 };
    stack.line(
        text,
        Style {
            size: title_px(),
            weight,
        },
        None,
    )
}

struct Shaped {
    generation: u64,
    focused: bool,
    line: Line,
}

/// What the bar shows: the image and where the line's box starts in it.
#[derive(Clone)]
pub struct Drawn {
    key: (u64, bool, i32, u32),
    pub buffer: MemoryRenderBuffer,
    /// Physical pixels: where the line box's top-left corner is in the image.
    pub origin: (i32, i32),
    /// Logical height of the line box (for centring in the bar).
    pub line_height: f32,
}

#[derive(Default)]
struct Title {
    text: ClientText,
    /// Bumped on every change from the client.
    generation: u64,
    /// The text that was last shaped (a focus change reshapes this one).
    shaped_text: ClientText,
    shaped: Option<Shaped>,
    /// When a new title was last shaped.
    last_new: Option<Instant>,
    timer_armed: bool,
    drawn: Option<Drawn>,
}

/// Every window's title, the text stack, and counters for the tests.
#[derive(Default)]
pub struct Titles {
    map: RefCell<HashMap<WindowId, Title>>,
    stack: RefCell<TextStack>,
    /// Titles shaped (new text or a focus change) and drawn, since startup.
    pub shapings: Cell<u64>,
    pub draws: Cell<u64>,
}

impl Titles {
    /// A new title from the client. Returns when a frame should come to show it: now,
    /// or after the wait the rate limit asks for (None: a timer is already armed).
    fn set(&self, id: WindowId, text: ClientText, now: Instant) -> Option<Duration> {
        let mut map = self.map.borrow_mut();
        let title = map.entry(id).or_default();
        if title.text == text {
            return None;
        }
        title.text = text;
        title.generation += 1;
        let wait = title.last_new.map_or(Duration::ZERO, |at| {
            (at + SHAPING_INTERVAL).saturating_duration_since(now)
        });
        if wait.is_zero() {
            return Some(Duration::ZERO);
        }
        if title.timer_armed {
            return None;
        }
        title.timer_armed = true;
        Some(wait)
    }

    fn timer_fired(&self, id: WindowId) {
        if let Some(title) = self.map.borrow_mut().get_mut(&id) {
            title.timer_armed = false;
        }
    }

    /// Tests: a new title, as from the client (no timer).
    #[cfg(test)]
    pub fn set_for_tests(&self, id: WindowId, text: ClientText) {
        let mut map = self.map.borrow_mut();
        let title = map.entry(id).or_default();
        title.text = text;
        title.generation += 1;
    }

    /// Tests: whether the latest title from the client is the one shaped.
    #[cfg(test)]
    pub fn is_current(&self, id: WindowId) -> bool {
        self.map.borrow().get(&id).is_some_and(|t| {
            t.shaped
                .as_ref()
                .is_some_and(|s| s.generation == t.generation)
        })
    }

    pub fn forget(&self, id: WindowId) {
        self.map.borrow_mut().remove(&id);
    }

    /// The title to draw for window `id` in a title area `width` logical pixels wide,
    /// at `scale`; None for an empty title or no room. Shapes and draws only when the
    /// title, the focus, the width or the scale changed.
    pub fn prepare(
        &self,
        id: WindowId,
        width: i32,
        focused: bool,
        scale: f32,
        now: Instant,
    ) -> Option<Drawn> {
        let mut map = self.map.borrow_mut();
        let title = map.get_mut(&id)?;
        if width <= 0 || (title.text.is_empty() && title.shaped_text.is_empty()) {
            return None;
        }
        let mut stack = self.stack.borrow_mut();
        let new_text = title
            .shaped
            .as_ref()
            .is_none_or(|s| s.generation != title.generation);
        let allowed = title.last_new.is_none_or(|at| now >= at + SHAPING_INTERVAL);
        if new_text && allowed {
            title.shaped_text = title.text.clone();
            title.last_new = Some(now);
            title.shaped = Some(Shaped {
                generation: title.generation,
                focused,
                line: shape(&mut stack, &title.shaped_text, focused),
            });
            self.shapings.set(self.shapings.get() + 1);
        } else if let Some(shaped) = title.shaped.as_mut().filter(|s| s.focused != focused) {
            shaped.focused = focused;
            shaped.line = shape(&mut stack, &title.shaped_text, focused);
            self.shapings.set(self.shapings.get() + 1);
        }
        let shaped = title.shaped.as_ref()?;
        if title.shaped_text.is_empty() {
            return None;
        }
        let key = (shaped.generation, focused, width, scale.to_bits());
        if title.drawn.as_ref().is_none_or(|d| d.key != key) {
            let image = draw(&mut stack, &shaped.line, focused, width, scale);
            self.draws.set(self.draws.get() + 1);
            if image.width == 0 || image.height == 0 {
                title.drawn = None;
                return None;
            }
            let buffer = MemoryRenderBuffer::from_slice(
                &image.pixels,
                Fourcc::Argb8888,
                (image.width as i32, image.height as i32),
                1,
                Transform::Normal,
                None,
            );
            title.drawn = Some(Drawn {
                key,
                buffer,
                origin: (image.origin.0 as i32, image.origin.1 as i32),
                line_height: shaped.line.height(),
            });
        }
        title.drawn.clone()
    }
}

impl Sash {
    /// The title of window `id` for its bar (shaped and drawn if needed) and where its
    /// image goes, in physical pixels; None without a bar, title or room.
    pub fn bar_title(
        &self,
        id: WindowId,
        outer: Rect,
        decoration: Decoration,
        scale: Scale<f64>,
    ) -> Option<(Drawn, Point<f64, Physical>)> {
        let area = decoration.title_rect(outer)?;
        let focused = self.windows.layout.focused() == Some(id);
        let drawn = self
            .titles
            .prepare(id, area.w, focused, scale.x as f32, Instant::now())?;
        // The line's box is centred in the bar; the image may reach past it (ink).
        let top =
            f64::from(area.y) + ((f64::from(area.h) - f64::from(drawn.line_height)) / 2.0).round();
        let location = Point::<f64, Logical>::from((f64::from(area.x), top)).to_physical(scale)
            - Point::from((f64::from(drawn.origin.0), f64::from(drawn.origin.1)));
        Some((drawn, location))
    }

    /// Tests (the harness draws nothing): what a frame does for the titles, for every
    /// window drawn.
    #[cfg(test)]
    pub fn draw_bar_titles(&self) {
        for visible in self.visible_windows() {
            let _ = self.bar_title(
                visible.id,
                visible.outer,
                visible.decoration,
                Scale::from(1.0),
            );
        }
    }

    /// xdg_toplevel.set_title: kept as client text; a frame comes when the rate limit
    /// allows. The title itself is never logged.
    pub fn title_changed(&mut self, toplevel: &ToplevelSurface) {
        let Some(id) = self.windows.by_surface(toplevel.wl_surface()).map(|m| m.id) else {
            return;
        };
        let text = compositor::with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| {
                    data.lock()
                        .ok()
                        .map(|d| ClientText::new(d.title.as_deref().unwrap_or("").as_bytes()))
                })
                .unwrap_or_default()
        });
        match self.titles.set(id, text, Instant::now()) {
            Some(wait) if wait.is_zero() => self.request_redraw(),
            Some(wait) => {
                let armed = self.loop_handle.insert_source(
                    Timer::from_duration(wait),
                    move |_, _, server| {
                        server.state.titles.timer_fired(id);
                        server.state.request_redraw();
                        TimeoutAction::Drop
                    },
                );
                if armed.is_err() {
                    self.titles.timer_fired(id);
                    self.request_redraw();
                }
            }
            None => {}
        }
    }
}
