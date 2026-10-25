// SPDX-License-Identifier: Apache-2.0

//! The default keybind table: every keyboard and mouse binding sash-wm has, as data.
//! docs/keybinds.md is generated from it (a test fails if the file is out of date).
//! Keys that trigger a binding are not forwarded to clients; nor are mod+clicks.

use std::ffi::OsString;

use smithay::input::keyboard::{Keycode, Keysym, ModifiersState};

use crate::{
    layout::{Direction, WindowId, ZoneId},
    seat::{BTN_LEFT, BTN_RIGHT},
};

/// The keybind modifier. One constant per backend: Alt when nested (the host
/// compositor captures Super), Super on DRM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Alt,
    #[cfg_attr(not(test), expect(dead_code, reason = "the DRM backend arrives in M9"))]
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
    /// Resizes the focused window by STEP: Right wider, Left narrower, Down taller, Up
    /// shorter (floating: top-left corner fixed; tiled: the right or bottom split).
    Resize(Direction),
    /// mod+arrow: snap by the state table (layout::snap_step).
    Snap(Direction),
    /// mod+Y/U/B/N: snap straight to a quarter zone, from any state but fullscreen.
    SnapQuarter(ZoneId),
    /// Steps through windows in most-recently-used order while the modifier is held.
    CycleNext,
    CyclePrevious,
    /// Flips the monitor's default mode. Debug builds only.
    ToggleDefaultMode,
    /// Pointer: drag to move the window under the pointer.
    MoveDrag,
    /// Pointer: drag to resize the window under the pointer (3x3 grid).
    ResizeDrag,
    /// Pointer: a click focuses and raises the window under the pointer.
    ClickFocus,
}

/// Which modifiers a binding needs, besides nothing else being held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mods {
    /// No modifier (plain clicks).
    None,
    Mod,
    ModShift,
    ModCtrl,
    ModCtrlShift,
}

impl Mods {
    #[cfg(test)]
    fn label(self) -> &'static str {
        match self {
            Mods::None => "",
            Mods::Mod => "mod+",
            Mods::ModShift => "mod+Shift+",
            Mods::ModCtrl => "mod+Ctrl+",
            Mods::ModCtrlShift => "mod+Ctrl+Shift+",
        }
    }

    /// The set held now. Without the keybind modifier it is None, whatever else is
    /// held (only plain clicks use None).
    fn held(modifier: Modifier, mods: &ModifiersState) -> Mods {
        if !modifier.held(mods) {
            return Mods::None;
        }
        match (mods.ctrl, mods.shift) {
            (false, false) => Mods::Mod,
            (false, true) => Mods::ModShift,
            (true, false) => Mods::ModCtrl,
            (true, true) => Mods::ModCtrlShift,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A key, matched on its unshifted (raw) keysym.
    Key(Keysym),
    /// A pointer button (linux input event code).
    Button(u32),
    /// Any pointer button.
    AnyButton,
}

#[cfg(test)]
impl Trigger {
    fn label(self) -> String {
        match self {
            Trigger::Key(key) => match key {
                Keysym::Return => "Return".into(),
                Keysym::Tab => "Tab".into(),
                Keysym::Left => "Left".into(),
                Keysym::Right => "Right".into(),
                Keysym::Up => "Up".into(),
                Keysym::Down => "Down".into(),
                other => char::from_u32(other.raw())
                    .filter(char::is_ascii_lowercase)
                    .map(|c| c.to_ascii_uppercase().to_string())
                    .unwrap_or_else(|| format!("{other:?}")),
            },
            Trigger::Button(BTN_LEFT) => "left drag".into(),
            Trigger::Button(BTN_RIGHT) => "right drag".into(),
            Trigger::Button(code) => format!("button {code:#x}"),
            Trigger::AnyButton => "click".into(),
        }
    }
}

pub struct Binding {
    pub trigger: Trigger,
    pub mods: Mods,
    pub action: Action,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the docs/keybinds.md generator")
    )]
    pub description: &'static str,
    pub debug_only: bool,
}

const fn key(mods: Mods, key: Keysym, action: Action, description: &'static str) -> Binding {
    Binding {
        trigger: Trigger::Key(key),
        mods,
        action,
        description,
        debug_only: false,
    }
}

const fn debug_key(mods: Mods, k: Keysym, action: Action, description: &'static str) -> Binding {
    Binding {
        debug_only: true,
        ..key(mods, k, action, description)
    }
}

const fn pointer(
    mods: Mods,
    trigger: Trigger,
    action: Action,
    description: &'static str,
) -> Binding {
    Binding {
        trigger,
        mods,
        action,
        description,
        debug_only: false,
    }
}

use Direction::{Down, Left, Right, Up};
use Mods::{Mod, ModCtrl, ModCtrlShift, ModShift};

/// The default keybind table.
pub const TABLE: &[Binding] = &[
    key(
        Mod,
        Keysym::h,
        Action::Focus(Left),
        "Focus the window to the left",
    ),
    key(
        Mod,
        Keysym::j,
        Action::Focus(Down),
        "Focus the window below",
    ),
    key(Mod, Keysym::k, Action::Focus(Up), "Focus the window above"),
    key(
        Mod,
        Keysym::l,
        Action::Focus(Right),
        "Focus the window to the right",
    ),
    key(
        Mod,
        Keysym::Tab,
        Action::CycleNext,
        "Cycle windows, most recent first (hold mod; release to choose)",
    ),
    key(
        ModShift,
        Keysym::Tab,
        Action::CyclePrevious,
        "Cycle windows backwards (hold mod)",
    ),
    key(
        ModShift,
        Keysym::h,
        Action::Swap(Left),
        "Swap the tiled window with its neighbour to the left",
    ),
    key(
        ModShift,
        Keysym::j,
        Action::Swap(Down),
        "Swap the tiled window with its neighbour below",
    ),
    key(
        ModShift,
        Keysym::k,
        Action::Swap(Up),
        "Swap the tiled window with its neighbour above",
    ),
    key(
        ModShift,
        Keysym::l,
        Action::Swap(Right),
        "Swap the tiled window with its neighbour to the right",
    ),
    key(
        Mod,
        Keysym::f,
        Action::ToggleFloating,
        "Toggle floating for the focused window",
    ),
    key(
        ModShift,
        Keysym::f,
        Action::FloatAll,
        "Float all windows on the monitor",
    ),
    key(
        ModShift,
        Keysym::t,
        Action::TileAll,
        "Tile all windows on the monitor",
    ),
    key(
        ModCtrl,
        Keysym::h,
        Action::Move(Left),
        "Move the window 40 px left (floats a tiled window)",
    ),
    key(
        ModCtrl,
        Keysym::j,
        Action::Move(Down),
        "Move the window 40 px down (floats a tiled window)",
    ),
    key(
        ModCtrl,
        Keysym::k,
        Action::Move(Up),
        "Move the window 40 px up (floats a tiled window)",
    ),
    key(
        ModCtrl,
        Keysym::l,
        Action::Move(Right),
        "Move the window 40 px right (floats a tiled window)",
    ),
    key(
        ModCtrlShift,
        Keysym::h,
        Action::Resize(Left),
        "Make the window 40 px narrower (tiled: moves the split at its right edge)",
    ),
    key(
        ModCtrlShift,
        Keysym::j,
        Action::Resize(Down),
        "Make the window 40 px taller (tiled: moves the split at its bottom edge)",
    ),
    key(
        ModCtrlShift,
        Keysym::k,
        Action::Resize(Up),
        "Make the window 40 px shorter (tiled: moves the split at its bottom edge)",
    ),
    key(
        ModCtrlShift,
        Keysym::l,
        Action::Resize(Right),
        "Make the window 40 px wider (tiled: moves the split at its right edge)",
    ),
    key(
        Mod,
        Keysym::Left,
        Action::Snap(Left),
        "Snap: left half, then bottom-left (see the snap table)",
    ),
    key(
        Mod,
        Keysym::Right,
        Action::Snap(Right),
        "Snap: right half, then bottom-right",
    ),
    key(
        Mod,
        Keysym::Up,
        Action::Snap(Up),
        "Snap: maximize, or the top quarter of a half",
    ),
    key(
        Mod,
        Keysym::Down,
        Action::Snap(Down),
        "Snap: quarter to half, then restore",
    ),
    key(
        Mod,
        Keysym::y,
        Action::SnapQuarter(ZoneId::TOP_LEFT),
        "Snap to the top-left quarter (a tiled window floats first)",
    ),
    key(
        Mod,
        Keysym::u,
        Action::SnapQuarter(ZoneId::TOP_RIGHT),
        "Snap to the top-right quarter",
    ),
    key(
        Mod,
        Keysym::b,
        Action::SnapQuarter(ZoneId::BOTTOM_LEFT),
        "Snap to the bottom-left quarter",
    ),
    key(
        Mod,
        Keysym::n,
        Action::SnapQuarter(ZoneId::BOTTOM_RIGHT),
        "Snap to the bottom-right quarter",
    ),
    key(
        Mod,
        Keysym::q,
        Action::CloseFocused,
        "Close the focused window",
    ),
    key(ModShift, Keysym::e, Action::Quit, "Quit sash-wm"),
    debug_key(Mod, Keysym::Return, Action::SpawnTerminal, "Start foot"),
    debug_key(
        ModCtrl,
        Keysym::d,
        Action::ToggleDefaultMode,
        "Toggle the monitor's default mode",
    ),
    pointer(
        Mod,
        Trigger::Button(BTN_LEFT),
        Action::MoveDrag,
        "Move the window; a tiled window floats; hold Shift to turn edge snapping off (keyboard: mod+Ctrl+H/J/K/L, mod+arrows)",
    ),
    pointer(
        Mod,
        Trigger::Button(BTN_RIGHT),
        Action::ResizeDrag,
        "Resize from the pointer's cell of a 3x3 grid; tiled: moves splits (keyboard: mod+Ctrl+Shift+H/J/K/L)",
    ),
    pointer(
        Mods::None,
        Trigger::AnyButton,
        Action::ClickFocus,
        "Focus and raise the window; the click is delivered (keyboard: mod+H/J/K/L, mod+Tab)",
    ),
];

const DEBUG_BUILD: bool = cfg!(debug_assertions);

fn active(binding: &&Binding) -> bool {
    DEBUG_BUILD || !binding.debug_only
}

/// The action for a key press, if any. The held modifiers must match the binding's
/// set exactly.
pub fn lookup(modifier: Modifier, mods: &ModifiersState, key: Option<Keysym>) -> Option<Action> {
    let key = key?;
    let held = Mods::held(modifier, mods);
    TABLE
        .iter()
        .filter(active)
        .find(|b| b.mods == held && b.trigger == Trigger::Key(key))
        .map(|b| b.action)
}

/// The action for a pointer button press, if any. Shift is ignored: holding it during
/// a drag turns edge snapping off.
pub fn lookup_button(modifier: Modifier, mods: &ModifiersState, button: u32) -> Option<Action> {
    let mods = ModifiersState {
        shift: false,
        ..*mods
    };
    let held = Mods::held(modifier, &mods);
    TABLE
        .iter()
        .filter(active)
        .filter(|b| b.mods == held)
        .find(|b| b.trigger == Trigger::Button(button) || b.trigger == Trigger::AnyButton)
        .map(|b| b.action)
}

/// docs/keybinds.md, generated from the table by the test below.
#[cfg(test)]
fn markdown() -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "<!-- SPDX-License-Identifier: Apache-2.0 -->\n\
         <!-- Generated from sash-wm/src/keybinds.rs. Do not edit: run\n     \
         SASH_WRITE_KEYBINDS_DOC=1 cargo test keybinds_doc -->\n\n\
         # Keybinds\n\n\
         **mod** is Alt when sash-wm runs nested (the host keeps Super) and Super on a real \
         session. Keys that trigger a binding are not passed to the focused app, and \
         neither are mod+clicks.\n\n\
         | Chord | Modifier set | Action | Builds |\n\
         |---|---|---|---|\n",
    );
    for b in TABLE {
        let set = match b.mods {
            Mods::None => "no mod",
            Mods::Mod => "mod",
            Mods::ModShift => "mod+Shift",
            Mods::ModCtrl => "mod+Ctrl",
            Mods::ModCtrlShift => "mod+Ctrl+Shift",
        };
        let builds = if b.debug_only { "debug only" } else { "all" };
        let _ = writeln!(
            out,
            "| {}{} | {set} | {} | {builds} |",
            b.mods.label(),
            b.trigger.label(),
            b.description
        );
    }
    out
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
        assert_eq!(
            lookup(Modifier::Alt, &alt, Some(Keysym::Up)),
            Some(Action::Snap(Direction::Up))
        );
        assert_eq!(
            lookup(Modifier::Alt, &alt_shift, Some(Keysym::Up)),
            None,
            "no Shift+arrow binding"
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

    #[test]
    fn pointer_bindings() {
        let alt = mods(true, false, false, false);
        let alt_shift = mods(true, false, true, false);
        let none = mods(false, false, false, false);
        let b = |m, button| lookup_button(Modifier::Alt, &m, button);
        assert_eq!(b(alt, BTN_LEFT), Some(Action::MoveDrag));
        assert_eq!(
            b(alt_shift, BTN_LEFT),
            Some(Action::MoveDrag),
            "Shift is ignored"
        );
        assert_eq!(b(alt, BTN_RIGHT), Some(Action::ResizeDrag));
        assert_eq!(b(alt, 0x112), None, "mod+middle is unbound");
        assert_eq!(b(none, BTN_RIGHT), Some(Action::ClickFocus));
        assert_eq!(
            b(mods(false, false, false, true), 0x112),
            Some(Action::ClickFocus)
        );
    }

    #[test]
    fn no_duplicate_chords() {
        for (i, a) in TABLE.iter().enumerate() {
            for b in &TABLE[i + 1..] {
                let overlap = a.trigger == b.trigger
                    || matches!(
                        (a.trigger, b.trigger),
                        (Trigger::AnyButton, Trigger::Button(_))
                            | (Trigger::Button(_), Trigger::AnyButton)
                    );
                assert!(
                    !(a.mods == b.mods && overlap),
                    "duplicate chord {}{}: {:?} and {:?}",
                    a.mods.label(),
                    a.trigger.label(),
                    a.action,
                    b.action
                );
            }
        }
    }

    /// Run with SASH_WRITE_KEYBINDS_DOC=1 to rewrite the file.
    #[test]
    fn keybinds_doc_is_up_to_date() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/keybinds.md");
        let expected = markdown();
        if std::env::var_os("SASH_WRITE_KEYBINDS_DOC").is_some() {
            std::fs::write(path, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            actual == expected,
            "docs/keybinds.md is out of date: run SASH_WRITE_KEYBINDS_DOC=1 cargo test keybinds_doc"
        );
    }
}
