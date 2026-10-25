// SPDX-License-Identifier: Apache-2.0

//! Input devices on DRM (libinput): the fixed defaults applied when a device appears
//! (owner decision: no input settings yet). Tap-to-click on, natural scrolling off,
//! libinput's default pointer speed. Devices are tracked so a removal is noticed.

use std::collections::BTreeSet;

/// What the defaults need from a device; libinput's devices and test fakes.
pub trait InputDevice {
    fn sysname(&self) -> String;
    fn tap_finger_count(&self) -> u32;
    fn set_tap(&mut self, enabled: bool);
    fn has_natural_scroll(&self) -> bool;
    fn set_natural_scroll(&mut self, enabled: bool);
    fn has_pointer_speed(&self) -> bool;
    fn set_pointer_speed(&mut self, speed: f64);
}

/// libinput's default pointer speed (-1 to 1).
pub const POINTER_SPEED: f64 = 0.0;

#[derive(Debug, Default)]
pub struct Devices {
    known: BTreeSet<String>,
}

impl Devices {
    /// A device appeared: apply the defaults. Returns false if it was already known.
    pub fn added(&mut self, device: &mut impl InputDevice) -> bool {
        if !self.known.insert(device.sysname()) {
            return false;
        }
        if device.tap_finger_count() > 0 {
            device.set_tap(true);
        }
        if device.has_natural_scroll() {
            device.set_natural_scroll(false);
        }
        if device.has_pointer_speed() {
            device.set_pointer_speed(POINTER_SPEED);
        }
        true
    }

    /// A device went away. Returns false if it was not known.
    pub fn removed(&mut self, device: &impl InputDevice) -> bool {
        self.known.remove(&device.sysname())
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.known.len()
    }
}

impl InputDevice for smithay::reexports::input::Device {
    fn sysname(&self) -> String {
        smithay::reexports::input::Device::sysname(self).to_string()
    }

    fn tap_finger_count(&self) -> u32 {
        self.config_tap_finger_count()
    }

    fn set_tap(&mut self, enabled: bool) {
        let _ = self.config_tap_set_enabled(enabled);
    }

    fn has_natural_scroll(&self) -> bool {
        self.config_scroll_has_natural_scroll()
    }

    fn set_natural_scroll(&mut self, enabled: bool) {
        let _ = self.config_scroll_set_natural_scroll_enabled(enabled);
    }

    fn has_pointer_speed(&self) -> bool {
        self.config_accel_is_available()
    }

    fn set_pointer_speed(&mut self, speed: f64) {
        let _ = self.config_accel_set_speed(speed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        name: &'static str,
        tap_fingers: u32,
        natural: Option<bool>,
        speed: Option<f64>,
        tap: Option<bool>,
    }

    impl InputDevice for Fake {
        fn sysname(&self) -> String {
            self.name.into()
        }
        fn tap_finger_count(&self) -> u32 {
            self.tap_fingers
        }
        fn set_tap(&mut self, enabled: bool) {
            self.tap = Some(enabled);
        }
        fn has_natural_scroll(&self) -> bool {
            self.natural.is_some()
        }
        fn set_natural_scroll(&mut self, enabled: bool) {
            self.natural = Some(enabled);
        }
        fn has_pointer_speed(&self) -> bool {
            self.speed.is_some()
        }
        fn set_pointer_speed(&mut self, speed: f64) {
            self.speed = Some(speed);
        }
    }

    #[test]
    fn a_touchpad_gets_the_defaults() {
        let mut devices = Devices::default();
        let mut touchpad = Fake {
            name: "event7",
            tap_fingers: 3,
            natural: Some(true),
            speed: Some(0.7),
            tap: None,
        };
        assert!(devices.added(&mut touchpad));
        assert_eq!(touchpad.tap, Some(true), "tap-to-click on");
        assert_eq!(touchpad.natural, Some(false), "natural scrolling off");
        assert_eq!(touchpad.speed, Some(POINTER_SPEED));
    }

    #[test]
    fn a_keyboard_is_left_alone() {
        let mut devices = Devices::default();
        let mut keyboard = Fake {
            name: "event3",
            ..Fake::default()
        };
        assert!(devices.added(&mut keyboard));
        assert_eq!(keyboard.tap, None);
        assert_eq!(keyboard.natural, None);
        assert_eq!(keyboard.speed, None);
    }

    #[test]
    fn add_and_remove() {
        let mut devices = Devices::default();
        let mut mouse = Fake {
            name: "event5",
            natural: Some(false),
            speed: Some(0.0),
            ..Fake::default()
        };
        let mut keyboard = Fake {
            name: "event3",
            ..Fake::default()
        };
        assert!(devices.added(&mut mouse));
        assert!(devices.added(&mut keyboard));
        assert!(!devices.added(&mut mouse), "already known");
        assert_eq!(devices.count(), 2);
        assert!(devices.removed(&mouse));
        assert!(!devices.removed(&mouse), "removed twice");
        assert_eq!(devices.count(), 1);
        // Plugged in again: the defaults are applied again.
        mouse.natural = Some(true);
        assert!(devices.added(&mut mouse));
        assert_eq!(mouse.natural, Some(false));
    }
}
