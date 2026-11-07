// SPDX-License-Identifier: Apache-2.0

//! Input as sash-ui's widgets see it: logical pixel positions and the keys that
//! navigate and activate. Typed text is not part of it (no text field yet).

/// The keys widgets use. Shift+Tab arrives as `BackTab`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Enter,
    Space,
    Escape,
}

/// The primary pointer button only; other buttons are ignored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pointer {
    Motion { x: f64, y: f64 },
    Leave,
    Press { x: f64, y: f64 },
    Release { x: f64, y: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Pointer(Pointer),
    Key(Key),
}
