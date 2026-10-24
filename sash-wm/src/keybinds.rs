// SPDX-License-Identifier: Apache-2.0

//! The minimal hardcoded keybind table used until M6 replaces it with the full
//! default table. Keys that trigger a keybind are not forwarded to clients.

use std::ffi::OsString;

use smithay::input::keyboard::{Keycode, Keysym, ModifiersState};

use crate::layout::Direction;

/// The keybind modifier. One constant per backend: Alt when nested (the host
/// compositor captures Super), Super on DRM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Alt,
    #[cfg_attr(not(test), expect(dead_code, reason = "the DRM backend arrives in M7"))]
    Super,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Starts foot. Debug builds only.
    SpawnTerminal,
    CloseFocused,
    Focus(Direction),
    Swap(Direction),
    Quit,
}

struct Binding {
    shift: bool,
    key: Keysym,
    action: Action,
}

const fn bind(shift: bool, key: Keysym, action: Action) -> Binding {
    Binding { shift, key, action }
}

/// mod+key and mod+Shift+key, matched on the unshifted (raw) keysym.
const BINDINGS: &[Binding] = &[
    bind(false, Keysym::Return, Action::SpawnTerminal),
    bind(false, Keysym::q, Action::CloseFocused),
    bind(false, Keysym::h, Action::Focus(Direction::Left)),
    bind(false, Keysym::j, Action::Focus(Direction::Down)),
    bind(false, Keysym::k, Action::Focus(Direction::Up)),
    bind(false, Keysym::l, Action::Focus(Direction::Right)),
    bind(true, Keysym::h, Action::Swap(Direction::Left)),
    bind(true, Keysym::j, Action::Swap(Direction::Down)),
    bind(true, Keysym::k, Action::Swap(Direction::Up)),
    bind(true, Keysym::l, Action::Swap(Direction::Right)),
    bind(true, Keysym::e, Action::Quit),
];

const SPAWN_TERMINAL_ENABLED: bool = cfg!(debug_assertions);

/// Finds the action for a key press, if any. Exactly the modifier (plus Shift for
/// the Shift bindings) must be held; Ctrl or the other main modifier disables it.
pub fn lookup(modifier: Modifier, mods: &ModifiersState, key: Option<Keysym>) -> Option<Action> {
    let key = key?;
    let held = match modifier {
        Modifier::Alt => mods.alt && !mods.logo,
        Modifier::Super => mods.logo && !mods.alt,
    };
    if !held || mods.ctrl {
        return None;
    }
    BINDINGS
        .iter()
        .filter(|b| SPAWN_TERMINAL_ENABLED || b.action != Action::SpawnTerminal)
        .find(|b| b.shift == mods.shift && b.key == key)
        .map(|b| b.action)
}

/// Keybind state of the compositor.
pub struct Keybinds {
    pub modifier: Modifier,
    /// Keys whose press triggered a keybind; their release is not forwarded either.
    pub(crate) swallowed: Vec<Keycode>,
    /// WAYLAND_DISPLAY for spawned clients.
    pub(crate) display_name: OsString,
}

impl Keybinds {
    pub fn new(modifier: Modifier, display_name: OsString) -> Self {
        Self {
            modifier,
            swallowed: Vec::new(),
            display_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(alt: bool, logo: bool, shift: bool, ctrl: bool) -> ModifiersState {
        ModifiersState {
            alt,
            logo,
            shift,
            ctrl,
            ..Default::default()
        }
    }

    #[test]
    fn exact_modifiers_are_required() {
        let alt = mods(true, false, false, false);
        let alt_shift = mods(true, false, true, false);
        assert_eq!(
            lookup(Modifier::Alt, &alt, Some(Keysym::h)),
            Some(Action::Focus(Direction::Left))
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt_shift, Some(Keysym::h)),
            Some(Action::Swap(Direction::Left))
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt_shift, Some(Keysym::e)),
            Some(Action::Quit)
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt, Some(Keysym::e)),
            None,
            "quit needs Shift"
        );
        assert_eq!(
            lookup(
                Modifier::Alt,
                &mods(false, false, false, false),
                Some(Keysym::h)
            ),
            None
        );
        assert_eq!(
            lookup(
                Modifier::Alt,
                &mods(true, false, false, true),
                Some(Keysym::h)
            ),
            None,
            "Ctrl held"
        );
        assert_eq!(
            lookup(
                Modifier::Alt,
                &mods(true, true, false, false),
                Some(Keysym::h)
            ),
            None,
            "Super held too"
        );
        assert_eq!(
            lookup(
                Modifier::Super,
                &mods(false, true, false, false),
                Some(Keysym::q)
            ),
            Some(Action::CloseFocused)
        );
        assert_eq!(lookup(Modifier::Super, &alt, Some(Keysym::q)), None);
    }

    #[test]
    fn spawn_terminal_only_in_debug_builds() {
        let alt = mods(true, false, false, false);
        let expected = cfg!(debug_assertions).then_some(Action::SpawnTerminal);
        assert_eq!(lookup(Modifier::Alt, &alt, Some(Keysym::Return)), expected);
    }
}
