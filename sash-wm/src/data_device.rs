// SPDX-License-Identifier: Apache-2.0

//! wl_data_device_manager, advertised only because some clients (foot) refuse to start
//! without it. No clipboard or drag-and-drop behaviour beyond Smithay's defaults.

use smithay::{
    delegate_data_device,
    wayland::selection::{
        SelectionHandler,
        data_device::{
            ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
        },
    },
};

use crate::state::Sash;

impl SelectionHandler for Sash {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Sash {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.protocols.data_device
    }
}

impl ClientDndGrabHandler for Sash {}
impl ServerDndGrabHandler for Sash {}

delegate_data_device!(Sash);
