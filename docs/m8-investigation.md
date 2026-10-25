<!-- SPDX-License-Identifier: Apache-2.0 -->
# M8 investigation: linux-dmabuf, explicit sync, cursors

Findings on this machine (Arch, 2026-10-25) and in Smithay 0.7.0, before any M8 code.

## Hardware and driver

- GPU: NVIDIA GeForce RTX 4070 (AD104), the only GPU in use. The board's AMD Raphael
  iGPU has no driver loaded (`amdgpu` is not loaded); `/dev/dri` has `card1` and
  `renderD128`, both the NVIDIA GPU.
- Driver: NVIDIA open kernel module 615.78.08 (`nvidia-open`, `nvidia-utils`),
  `nvidia_drm modeset=Y`, `fbdev=Y`. One monitor, on HDMI-A-1.
- Wayland EGL platforms: egl-wayland2 1.0.2 (preferred, priority 09) and egl-wayland
  1.1.24; egl-gbm 1.1.4 provides GBM for the DRM backend (M9).

## zwp_linux_dmabuf_v1 in Smithay 0.7.0

- `DmabufState` creates the global. With a default feedback
  (`create_global_with_default_feedback`) it is **version 5** (feedback v4+: main
  device, tranches, formats and modifiers); without feedback, version 3.
  `DmabufFeedbackBuilder::new(main_device, formats)` builds the feedback; per-surface
  feedback is optional (`DmabufHandler::new_surface_feedback`).
- Smithay's params dispatch already sends the protocol errors: `plane_idx` (index >=
  4, `MAX_PLANES`), `plane_set` (a plane index twice), `invalid_format` (format or
  modifier not in the global's table; non-matching modifiers between planes on
  version 5), `already_used`, `incomplete` (missing plane), `invalid_dimensions`,
  `out_of_bounds` (offset/stride beyond the fd's size).
- The compositor implements `DmabufHandler::dmabuf_imported(global, dmabuf,
  notifier)`: it test-imports the buffer and answers `notifier.successful()` or
  `failed()`. For `create` a failure is the `failed` event; for `create_immed` it is
  `invalid_wl_buffer` (the client is killed), as the protocol says.
- Destroying params after `create_immed` is allowed (no error); the wl_buffer stays
  valid.
- Import into GLES: `GlesRenderer` implements `ImportDma` (EGL image from the planes,
  `EGL_EXT_image_dma_buf_import[_modifiers]`). Surface buffers go through
  `ImportAll` in `render_elements_from_surface_tree`, the same call sash-wm already
  uses for shm, so tile clipping, floating, popups and the snap preview need no
  dmabuf-specific path. Buffers are released by Smithay's renderer surface state when
  they are replaced, as for shm.
- `renderer.dmabuf_formats()` lists what the renderer can sample.
- Export: `EGLDisplay::create_dmabuf_from_image` (`EGL_MESA_image_dma_buf_export`,
  present on this driver) turns a GL texture into a real dmabuf. That is the
  self-test at startup and the GPU tests, with no new dependency.

## What the NVIDIA driver imports (measured)

`cargo test -p sash-wm -- --ignored --nocapture gpu_dmabuf_formats` (headless EGL
device on `/dev/dri/renderD128`):

- 48 formats: ARGB/XRGB/ABGR/XBGR/RGBA/BGRA 8888, 2101010 and 1555 variants, 16-bit
  float `AB4H`/`XB4H`, `RGB8`, `BG24`, `RG24`, `R8`, `R16`, `RG88`, `RG16`, `RG32`,
  `BG16`, YUV `NV12`, `NV16`, `NV21`, `NV24`, `P010`, `P012`, `P210`, `UYVY`, `YU12`.
- Each with the same 14 modifiers: 12 NVIDIA block-linear ones
  (`0x0300000000606010`..`015`, `0x0300000000e08010`..`015`), `LINEAR` (0) and
  `INVALID` (implicit, `0x00ffffffffffffff`).
- EGL extensions present: `EGL_EXT_image_dma_buf_import`,
  `EGL_EXT_image_dma_buf_import_modifiers`, `EGL_MESA_image_dma_buf_export`,
  `EGL_ANDROID_native_fence_sync`, `EGL_KHR_fence_sync`, `EGL_KHR_wait_sync`,
  `EGL_NV_nvrm_fence_sync`.

Owner decision: advertise the formats and modifiers EGL reports as importable,
after a startup self-test (export a GL texture as a dmabuf and import it back); if
the self-test fails, no dmabuf global (shm only).

## What the nested winit backend can offer

- The host (Hyprland 0.56) gives sash-wm, as a client, `zwp_linux_dmabuf_v1` v5,
  `wp_linux_drm_syncobj_manager_v1` v1, `wp_cursor_shape_manager_v1` v2 and
  `wp_presentation` v2 (`wayland-info`).
- sash-wm renders client dmabufs into its own EGL window surface; it does not pass
  client buffers through to the host. So the main device of the feedback is the
  render node of sash-wm's own EGL display (`renderD128`), and one tranche: the
  renderer's importable formats.
- Today (M7) GL clients cannot use the NVIDIA driver on sash-wm: without a dmabuf
  global, NVIDIA's EGL Wayland platform fails and they fall back to Mesa's software
  renderer with wl_shm buffers (kitty: 77 wl_shm requests, no dmabuf, and
  `MESA-EGL: failed to get driver name` warnings). foot always uses wl_shm (it renders
  on the CPU by design), so dmabuf will not change foot.

## Explicit sync (wp_linux_drm_syncobj_v1), report only

- Smithay 0.7.0 implements it (`wayland::drm_syncobj`), but only with the
  `backend_drm` feature (drm crates) and an open DRM device fd
  (`DrmSyncobjState::new(display, DrmDeviceFd)`, exposed only if
  `supports_syncobj_eventfd`). The implementation waits for acquire points with a
  blocker before a commit applies and signals release points when the buffer is
  dropped.
- NVIDIA: the driver supports syncobj timelines (since 555); egl-wayland2 uses the
  protocol when the compositor offers it. Without it, the client side relies on
  implicit synchronisation of the dmabuf, which the NVIDIA driver has historically
  not provided reliably; the known symptom is flicker or frames shown before
  rendering finished. Whether that happens with this driver (615) and egl-wayland2
  1.0.2 on sash-wm is measured in M8 (kitty, Firefox) and reported.
- Hyprland offers `wp_linux_drm_syncobj_manager_v1` v1 to its clients, so sash-wm's
  own frames to the host are explicitly synced by NVIDIA's EGL already.
- Building it means enabling `backend_drm` (new dependencies, which M9 needs anyway).
  Not built without the owner's approval.

## Cursors

- Today sash-wm has no cursor handling: `SeatHandler::cursor_image` keeps Smithay's
  empty default, so `wl_pointer.set_cursor` is ignored. Nested, the host shows its
  default arrow over the whole window; a client's cursor surface is never drawn.
- `wp_cursor_shape_manager_v1`: Smithay 0.7.0 has `CursorShapeManagerState` (global
  version 2) and `delegate_cursor_shape!`. A `set_shape` from a client whose surface
  has pointer focus (matching enter serial) becomes
  `SeatHandler::cursor_image(CursorImageStatus::Named(icon))`; without pointer
  focus it is ignored by Smithay. The device dispatch requires `TabletSeatHandler`
  (default methods suffice).
- Nested: winit's `Window::set_cursor(CursorIcon)` and `set_cursor_visible` set the
  host cursor; `CursorIcon` is the same type Smithay uses.
- Owner decision: cursor surfaces are drawn by sash-wm at the pointer (hotspot from
  the cursor role) with the host cursor hidden while one is set; named shapes go to
  the host cursor; `hidden` hides it. The DRM backend (M9) will use the same software
  cursor, or a cursor plane.

## Limits (constants)

- Planes per buffer: 4 (`MAX_PLANES`, Smithay; the protocol's plane index limit).
- Buffer size: at most 16384 x 16384; larger dmabufs fail to import
  (`failed`/`invalid_wl_buffer`).
