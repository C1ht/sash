// SPDX-License-Identifier: Apache-2.0

//! wl_output: the single output sash-wm shows its windows on.

use smithay::{
    delegate_output,
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    utils::{Physical, Size, Transform},
    wayland::output::OutputHandler,
};

use crate::state::Sash;

/// Nominal refresh rate in mHz. The nested window has no real mode; this is what
/// clients are told.
const NOMINAL_REFRESH_MHZ: i32 = 60_000;

impl OutputHandler for Sash {}

delegate_output!(Sash);

/// Creates the output, advertises it to clients and places it at the origin of the space.
pub fn create(state: &mut Sash, name: &str, size: Size<i32, Physical>) -> Output {
    let properties = PhysicalProperties {
        size: (0, 0).into(),
        subpixel: Subpixel::Unknown,
        make: "Sash".into(),
        model: name.into(),
    };
    let output = Output::new(name.into(), properties);
    output.create_global::<Sash>(&state.display_handle);
    set_size(&output, size);
    state.space.map_output(&output, (0, 0));
    state.output = Some(output.clone());
    output
}

/// Creates the output for a real display (DRM): its connector name, physical size in
/// millimetres and its one mode.
pub fn create_physical(state: &mut Sash, name: &str, size_mm: (i32, i32), mode: Mode) -> Output {
    let properties = PhysicalProperties {
        size: size_mm.into(),
        subpixel: Subpixel::Unknown,
        make: "Unknown".into(),
        model: name.into(),
    };
    let output = Output::new(name.into(), properties);
    output.create_global::<Sash>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));
    state.output = Some(output.clone());
    output
}

/// Sets the output's only mode. Earlier modes are removed so clients see exactly one.
pub fn set_size(output: &Output, size: Size<i32, Physical>) {
    let mode = Mode {
        size,
        refresh: NOMINAL_REFRESH_MHZ,
    };
    for old in output.modes().into_iter().filter(|old| *old != mode) {
        output.delete_mode(old);
    }
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_keeps_a_single_mode() {
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Sash".into(),
                model: "test".into(),
            },
        );
        set_size(&output, (800, 600).into());
        set_size(&output, (1024, 768).into());
        set_size(&output, (1024, 768).into());
        assert_eq!(output.modes().len(), 1);
        assert_eq!(
            output.current_mode().map(|m| m.size),
            Some((1024, 768).into())
        );
        assert_eq!(output.preferred_mode(), output.current_mode());
    }
}
