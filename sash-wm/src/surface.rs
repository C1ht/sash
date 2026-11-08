// SPDX-License-Identifier: Apache-2.0

//! wl_compositor, wl_shm and buffer handling.

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
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

    fn destroyed(&mut self, surface: &WlSurface) {
        // A layer surface's wl_surface gone before its layer_surface: unmapped.
        self.forget_layer(surface);
    }

    fn commit(&mut self, surface: &WlSurface) {
        if self.reject_invalid_commit(surface) {
            return;
        }
        on_commit_buffer_handler::<Self>(surface);
        self.popups.manager.commit(surface);
        // Any commit can change what is under the pointer (focus.rs).
        self.focus.geometry_changed();

        let root = root_of(surface);
        if compositor::get_role(&root) == Some(XDG_POPUP_ROLE) {
            self.popup_committed(&root);
        } else if self.cursor.is_surface(&root) {
            self.request_redraw();
        } else if self.drag_icon_committed(&root) {
            // The drag icon: drawn at the pointer, redrawn.
        } else if self.layer_committed(&root) {
            // A layer surface (layer_shell.rs).
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

// What `delegate_compositor!` and `delegate_shm!` do, with binds counted (limits.rs).
crate::limits::counted_global!(smithay::reexports::wayland_server::protocol::wl_compositor::WlCompositor: () => smithay::wayland::compositor::CompositorState);
crate::limits::counted_global!(smithay::reexports::wayland_server::protocol::wl_subcompositor::WlSubcompositor: () => smithay::wayland::compositor::CompositorState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_compositor::WlCompositor: ()] => smithay::wayland::compositor::CompositorState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_surface::WlSurface: smithay::wayland::compositor::SurfaceUserData] => smithay::wayland::compositor::CompositorState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_region::WlRegion: smithay::wayland::compositor::RegionUserData] => smithay::wayland::compositor::CompositorState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_callback::WlCallback: ()] => smithay::wayland::compositor::CompositorState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_subcompositor::WlSubcompositor: ()] => smithay::wayland::compositor::CompositorState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_subsurface::WlSubsurface: smithay::wayland::compositor::SubsurfaceUserData] => smithay::wayland::compositor::CompositorState);
crate::limits::counted_global!(smithay::reexports::wayland_server::protocol::wl_shm::WlShm: () => smithay::wayland::shm::ShmState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_shm::WlShm: ()] => smithay::wayland::shm::ShmState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_shm_pool::WlShmPool: smithay::wayland::shm::ShmPoolUserData] => smithay::wayland::shm::ShmState);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer: smithay::wayland::shm::ShmBufferUserData] => smithay::wayland::shm::ShmState);
