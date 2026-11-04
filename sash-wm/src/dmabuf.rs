// SPDX-License-Identifier: Apache-2.0

//! zwp_linux_dmabuf_v1: client buffers that live on the GPU.
//!
//! The backend enables it with the formats its renderer can import, the render node
//! as the main device, and a function that test-imports a buffer. Smithay's dispatch
//! sends most zwp_linux_buffer_params_v1 errors. Smithay renumbers planes and does
//! not know how many planes a format has, nor check the modifier, so sash-wm looks at
//! the params requests first: on create, every plane the format needs (indices 0..n)
//! must have been added (`incomplete`), and the format and modifier must be advertised
//! (`invalid_format`). Then each buffer is checked against the size limit and
//! test-imported, and the client gets `created`/`failed` (or `invalid_wl_buffer` for
//! create_immed).
//! Rendering needs nothing special: the renderer imports dmabufs like shm buffers.

use std::{collections::HashMap, os::unix::fs::MetadataExt};

use smithay::{
    backend::{
        allocator::{Buffer, Format, Fourcc, Modifier, dmabuf::Dmabuf},
        egl::{EGLDevice, ffi::egl},
        renderer::{
            ImportDma, Offscreen,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    reexports::{
        wayland_protocols::wp::linux_dmabuf::zv1::server::{
            zwp_linux_buffer_params_v1::{self, Error, ZwpLinuxBufferParamsV1},
            zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
            zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource,
            backend::{ClientId, ObjectId},
            delegate_dispatch,
            protocol::wl_buffer::WlBuffer,
        },
    },
    utils::{Buffer as BufferCoords, Size},
    wayland::dmabuf::{
        DmabufData, DmabufFeedbackBuilder, DmabufFeedbackData, DmabufGlobal, DmabufGlobalData,
        DmabufHandler, DmabufParamsData, DmabufState, ImportNotifier,
    },
};

use crate::state::Sash;

/// Largest dmabuf accepted, per side.
pub const MAX_DMABUF_SIZE: i32 = 16384;

/// Test-imports a buffer into the renderer: true if it can be drawn.
pub type Importer = Box<dyn FnMut(&Dmabuf) -> bool>;

pub struct Dmabufs {
    state: DmabufState,
    global: Option<DmabufGlobal>,
    importer: Option<Importer>,
    /// The advertised format table.
    formats: Vec<Format>,
    /// Per live params object: the plane indices added (bit per index) and the first
    /// plane's modifier; `used` once a create was checked.
    params: HashMap<ObjectId, Params>,
}

#[derive(Default)]
struct Params {
    planes: u32,
    modifier: Option<u64>,
    used: bool,
}

impl Default for Dmabufs {
    fn default() -> Self {
        Self {
            state: DmabufState::new(),
            global: None,
            importer: None,
            formats: Vec::new(),
            params: HashMap::new(),
        }
    }
}

/// How many planes a buffer of `format` has (modifiers with auxiliary planes are not
/// advertised on the supported hardware).
fn plane_count(format: Fourcc) -> u32 {
    use Fourcc as F;
    match format {
        F::Nv12
        | F::Nv21
        | F::Nv16
        | F::Nv61
        | F::Nv24
        | F::Nv42
        | F::P010
        | F::P012
        | F::P016
        | F::P210 => 2,
        F::Yuv410
        | F::Yvu410
        | F::Yuv411
        | F::Yvu411
        | F::Yuv420
        | F::Yvu420
        | F::Yuv422
        | F::Yvu422
        | F::Yuv444
        | F::Yvu444 => 3,
        _ => 1,
    }
}

impl Dmabufs {
    /// The protocol error for a create on these params, if any (None also when the
    /// format is unknown or the params were used: Smithay reports those).
    fn check_create(&mut self, params: &ObjectId, format: u32) -> Option<(Error, String)> {
        let pending = self.params.entry(params.clone()).or_default();
        if std::mem::replace(&mut pending.used, true) {
            return None;
        }
        let format = Fourcc::try_from(format).ok()?;
        if !self.formats.iter().any(|f| f.code == format) {
            return None;
        }
        let needed = plane_count(format);
        if pending.planes != (1 << needed) - 1 {
            let message = format!("{format:?} needs planes 0..{needed}");
            return Some((Error::Incomplete, message));
        }
        let modifier = Modifier::from(pending.modifier.unwrap_or(u64::MAX));
        if !self.formats.contains(&Format {
            code: format,
            modifier,
        }) {
            let message = format!("{format:?} with modifier {modifier:?} is not supported");
            return Some((Error::InvalidFormat, message));
        }
        None
    }
}

impl Sash {
    /// Advertises zwp_linux_dmabuf_v1 (version 5, with default feedback) for `formats`
    /// on the render node `main_device`.
    pub fn enable_dmabuf(&mut self, main_device: u64, formats: Vec<Format>, importer: Importer) {
        let feedback = match DmabufFeedbackBuilder::new(main_device, formats.clone()).build() {
            Ok(feedback) => feedback,
            Err(err) => {
                tracing::warn!(%err, "dmabuf feedback failed; shm only");
                return;
            }
        };
        let global = self
            .dmabufs
            .state
            .create_global_with_default_feedback::<Sash>(&self.display_handle, &feedback);
        self.dmabufs.global = Some(global);
        self.dmabufs.importer = Some(importer);
        self.dmabufs.formats = formats;
    }
}

impl DmabufHandler for Sash {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabufs.state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let size = dmabuf.size();
        let fits = size.w <= MAX_DMABUF_SIZE && size.h <= MAX_DMABUF_SIZE;
        let imported = fits
            && self
                .dmabufs
                .importer
                .as_mut()
                .is_some_and(|import| import(&dmabuf));
        if imported {
            if notifier.successful::<Sash>().is_err() {
                tracing::debug!("dmabuf client gone before its buffer was created");
            }
        } else {
            tracing::debug!(?size, format = ?dmabuf.format(), fits, "dmabuf import failed");
            notifier.failed();
        }
    }
}

// What `delegate_dmabuf!` would do, except for zwp_linux_buffer_params_v1 (below).
crate::limits::counted_global!(ZwpLinuxDmabufV1: DmabufGlobalData => DmabufState);
delegate_dispatch!(Sash: [ZwpLinuxDmabufV1: DmabufData] => DmabufState);
delegate_dispatch!(Sash: [WlBuffer: Dmabuf] => DmabufState);
delegate_dispatch!(Sash: [ZwpLinuxDmabufFeedbackV1: DmabufFeedbackData] => DmabufState);

/// Records planes and modifiers, checks creates (see the module docs), then lets
/// Smithay handle the request.
impl Dispatch<ZwpLinuxBufferParamsV1, DmabufParamsData> for Sash {
    fn request(
        state: &mut Self,
        client: &Client,
        params: &ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
        data: &DmabufParamsData,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwp_linux_buffer_params_v1::Request;
        match &request {
            Request::Add {
                plane_idx,
                modifier_hi,
                modifier_lo,
                ..
            } => {
                let pending = state.dmabufs.params.entry(params.id()).or_default();
                if *plane_idx < 32 {
                    pending.planes |= 1 << plane_idx;
                }
                pending
                    .modifier
                    .get_or_insert(((*modifier_hi as u64) << 32) | *modifier_lo as u64);
            }
            Request::Create { format, .. } | Request::CreateImmed { format, .. } => {
                if let Some((error, message)) = state.dmabufs.check_create(&params.id(), *format) {
                    tracing::debug!(%message, "dmabuf params error");
                    params.post_error(error, message);
                    return;
                }
            }
            _ => {}
        }
        <DmabufState as Dispatch<ZwpLinuxBufferParamsV1, DmabufParamsData, Self>>::request(
            state, client, params, request, data, display, data_init,
        );
    }

    fn destroyed(
        state: &mut Self,
        client: ClientId,
        params: &ZwpLinuxBufferParamsV1,
        data: &DmabufParamsData,
    ) {
        state.dmabufs.params.remove(&params.id());
        <DmabufState as Dispatch<ZwpLinuxBufferParamsV1, DmabufParamsData, Self>>::destroyed(
            state, client, params, data,
        );
    }
}

/// What a renderer can offer clients as dmabufs: its render node and the formats EGL
/// can import, provided a texture exported as a dmabuf imports back
/// (self-test). None means shm only.
pub fn offer(renderer: &mut GlesRenderer) -> Option<(u64, Vec<Format>)> {
    let device = EGLDevice::device_for_display(renderer.egl_context().display()).ok()?;
    let node = device.render_device_path().ok()?;
    let main_device = std::fs::metadata(&node).ok()?.rdev();
    let formats: Vec<Format> = renderer.dmabuf_formats().into_iter().collect();
    let Some(test) = export_texture(renderer, Fourcc::Abgr8888, (16, 16).into(), |_, _| true)
    else {
        tracing::warn!("dmabuf self-test: exporting a texture failed; shm only");
        return None;
    };
    if let Err(err) = renderer.import_dmabuf(&test, None) {
        tracing::warn!(%err, "dmabuf self-test: importing it back failed; shm only");
        return None;
    }
    tracing::info!(
        node = %node.display(),
        formats = formats.len(),
        "dmabuf self-test passed"
    );
    Some((main_device, formats))
}

/// Creates a new `size` texture of `format` cleared by `clear`, then
/// exports it as a dmabuf (EGL_MESA_image_dma_buf_export). Used by the startup
/// self-test and the GPU tests.
pub fn export_texture(
    renderer: &mut GlesRenderer,
    format: Fourcc,
    size: Size<i32, BufferCoords>,
    clear: impl FnOnce(&mut GlesRenderer, &mut GlesTexture) -> bool,
) -> Option<Dmabuf> {
    let mut texture: GlesTexture = renderer.create_buffer(format, size).ok()?;
    if !clear(renderer, &mut texture) {
        return None;
    }
    let display = renderer.egl_context().display().clone();
    let display_handle = display.get_display_handle();
    let context = renderer.egl_context().get_context_handle();
    const GL_TEXTURE_2D: egl::types::EGLenum = 0x30B1; // EGL_GL_TEXTURE_2D_KHR
    let attributes = [egl::NONE as egl::types::EGLint];
    // Safety: the display, context and texture are alive; the image is destroyed below.
    let image = unsafe {
        egl::CreateImageKHR(
            display_handle.handle,
            context,
            GL_TEXTURE_2D,
            texture.tex_id() as usize as egl::types::EGLClientBuffer,
            attributes.as_ptr(),
        )
    };
    if image.is_null() {
        return None;
    }
    let dmabuf = display.create_dmabuf_from_image(image, size, false).ok();
    // Safety: the image was created above on this display; the dmabuf keeps its own fds.
    unsafe { egl::DestroyImageKHR(display_handle.handle, image) };
    dmabuf
}
