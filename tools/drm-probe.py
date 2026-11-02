# SPDX-License-Identifier: Apache-2.0
#
# Read-only DRM probe (M9 stage A): lists DRM devices, connectors with their modes
# (preferred first), CRTCs, planes with their type and formats, and the device caps
# the backend needs. Queries only: it never takes DRM master (it checks right after
# opening, and drops master and exits if the kernel handed it over), never sets a mode,
# and uses drmModeGetConnectorCurrent, which does not trigger a hardware probe. The
# only state it sets is per-file client caps (universal planes, atomic), which affect
# nothing outside its own file descriptor.
#
# Usage: python3 -I tools/drm-probe.py [--all-modes] [/dev/dri/cardN ...]   (default: every card)

import ctypes as C
import glob
import os
import struct
import sys

drm = C.CDLL("libdrm.so.2", use_errno=True)

class ModeInfo(C.Structure):
    _fields_ = [("clock", C.c_uint32),
                ("hdisplay", C.c_uint16), ("hsync_start", C.c_uint16), ("hsync_end", C.c_uint16),
                ("htotal", C.c_uint16), ("hskew", C.c_uint16),
                ("vdisplay", C.c_uint16), ("vsync_start", C.c_uint16), ("vsync_end", C.c_uint16),
                ("vtotal", C.c_uint16), ("vscan", C.c_uint16),
                ("vrefresh", C.c_uint32), ("flags", C.c_uint32), ("type", C.c_uint32),
                ("name", C.c_char * 32)]

class Res(C.Structure):
    _fields_ = [("count_fbs", C.c_int), ("fbs", C.POINTER(C.c_uint32)),
                ("count_crtcs", C.c_int), ("crtcs", C.POINTER(C.c_uint32)),
                ("count_connectors", C.c_int), ("connectors", C.POINTER(C.c_uint32)),
                ("count_encoders", C.c_int), ("encoders", C.POINTER(C.c_uint32)),
                ("min_width", C.c_uint32), ("max_width", C.c_uint32),
                ("min_height", C.c_uint32), ("max_height", C.c_uint32)]

class Connector(C.Structure):
    _fields_ = [("connector_id", C.c_uint32), ("encoder_id", C.c_uint32),
                ("connector_type", C.c_uint32), ("connector_type_id", C.c_uint32),
                ("connection", C.c_int), ("mm_width", C.c_uint32), ("mm_height", C.c_uint32),
                ("subpixel", C.c_int), ("count_modes", C.c_int), ("modes", C.POINTER(ModeInfo)),
                ("count_props", C.c_int), ("props", C.POINTER(C.c_uint32)),
                ("prop_values", C.POINTER(C.c_uint64)),
                ("count_encoders", C.c_int), ("encoders", C.POINTER(C.c_uint32))]

class Crtc(C.Structure):
    _fields_ = [("crtc_id", C.c_uint32), ("buffer_id", C.c_uint32), ("x", C.c_uint32),
                ("y", C.c_uint32), ("width", C.c_uint32), ("height", C.c_uint32),
                ("mode_valid", C.c_int), ("mode", ModeInfo), ("gamma_size", C.c_int)]

class PlaneRes(C.Structure):
    _fields_ = [("count_planes", C.c_uint32), ("planes", C.POINTER(C.c_uint32))]

class Plane(C.Structure):
    _fields_ = [("count_formats", C.c_uint32), ("formats", C.POINTER(C.c_uint32)),
                ("plane_id", C.c_uint32), ("crtc_id", C.c_uint32), ("fb_id", C.c_uint32),
                ("crtc_x", C.c_uint32), ("crtc_y", C.c_uint32), ("x", C.c_uint32),
                ("y", C.c_uint32), ("possible_crtcs", C.c_uint32), ("gamma_size", C.c_uint32)]

class ObjectProps(C.Structure):
    _fields_ = [("count_props", C.c_uint32), ("props", C.POINTER(C.c_uint32)),
                ("prop_values", C.POINTER(C.c_uint64))]

class Property(C.Structure):
    _fields_ = [("prop_id", C.c_uint32), ("flags", C.c_uint32), ("name", C.c_char * 32),
                ("count_values", C.c_int), ("values", C.POINTER(C.c_uint64)),
                ("count_enums", C.c_int), ("enums", C.c_void_p),
                ("count_blobs", C.c_int), ("blob_ids", C.POINTER(C.c_uint32))]

class Blob(C.Structure):
    _fields_ = [("id", C.c_uint32), ("length", C.c_uint32), ("data", C.c_void_p)]

class Version(C.Structure):
    _fields_ = [("major", C.c_int), ("minor", C.c_int), ("patch", C.c_int),
                ("name_len", C.c_int), ("name", C.c_char_p), ("date_len", C.c_int),
                ("date", C.c_char_p), ("desc_len", C.c_int), ("desc", C.c_char_p)]

for name, res in [("drmModeGetResources", Res), ("drmModeGetConnectorCurrent", Connector),
                  ("drmModeGetCrtc", Crtc), ("drmModeGetPlaneResources", PlaneRes),
                  ("drmModeGetPlane", Plane), ("drmModeObjectGetProperties", ObjectProps),
                  ("drmModeGetProperty", Property), ("drmModeGetPropertyBlob", Blob),
                  ("drmGetVersion", Version)]:
    getattr(drm, name).restype = C.POINTER(res)

OBJECT_PLANE = 0xEEEEEEEE
CONNECTOR_TYPES = {0: "Unknown", 1: "VGA", 2: "DVI-I", 3: "DVI-D", 4: "DVI-A", 5: "Composite",
                   6: "SVIDEO", 7: "LVDS", 8: "Component", 9: "DIN", 10: "DP", 11: "HDMI-A",
                   12: "HDMI-B", 13: "TV", 14: "eDP", 15: "Virtual", 16: "DSI", 17: "DPI",
                   18: "Writeback", 19: "SPI", 20: "USB"}
CONNECTION = {1: "connected", 2: "disconnected", 3: "unknown"}
PLANE_TYPES = {0: "overlay", 1: "primary", 2: "cursor"}
CAPS = {"DUMB_BUFFER": 0x1, "PRIME": 0x5, "TIMESTAMP_MONOTONIC": 0x6, "ASYNC_PAGE_FLIP": 0x7,
        "CURSOR_WIDTH": 0x8, "CURSOR_HEIGHT": 0x9, "ADDFB2_MODIFIERS": 0x10,
        "CRTC_IN_VBLANK_EVENT": 0x12, "SYNCOBJ": 0x13, "SYNCOBJ_TIMELINE": 0x14,
        "ATOMIC_ASYNC_PAGE_FLIP": 0x15}

def fourcc(code):
    return struct.pack("<I", code).decode("ascii", "replace")

def props_of(fd, obj, kind):
    """Property name -> (value, property) of a DRM object."""
    out = {}
    p = drm.drmModeObjectGetProperties(fd, obj, kind)
    if not p:
        return out
    for i in range(p.contents.count_props):
        prop = drm.drmModeGetProperty(fd, p.contents.props[i])
        if prop:
            out[prop.contents.name.decode()] = (p.contents.prop_values[i], prop)
    return out

def in_formats(fd, blob_id):
    """Formats and modifier count from an IN_FORMATS blob."""
    blob = drm.drmModeGetPropertyBlob(fd, blob_id)
    if not blob:
        return None
    data = C.string_at(blob.contents.data, blob.contents.length)
    _, _, n_formats, f_off, n_mods, m_off = struct.unpack_from("<6I", data)
    formats = [struct.unpack_from("<I", data, f_off + 4 * i)[0] for i in range(n_formats)]
    mods = sorted({struct.unpack_from("<QIIQ", data, m_off + 24 * i)[3] for i in range(n_mods)})
    return formats, mods

def mode_str(m):
    preferred = " preferred" if m.type & 0x8 else ""
    interlaced = " interlaced" if m.flags & 0x10 else ""
    exact = m.clock * 1000 / max(m.htotal * m.vtotal, 1)
    return (f"{m.hdisplay}x{m.vdisplay}@{m.vrefresh}Hz ({exact:.3f} Hz, {m.clock} kHz)"
            f"{interlaced}{preferred}")

def probe(path):
    try:
        fd = os.open(path, os.O_RDWR | os.O_CLOEXEC)
    except PermissionError:
        print(f"{path}: permission denied; stopping (would need a group change or sudo)")
        return
    try:
        if drm.drmIsMaster(fd):
            drm.drmDropMaster(fd)
            print(f"{path}: the kernel made this probe DRM master (no other master);"
                  " dropped it and stopped without any query")
            return
        version = drm.drmGetVersion(fd).contents
        print(f"== {path}: driver {version.name.decode()} {version.major}.{version.minor}."
              f"{version.patch} ({version.desc.decode()}); DRM master: no")
        caps = {}
        for name, cap in CAPS.items():
            value = C.c_uint64()
            caps[name] = value.value if drm.drmGetCap(fd, cap, C.byref(value)) == 0 else None
        print("caps:", ", ".join(f"{k}={v}" for k, v in caps.items()))
        universal = drm.drmSetClientCap(fd, 2, 1) == 0
        atomic = drm.drmSetClientCap(fd, 3, 1) == 0
        print(f"client caps: universal planes {'yes' if universal else 'no'},"
              f" atomic {'yes' if atomic else 'no'}")
        res = drm.drmModeGetResources(fd)
        if not res:
            print("no KMS resources (not a display device)")
            return
        r = res.contents
        print(f"framebuffer size limits: {r.min_width}x{r.min_height} .. {r.max_width}x{r.max_height}")
        for i in range(r.count_connectors):
            conn = drm.drmModeGetConnectorCurrent(fd, r.connectors[i]).contents
            kind = CONNECTOR_TYPES.get(conn.connector_type, str(conn.connector_type))
            print(f"connector {conn.connector_id} {kind}-{conn.connector_type_id}:"
                  f" {CONNECTION.get(conn.connection, conn.connection)},"
                  f" {conn.mm_width}x{conn.mm_height} mm, {conn.count_modes} modes,"
                  f" encoder {conn.encoder_id}")
            modes = [conn.modes[j] for j in range(conn.count_modes)]
            modes.sort(key=lambda m: (not m.type & 0x8, -m.hdisplay * m.vdisplay, -m.vrefresh))
            shown = modes if ALL_MODES else modes[:8]
            for m in shown:
                print("   ", mode_str(m))
            if len(modes) > len(shown):
                print(f"    ... {len(modes) - 8} more (--all-modes lists them)")
        for i in range(r.count_crtcs):
            crtc = drm.drmModeGetCrtc(fd, r.crtcs[i]).contents
            mode = mode_str(crtc.mode) if crtc.mode_valid else "off"
            print(f"crtc {crtc.crtc_id} (index {i}): {mode}")
        planes = drm.drmModeGetPlaneResources(fd)
        if planes:
            pr = planes.contents
            for i in range(pr.count_planes):
                plane = drm.drmModeGetPlane(fd, pr.planes[i]).contents
                props = props_of(fd, plane.plane_id, OBJECT_PLANE)
                kind = PLANE_TYPES.get(props.get("type", (None,))[0], "?")
                crtcs = [j for j in range(r.count_crtcs) if plane.possible_crtcs >> j & 1]
                formats = [fourcc(plane.formats[j]) for j in range(plane.count_formats)]
                line = (f"plane {plane.plane_id} {kind}: crtcs {crtcs},"
                        f" {len(formats)} formats: {' '.join(formats)}")
                if "IN_FORMATS" in props:
                    parsed = in_formats(fd, props["IN_FORMATS"][0])
                    if parsed:
                        line += f"; IN_FORMATS modifiers: {' '.join(hex(m) for m in parsed[1])}"
                print(line)
    finally:
        os.close(fd)

ALL_MODES = "--all-modes" in sys.argv[1:]
paths = [a for a in sys.argv[1:] if a != "--all-modes"] or sorted(glob.glob("/dev/dri/card*"))
print("render nodes:", " ".join(sorted(glob.glob("/dev/dri/renderD*"))) or "none")
for path in paths:
    probe(path)
