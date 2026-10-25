// SPDX-License-Identifier: Apache-2.0

//! wl_compositor, wl_shm and buffer handling.

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{self, CompositorClientState, CompositorHandler, CompositorState},
        shell::xdg::XDG_POPUP_ROLE,
        shm::{ShmHandler, ShmState},
    },
};

use crate::{clients::ClientState, state::Sash};

impl CompositorHandler for Sash {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.protocols.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("every client is inserted with ClientState")
            .compositor
    }

    fn commit(&mut self, surface: &WlSurface) {
        if self.reject_invalid_commit(surface) {
            return;
        }
        on_commit_buffer_handler::<Self>(surface);
        self.popups.manager.commit(surface);

        let root = root_of(surface);
        if compositor::get_role(&root) == Some(XDG_POPUP_ROLE) {
            self.popup_committed(&root);
        } else if self.cursor.is_surface(&root) {
            self.request_redraw();
        } else {
            self.root_committed(&root);
        }
    }
}

/// The surface at the top of `surface`'s subsurface tree (itself if it has no parent).
fn root_of(surface: &WlSurface) -> WlSurface {
    std::iter::successors(Some(surface.clone()), compositor::get_parent)
        .last()
        .expect("the iterator starts with `surface`")
}

impl BufferHandler for Sash {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Sash {
    fn shm_state(&self) -> &ShmState {
        &self.protocols.shm
    }
}

delegate_compositor!(Sash);
delegate_shm!(Sash);
