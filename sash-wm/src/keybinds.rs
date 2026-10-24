// SPDX-License-Identifier: Apache-2.0

//! The minimal hardcoded keybind table used until M6 replaces it with the full
//! default table. Keys that trigger a keybind are not forwarded to clients.

use std::ffi::OsString;

use smithay::input::keyboard::{Keycode, Keysym, ModifiersState};

use crate::layout::{Direction, WindowId};

/// The keybind modifier. One constant per backend: Alt when nested (the host
/// compositor captures Super), Super on DRM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Alt,
    #[cfg_attr(not(test), expect(dead_code, reason = "the DRM backend arrives in M7"))]
    Super,
}

impl Modifier {
    /// Whether exactly this modifier (not the other main one) is held.
    pub fn held(self, mods: &ModifiersState) -> bool {
        match self {
            Modifier::Alt => mods.alt && !mods.logo,
            Modifier::Super => mods.logo && !mods.alt,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Starts foot. Debug builds only.
    SpawnTerminal,
    CloseFocused,
    Focus(Direction),
    Swap(Direction),
    Quit,
    ToggleFloating,
    FloatAll,
    TileAll,
    /// Moves the focused window by STEP.
    Move(Direction),
    /// Resizes the focused window by STEP, top-left corner fixed: Right wider, Left
    /// narrower, Down taller, Up shorter.
    Resize(Direction),
    /// Steps through windows in most-recently-used order while the modifier is held.
    CycleNext,
    CyclePrevious,
    /// Flips the monitor's default mode. Debug builds only.
    ToggleDefaultMode,
}

impl Action {
    fn debug_only(self) -> bool {
        matches!(self, Action::SpawnTerminal | Action::ToggleDefaultMode)
    }
}

struct Binding {
    shift: bool,
    ctrl: bool,
    key: Keysym,
    action: Action,
}

const fn bind(shift: bool, ctrl: bool, key: Keysym, action: Action) -> Binding {
    Binding {
        shift,
        ctrl,
        key,
        action,
    }
}

const NONE: (bool, bool) = (false, false);
const SHIFT: (bool, bool) = (true, false);
const CTRL: (bool, bool) = (false, true);
const CTRL_SHIFT: (bool, bool) = (true, true);

const fn with(mods: (bool, bool), key: Keysym, action: Action) -> Binding {
    bind(mods.0, mods.1, key, action)
}

/// mod + optional Shift / Ctrl + key, matched on the unshifted (raw) keysym.
const BINDINGS: &[Binding] = &[
    with(NONE, Keysym::Return, Action::SpawnTerminal),
    with(NONE, Keysym::q, Action::CloseFocused),
    with(NONE, Keysym::h, Action::Focus(Direction::Left)),
    with(NONE, Keysym::j, Action::Focus(Direction::Down)),
    with(NONE, Keysym::k, Action::Focus(Direction::Up)),
    with(NONE, Keysym::l, Action::Focus(Direction::Right)),
    with(SHIFT, Keysym::h, Action::Swap(Direction::Left)),
    with(SHIFT, Keysym::j, Action::Swap(Direction::Down)),
    with(SHIFT, Keysym::k, Action::Swap(Direction::Up)),
    with(SHIFT, Keysym::l, Action::Swap(Direction::Right)),
    with(SHIFT, Keysym::e, Action::Quit),
    with(NONE, Keysym::f, Action::ToggleFloating),
    with(SHIFT, Keysym::f, Action::FloatAll),
    with(SHIFT, Keysym::t, Action::TileAll),
    with(CTRL, Keysym::h, Action::Move(Direction::Left)),
    with(CTRL, Keysym::j, Action::Move(Direction::Down)),
    with(CTRL, Keysym::k, Action::Move(Direction::Up)),
    with(CTRL, Keysym::l, Action::Move(Direction::Right)),
    with(CTRL_SHIFT, Keysym::h, Action::Resize(Direction::Left)),
    with(CTRL_SHIFT, Keysym::j, Action::Resize(Direction::Down)),
    with(CTRL_SHIFT, Keysym::k, Action::Resize(Direction::Up)),
    with(CTRL_SHIFT, Keysym::l, Action::Resize(Direction::Right)),
    with(NONE, Keysym::Tab, Action::CycleNext),
    with(SHIFT, Keysym::Tab, Action::CyclePrevious),
    with(CTRL, Keysym::d, Action::ToggleDefaultMode),
];

const DEBUG_BUILD: bool = cfg!(debug_assertions);

/// Finds the action for a key press, if any. Exactly the modifier must be held, plus
/// exactly the Shift and Ctrl the binding asks for.
pub fn lookup(modifier: Modifier, mods: &ModifiersState, key: Option<Keysym>) -> Option<Action> {
    let key = key?;
    if !modifier.held(mods) {
        return None;
    }
    BINDINGS
        .iter()
        .filter(|b| DEBUG_BUILD || !b.action.debug_only())
        .find(|b| b.shift == mods.shift && b.ctrl == mods.ctrl && b.key == key)
        .map(|b| b.action)
}

/// A mod+Tab cycle in progress: a frozen most-recently-used order and the position in it.
pub struct Cycle {
    pub order: Vec<WindowId>,
    pub index: usize,
}

/// Keybind state of the compositor.
pub struct Keybinds {
    pub modifier: Modifier,
    /// Keys whose press triggered a keybind; their release is not forwarded either.
    pub(crate) swallowed: Vec<Keycode>,
    /// WAYLAND_DISPLAY for spawned clients.
    pub(crate) display_name: OsString,
    pub(crate) cycle: Option<Cycle>,
}

impl Keybinds {
    pub fn new(modifier: Modifier, display_name: OsString) -> Self {
        Self {
            modifier,
            swallowed: Vec::new(),
            display_name,
            cycle: None,
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
        let alt_ctrl = mods(true, false, false, true);
        let alt_ctrl_shift = mods(true, false, true, true);
        let l = |m| lookup(Modifier::Alt, &m, Some(Keysym::h));
        assert_eq!(l(alt), Some(Action::Focus(Direction::Left)));
        assert_eq!(l(alt_shift), Some(Action::Swap(Direction::Left)));
        assert_eq!(l(alt_ctrl), Some(Action::Move(Direction::Left)));
        assert_eq!(l(alt_ctrl_shift), Some(Action::Resize(Direction::Left)));
        assert_eq!(l(mods(false, false, false, false)), None);
        assert_eq!(l(mods(true, true, false, false)), None, "Super held too");
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
            lookup(Modifier::Alt, &alt_ctrl, Some(Keysym::q)),
            None,
            "close has no Ctrl"
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
    fn floating_bindings() {
        let alt = mods(true, false, false, false);
        let alt_shift = mods(true, false, true, false);
        assert_eq!(
            lookup(Modifier::Alt, &alt, Some(Keysym::f)),
            Some(Action::ToggleFloating)
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt_shift, Some(Keysym::f)),
            Some(Action::FloatAll)
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt_shift, Some(Keysym::t)),
            Some(Action::TileAll)
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt, Some(Keysym::Tab)),
            Some(Action::CycleNext)
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt_shift, Some(Keysym::Tab)),
            Some(Action::CyclePrevious)
        );
    }

    #[test]
    fn debug_only_bindings() {
        let alt = mods(true, false, false, false);
        let alt_ctrl = mods(true, false, false, true);
        let debug = |action| cfg!(debug_assertions).then_some(action);
        assert_eq!(
            lookup(Modifier::Alt, &alt, Some(Keysym::Return)),
            debug(Action::SpawnTerminal)
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt_ctrl, Some(Keysym::d)),
            debug(Action::ToggleDefaultMode)
        );
    }
}
