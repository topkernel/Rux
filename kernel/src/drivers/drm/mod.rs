//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Minimal DRM/KMS character devices (/dev/dri/card0 + /dev/dri/renderD128)
//!
//! Implements just enough of the Linux DRM ioctl surface for the
//! OpenHarmony display HDI stack:
//!
//! - composer (drivers/peripheral/display/composer): drmSetClientCap
//!   (UNIVERSAL_PLANES + ATOMIC), drmSetMaster/drmIsMaster,
//!   drmModeGetResources/Crtc/Encoder/Connector/Plane(+Properties),
//!   drmModeCreatePropertyBlob, drmModeAddFB2/RmFB, drmPrimeFDToHandle,
//!   GEM_CLOSE, drmModeAtomicCommit and drmWaitVBlank.
//! - buffer allocator (drivers/peripheral/display/buffer on renderD128):
//!   drmDropMaster, DRM_IOCTL_MODE_CREATE_DUMB / DESTROY_DUMB,
//!   drmPrimeHandleToFD, plus mmap() and DMA_BUF_IOCTL_SYNC on the
//!   exported prime fd.
//!
//! Scanout is backed by the virtio-gpu 2D device: every GEM object that
//! an atomic commit puts on the primary plane gets its own virtio
//! RESOURCE_CREATE_2D + ATTACH_BACKING; a flip is TRANSFER_TO_HOST_2D +
//! (SET_SCANOUT when the resource changes) + RESOURCE_FLUSH.
//!
//! The DRM object model is fixed and tiny (one CRTC/encoder/connector/
//! primary plane), mirroring how QEMU virtio-gpu presents itself. The
//! plane exposes the rockchip-style "NAME"/"PLANE_MASK" enum/bitmask
//! properties the OH composer matches planes with.

use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::sync::spinlock::Spinlock;

// ============================================================================
// Device numbers / nodes
// ============================================================================

/// Linux DRM primary-node major.
const DRM_MAJOR: u32 = 226;
/// /dev/dri/card0 (primary node, opened by the composer).
pub const DEV_DRM_CARD0: crate::fs::dev_t::DevNo = crate::fs::dev_t::DevNo::new(DRM_MAJOR, 0);
/// /dev/dri/renderD128 (render node, opened by the gbm gralloc).
pub const DEV_DRM_RENDER: crate::fs::dev_t::DevNo = crate::fs::dev_t::DevNo::new(DRM_MAJOR, 128);

// ============================================================================
// KMS object IDs (single fixed pipeline)
// ============================================================================

const OBJ_CRTC: u32 = 40;
const OBJ_ENCODER: u32 = 41;
const OBJ_CONNECTOR: u32 = 42;
const OBJ_PLANE: u32 = 43;

/// Property IDs (the composer looks properties up by NAME through
/// OBJ_GETPROPERTIES + GETPROPERTY; the ids themselves are arbitrary).
const PROP_CRTC_ACTIVE: u32 = 1;
const PROP_CRTC_MODE_ID: u32 = 2;
const PROP_CRTC_OUTFENCE: u32 = 3;
const PROP_CRTC_PLANE_MASK: u32 = 4;
const PROP_CONN_DPMS: u32 = 5;
const PROP_CONN_CRTC_ID: u32 = 6;
const PROP_CONN_BRIGHTNESS: u32 = 7;
const PROP_PLANE_TYPE: u32 = 8;
const PROP_PLANE_FB_ID: u32 = 9;
const PROP_PLANE_CRTC_ID: u32 = 10;
const PROP_PLANE_CRTC_X: u32 = 11;
const PROP_PLANE_CRTC_Y: u32 = 12;
const PROP_PLANE_CRTC_W: u32 = 13;
const PROP_PLANE_CRTC_H: u32 = 14;
const PROP_PLANE_SRC_X: u32 = 15;
const PROP_PLANE_SRC_Y: u32 = 16;
const PROP_PLANE_SRC_W: u32 = 17;
const PROP_PLANE_SRC_H: u32 = 18;
const PROP_PLANE_ZPOS: u32 = 19;
const PROP_PLANE_IN_FENCE_FD: u32 = 20;
const PROP_PLANE_NAME: u32 = 21;

/// DRM_MODE_PROP_* flag bits (uapi).
const DRM_MODE_PROP_RANGE: u32 = 1 << 1;
const DRM_MODE_PROP_ENUM: u32 = 1 << 3;
const DRM_MODE_PROP_BLOB: u32 = 1 << 4;
const DRM_MODE_PROP_BITMASK: u32 = 1 << 5;
const DRM_MODE_PROP_SIGNED_RANGE: u32 = 2 << 6;
const DRM_MODE_PROP_ATOMIC: u32 = 0x8000_0000;
const DRM_MODE_PROP_IMMUTABLE: u32 = 1 << 2;

/// One (name, flags, enums, value, values[]) description of a property.
#[derive(Clone, Copy)]
struct PropDesc {
    prop_id: u32,
    name: &'static str,
    flags: u32,
    /// (enum-name, enum-value) pairs for ENUM/BITMASK props.
    enums: &'static [(&'static str, u64)],
    /// Current value of the property on its owning object.
    value: u64,
    /// For RANGE props: [min, max]; for ENUM props: the enum values
    /// (count == enums.len()).
    values: &'static [u64],
}

const U64_MAX: u64 = u64::MAX;
const I64_MIN: u64 = i64::MIN as u64;

/// Property set for one object: (prop, type-tag) is implicit per list.
fn crtc_props() -> [PropDesc; 4] {
    [
        PropDesc {
            prop_id: PROP_CRTC_ACTIVE,
            name: "ACTIVE",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 1,
            values: &[0, 1],
        },
        PropDesc {
            prop_id: PROP_CRTC_MODE_ID,
            name: "MODE_ID",
            flags: DRM_MODE_PROP_BLOB | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_CRTC_OUTFENCE,
            name: "OUT_FENCE_PTR",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_CRTC_PLANE_MASK,
            name: "PLANE_MASK",
            flags: DRM_MODE_PROP_BITMASK | DRM_MODE_PROP_ATOMIC,
            // Bit positions mirror the rockchip plane-mask the composer
            // parses (DrmCrtc::Init): enum "Smart0" at bit 12 sets
            // mPlaneMask |= SMART0_MASK, which then matches a plane
            // whose NAME enum is "Smart0-win0".
            enums: &[
                ("Cluster0", 0),
                ("Cluster1", 2),
                ("Esmart0", 4),
                ("Esmart1", 8),
                ("Smart0", 12),
                ("Smart1", 16),
            ],
            value: (1 << 0) | (1 << 2) | (1 << 4) | (1 << 8) | (1 << 12) | (1 << 16),
            values: &[],
        },
    ]
}

fn connector_props() -> [PropDesc; 3] {
    [
        PropDesc {
            prop_id: PROP_CONN_DPMS,
            name: "DPMS",
            flags: DRM_MODE_PROP_ENUM | DRM_MODE_PROP_ATOMIC,
            enums: &[("On", 0), ("Standby", 1), ("Suspend", 2), ("Off", 3)],
            value: 0,
            values: &[0, 1, 2, 3],
        },
        PropDesc {
            prop_id: PROP_CONN_CRTC_ID,
            name: "CRTC_ID",
            flags: DRM_MODE_PROP_ATOMIC, // OBJECT type carries no flag bits
            enums: &[],
            value: OBJ_CRTC as u64,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_CONN_BRIGHTNESS,
            name: "brightness",
            flags: DRM_MODE_PROP_RANGE,
            enums: &[],
            value: 255,
            values: &[0, 255],
        },
    ]
}

fn plane_props() -> [PropDesc; 13] {
    [
        PropDesc {
            prop_id: PROP_PLANE_TYPE,
            name: "type",
            flags: DRM_MODE_PROP_ENUM | DRM_MODE_PROP_IMMUTABLE,
            enums: &[("Overlay", 0), ("Primary", 1), ("Cursor", 2)],
            value: 1, // DRM_PLANE_TYPE_PRIMARY
            values: &[0, 1, 2],
        },
        PropDesc {
            prop_id: PROP_PLANE_FB_ID,
            name: "FB_ID",
            flags: DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_ID,
            name: "CRTC_ID",
            flags: DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_X,
            name: "CRTC_X",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_Y,
            name: "CRTC_Y",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_W,
            name: "CRTC_W",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_H,
            name: "CRTC_H",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_X,
            name: "SRC_X",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_Y,
            name: "SRC_Y",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_W,
            name: "SRC_W",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_H,
            name: "SRC_H",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_ZPOS,
            name: "zpos",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, 255],
        },
        PropDesc {
            prop_id: PROP_PLANE_IN_FENCE_FD,
            name: "IN_FENCE_FD",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: u64::MAX, // -1
            values: &[I64_MIN, 0x7fff_ffff],
        },
    ]
}

/// The plane "NAME" enum property (rockchip plane-name style; queried
/// separately because it rides on plane_props' tail).
fn plane_name_prop() -> PropDesc {
    PropDesc {
        prop_id: PROP_PLANE_NAME,
        name: "NAME",
        flags: DRM_MODE_PROP_ENUM,
        // "Smart0-win0" (value 12) is the win-type the composer's
        // planeTypeNames table maps to DRM_PLANE_TYPE_SMART0_WIN0, which
        // intersects the CRTC PLANE_MASK (Smart0 -> 0xf000) nonzero.
        enums: &[
            ("Cluster0-win0", 0),
            ("Cluster0-win1", 1),
            ("Cluster1-win0", 2),
            ("Cluster1-win1", 3),
            ("Esmart0-win0", 4),
            ("Esmart0-win1", 5),
            ("Esmart0-win2", 6),
            ("Esmart0-win3", 7),
            ("Esmart1-win0", 8),
            ("Esmart1-win1", 9),
            ("Esmart1-win2", 10),
            ("Esmart1-win3", 11),
            ("Smart0-win0", 12),
            ("Smart0-win1", 13),
            ("Smart0-win2", 14),
            ("Smart0-win3", 15),
            ("Smart1-win0", 16),
            ("Smart1-win1", 17),
            ("Smart1-win2", 18),
            ("Smart1-win3", 19),
        ],
        value: 12, // Smart0-win0
        values: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19],
    }
}

// ============================================================================
// Fixed mode (matches the virtio-gpu scanout QEMU was started with)
// ============================================================================

/// The single display mode exposed on the connector. Struct layout is
/// uapi struct drm_mode_modeinfo (72 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DrmModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub type_: u32,
    pub name: [u8; 32],
}

const DRM_MODE_TYPE_PREFERRED: u32 = 1 << 3;
const DRM_MODE_TYPE_DRIVER: u32 = 1 << 6;
const DRM_MODE_FLAG_NHSYNC: u32 = 1 << 1;
const DRM_MODE_FLAG_NVSYNC: u32 = 1 << 2;
/// DRM_MODE_CONNECTOR_DSI.
const DRM_MODE_CONNECTOR_DSI: u32 = 16;
/// DRM_MODE_ENCODER_DSI.
const DRM_MODE_ENCODER_DSI: u32 = 6;
/// DRM_MODE_CONNECTED.
const DRM_MODE_CONNECTED: u32 = 1;

fn fixed_mode_name(w: u32, h: u32) -> [u8; 32] {
    let mut name = [0u8; 32];
    let label = alloc::format!("{}x{}", w, h);
    let n = label.len().min(31);
    name[..n].copy_from_slice(&label.as_bytes()[..n]);
    name
}

fn current_mode() -> DrmModeInfo {
    let (w, h) = match crate::drivers::gpu::get_framebuffer_info() {
        Some(i) => (i.width, i.height),
        None => (720, 1280),
    };
    // 60 Hz CVT-ish timing: htotal = w + 80, vtotal = h + 80.
    let htotal = w + 80;
    let vtotal = h + 80;
    let clock = (htotal as u64) * (vtotal as u64) * 60 / 1000;
    DrmModeInfo {
        name: fixed_mode_name(w, h),
        clock: clock as u32,
        hdisplay: w as u16,
        hsync_start: (w + 20) as u16,
        hsync_end: (w + 60) as u16,
        htotal: htotal as u16,
        hskew: 0,
        vdisplay: h as u16,
        vsync_start: (h + 20) as u16,
        vsync_end: (h + 50) as u16,
        vtotal: vtotal as u16,
        vscan: 0,
        vrefresh: 60,
        flags: DRM_MODE_FLAG_NHSYNC | DRM_MODE_FLAG_NVSYNC,
        type_: DRM_MODE_TYPE_PREFERRED | DRM_MODE_TYPE_DRIVER,
    }
}

// ============================================================================
// ioctl numbers (Linux uapi encodings, x86_64/riscv64 LP64 sizes)
// ============================================================================

const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | (0x64 << 8) | nr
}

const DRM_IOCTL_VERSION: u32 = ioc(3, 0x00, 64);
const DRM_IOCTL_SET_VERSION: u32 = ioc(3, 0x07, 16);
const DRM_IOCTL_GET_CAP: u32 = ioc(3, 0x0c, 16);
const DRM_IOCTL_SET_CLIENT_CAP: u32 = ioc(1, 0x0d, 16);
const DRM_IOCTL_SET_MASTER: u32 = ioc(0, 0x1e, 0);
const DRM_IOCTL_DROP_MASTER: u32 = ioc(0, 0x1f, 0);
const DRM_IOCTL_AUTH_MAGIC: u32 = ioc(1, 0x11, 4);
const DRM_IOCTL_GEM_CLOSE: u32 = ioc(1, 0x09, 8);
const DRM_IOCTL_PRIME_HANDLE_TO_FD: u32 = ioc(3, 0x2d, 16);
const DRM_IOCTL_PRIME_FD_TO_HANDLE: u32 = ioc(3, 0x2e, 16);
const DRM_IOCTL_WAIT_VBLANK: u32 = ioc(3, 0x3a, 24);
const DRM_IOCTL_MODE_GETRESOURCES: u32 = ioc(3, 0xa0, 64);
const DRM_IOCTL_MODE_GETCRTC: u32 = ioc(3, 0xa1, 112);
const DRM_IOCTL_MODE_SETCRTC: u32 = ioc(3, 0xa2, 112);
const DRM_IOCTL_MODE_GETENCODER: u32 = ioc(3, 0xa6, 24);
const DRM_IOCTL_MODE_GETCONNECTOR: u32 = ioc(3, 0xa7, 80);
const DRM_IOCTL_MODE_GETPROPERTY: u32 = ioc(3, 0xaa, 64);
const DRM_IOCTL_MODE_SETPROPERTY: u32 = ioc(3, 0xab, 16);
const DRM_IOCTL_MODE_GETPROPBLOB: u32 = ioc(3, 0xac, 16);
const DRM_IOCTL_MODE_ADDFB: u32 = ioc(3, 0xae, 24);
const DRM_IOCTL_MODE_RMFB: u32 = ioc(3, 0xaf, 4);
const DRM_IOCTL_MODE_PAGE_FLIP: u32 = ioc(3, 0xb0, 24);
const DRM_IOCTL_MODE_CREATE_DUMB: u32 = ioc(3, 0xb2, 32);
const DRM_IOCTL_MODE_MAP_DUMB: u32 = ioc(3, 0xb3, 16);
const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = ioc(3, 0xb4, 4);
const DRM_IOCTL_MODE_GETPLANERESOURCES: u32 = ioc(3, 0xb5, 16);
const DRM_IOCTL_MODE_GETPLANE: u32 = ioc(3, 0xb6, 32);
const DRM_IOCTL_MODE_OBJ_GETPROPERTIES: u32 = ioc(3, 0xb9, 32);
const DRM_IOCTL_MODE_ADDFB2: u32 = ioc(3, 0xb8, 104);
/// NOTE: the OH image ships a libdrm that still uses the PRE-4.0
/// drm_mode_atomic layout (objs_ptr/count_props_ptr/props_ptr/
/// prop_values_ptr/user_data, 56 bytes) — not the mainline count_props
/// layout. Both /dev nodes and this driver must speak that layout.
const DRM_IOCTL_MODE_ATOMIC: u32 = ioc(3, 0xbc, 56);
const DRM_IOCTL_MODE_CREATEPROPBLOB: u32 = ioc(3, 0xbd, 16);
const DRM_IOCTL_MODE_DESTROYPROPBLOB: u32 = ioc(3, 0xbe, 4);

/// DRM_CAP_* ids queried through DRM_IOCTL_GET_CAP.
const DRM_CAP_DUMB_BUFFER: u64 = 0x1;
const DRM_CAP_VBLANK_HIGH_CRTC: u64 = 0x2;
const DRM_CAP_PRIME: u64 = 0x5;
const DRM_CAP_CURSOR_WIDTH: u64 = 0x8;
const DRM_CAP_CURSOR_HEIGHT: u64 = 0x9;
/// DRM_PRIME_CAP_IMPORT | DRM_PRIME_CAP_EXPORT.
const DRM_PRIME_CAPS: u64 = 0x3;

/// DRM_CLIENT_CAP_UNIVERSAL_PLANES / _ATOMIC.
const DRM_CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
const DRM_CLIENT_CAP_ATOMIC: u64 = 3;

/// DMA_BUF_IOCTL_SYNC (_IOW('b', 0, u64), issued on prime fds).
const DMA_BUF_IOCTL_SYNC: u32 = 0x4008_6200;

// ============================================================================
// GEM objects, framebuffers, blobs
// ============================================================================

/// A dumb-buffer backing store. Kernel-heap allocated (physically
/// contiguous — same discipline as the virtio-gpu framebuffer), mapped
/// to userspace by the mmap path and scanned out through its own virtio
/// 2D resource once flipped.
pub struct GemObject {
    pub id: u32,
    /// Kernel virtual address of the backing (linear map).
    pub kva: *mut u8,
    /// Physical address (what virtio ATTACH_BACKING wants).
    pub phys: u64,
    pub size: usize,
    layout: Layout,
    /// virtio-gpu 2D resource bound to this object (0 = none). Set on
    /// first scanout.
    virtio_res: AtomicU32,

}

// SAFETY: the object's mutable state is a single AtomicU32; kva/phys are
// stable for the object's lifetime.
unsafe impl Send for GemObject {}
unsafe impl Sync for GemObject {}

impl Drop for GemObject {
    fn drop(&mut self) {
        let res = self.virtio_res.load(Ordering::Acquire);
        if res != 0 {
            // Best effort: release the host-side resource. Never runs
            // under the DRM state lock (see drm_state discipline).
            let _ = crate::drivers::gpu::with_gpu_device(|gpu| gpu.unref_resource(res));
        }
        // SAFETY: kva was allocated with exactly this layout in
        // create_dumb and never reallocated.
        unsafe { dealloc(self.kva, self.layout) };
    }
}

struct FbObject {
    _gem: Arc<GemObject>,
    width: u32,
    height: u32,
    /// virtio_gpu format id the GEM bytes are laid out as.
    virtio_format: u32,
}

struct DrmState {
    gems: BTreeMap<u32, Arc<GemObject>>,
    fbs: BTreeMap<u32, FbObject>,
    blobs: BTreeMap<u32, Vec<u8>>,
    /// virtio resource currently bound to scanout 0 (0 = the boot fbdev
    /// resource owned by the fbdev driver).
    scanout_res: u32,
}

static DRM_STATE: Spinlock<DrmState> = Spinlock::new(DrmState {
    gems: BTreeMap::new(),
    fbs: BTreeMap::new(),
    blobs: BTreeMap::new(),
    scanout_res: 0,
});

static NEXT_GEM_ID: AtomicU32 = AtomicU32::new(1000);
static NEXT_FB_ID: AtomicU32 = AtomicU32::new(100);
static NEXT_BLOB_ID: AtomicU32 = AtomicU32::new(200);
static NEXT_VIRTIO_RES: AtomicU32 = AtomicU32::new(2); // 1 = fbdev fb

/// DRM fourcc -> virtio-gpu 2D format. Both describe the same memory
/// byte order; see the format table in virtio_cmd.rs.
fn virtio_format_for(fourcc: u32) -> Option<u32> {
    match fourcc {
        0x3432_5258 => Some(crate::drivers::gpu::virtio_cmd::format::B8G8R8X8_UNORM), // XRGB8888
        0x3432_5241 => Some(crate::drivers::gpu::virtio_cmd::format::B8G8R8X8_UNORM), // ARGB8888
        0x3432_4258 => Some(crate::drivers::gpu::virtio_cmd::format::R8G8B8X8_UNORM), // XBGR8888
        0x3432_4241 => Some(crate::drivers::gpu::virtio_cmd::format::R8G8B8A8_UNORM), // ABGR8888
        0x3432_5842 => Some(crate::drivers::gpu::virtio_cmd::format::X8R8G8B8_UNORM), // BGRX8888
        0x3432_4742 => Some(crate::drivers::gpu::virtio_cmd::format::A8R8G8B8_UNORM), // BGRA8888
        _ => None,
    }
}

// ============================================================================
// File operations (both DRM nodes share one ops table)
// ============================================================================

fn drmdev_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    0 // EOF (no readable event stream yet)
}

fn drmdev_write(_file: &crate::fs::file::File, buf: &[u8]) -> isize {
    buf.len() as isize
}

/// File ops for /dev/dri/card0 and /dev/dri/renderD128.
pub static DRM_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(drmdev_read),
    write: Some(drmdev_write),
    lseek: None,
    close: None,
    poll: None,
    read_at: None,
};

/// True when the File is one of the /dev/dri nodes.
pub fn is_drm_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &DRM_OPS as *const _),
        None => false,
    }
}

// --- prime (dma-buf style) file ------------------------------------------------

fn prime_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    -25 // ENOTTY: dma-buf is mmap/DMABUF_IOCTL_SYNC only
}

fn prime_write(_file: &crate::fs::file::File, _buf: &[u8]) -> isize {
    -25
}

/// Close: drop the Arc<GemObject> the export stashed in private_data.
fn prime_close(file: &crate::fs::file::File) -> i32 {
    // SAFETY: private_data was installed by prime_handle_to_fd as
    // Box<Arc<GemObject>>; reconstruct and drop it once here.
    unsafe {
        let cell = file.private_data.get();
        if let Some(ptr) = *cell {
            *cell = None;
            drop(alloc::boxed::Box::from_raw(ptr as *mut Arc<GemObject>));
        }
    }
    0
}

pub static PRIME_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(prime_read),
    write: Some(prime_write),
    lseek: None,
    close: Some(prime_close),
    poll: None,
    read_at: None,
};

/// True when the File is an exported DRM prime fd.
pub fn is_prime_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &PRIME_OPS as *const _),
        None => false,
    }
}

/// GEM object backing a prime fd.
// SAFETY: private_data holds a Box<Arc<GemObject>> installed at export.
fn prime_file_gem(file: &crate::fs::file::File) -> Option<Arc<GemObject>> {
    unsafe {
        let cell = file.private_data.get();
        match *cell {
            Some(ptr) => Some((*(ptr as *const Arc<GemObject>)).clone()),
            None => None,
        }
    }
}

// ============================================================================
// uaccess helpers
// ============================================================================

/// Copy `size` bytes from user `arg`, zero-padded/extended into a fixed
/// stack buffer of N bytes. Returns None on EFAULT.
fn copy_in<const N: usize>(arg: usize, size: usize) -> Option<[u8; N]> {
    let n = size.min(N);
    let mut buf = [0u8; N];
    if n == 0 {
        return Some(buf);
    }
    if !crate::arch::uaccess::access_ok(arg, n) {
        return None;
    }
    // SAFETY: buf is a local; arg validated by access_ok.
    if unsafe {
        crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), arg as *const u8, n)
    } != 0
    {
        return None;
    }
    Some(buf)
}

/// Copy `size` bytes of a fixed buffer out to user `arg`.
fn copy_out<const N: usize>(arg: usize, buf: &[u8; N], size: usize) -> bool {
    let n = size.min(N);
    if n == 0 {
        return true;
    }
    if !crate::arch::uaccess::access_ok(arg, n) {
        return false;
    }
    // SAFETY: buf is initialized for n bytes; arg validated.
    unsafe { crate::arch::uaccess::copy_to_user(arg as *mut u8, buf.as_ptr(), n) == 0 }
}

fn rdu32(b: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes(b[off..off + 4].try_into().unwrap())
}
fn wru32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}
fn rdu64(b: &[u8], off: usize) -> u64 {
    u64::from_ne_bytes(b[off..off + 8].try_into().unwrap())
}
fn wru64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_ne_bytes());
}

// ============================================================================
// DRM ioctl implementation
// ============================================================================

/// Handle a DRM ioctl on a /dev/dri node.
pub fn drm_ioctl(request: u32, arg: usize) -> i64 {
    let ret = drm_ioctl_inner(request, arg);
    trace_ioctl(request, ret);
    ret
}

// ============================================================================
// Console tracing (bring-up): first use of each ioctl plus throttled
// counters, so the composer's DRM handshake is visible on serial (hilog
// never reaches the console on this image).
// ============================================================================

static TRACE_FIRST_DONE: AtomicU32 = AtomicU32::new(0);
static TRACE_COUNTS: [AtomicU32; 8] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
const T_RES: usize = 0; // GETRESOURCES-family (crtc/encoder/connector/plane/props)
const T_DUMB: usize = 1; // CREATE_DUMB
const T_EXPORT: usize = 2; // PRIME_HANDLE_TO_FD
const T_IMPORT: usize = 3; // PRIME_FD_TO_HANDLE
const T_FB: usize = 4; // ADDFB2
const T_ATOMIC: usize = 5; // ATOMIC
const T_VBLANK: usize = 6; // WAIT_VBLANK
const T_UNHANDLED: usize = 7; // ENOTTY answers

fn trace_ioctl(request: u32, ret: i64) {
    let (slot, name) = match request {
        DRM_IOCTL_VERSION => return,
        DRM_IOCTL_MODE_GETRESOURCES
        | DRM_IOCTL_MODE_GETCRTC
        | DRM_IOCTL_MODE_GETENCODER
        | DRM_IOCTL_MODE_GETCONNECTOR
        | DRM_IOCTL_MODE_GETPROPERTY
        | DRM_IOCTL_MODE_OBJ_GETPROPERTIES
        | DRM_IOCTL_MODE_GETPLANERESOURCES
        | DRM_IOCTL_MODE_GETPLANE
        | DRM_IOCTL_MODE_CREATEPROPBLOB => (T_RES, "enum"),
        DRM_IOCTL_MODE_CREATE_DUMB => (T_DUMB, "create_dumb"),
        DRM_IOCTL_PRIME_HANDLE_TO_FD => (T_EXPORT, "prime_export"),
        DRM_IOCTL_PRIME_FD_TO_HANDLE => (T_IMPORT, "prime_import"),
        DRM_IOCTL_MODE_ADDFB2 | DRM_IOCTL_MODE_ADDFB => (T_FB, "addfb"),
        DRM_IOCTL_MODE_ATOMIC | DRM_IOCTL_MODE_PAGE_FLIP => (T_ATOMIC, "atomic"),
        DRM_IOCTL_WAIT_VBLANK => (T_VBLANK, "wait_vblank"),
        DRM_IOCTL_MODE_SETCRTC | DRM_IOCTL_SET_CLIENT_CAP => return,
        _ if ret == -25 => (T_UNHANDLED, "ENOTTY"),
        _ => return,
    };
    let n = TRACE_COUNTS[slot].fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        crate::pr_info!("drm: first {} (ret {})", name, ret);
    } else if slot == T_ATOMIC && n % 100 == 0 {
        crate::pr_info!("drm: atomic commits so far: {}", n + 1);
    } else if slot == T_DUMB && n % 50 == 0 {
        crate::pr_info!("drm: dumb buffers so far: {}", n + 1);
    }
    if ret < 0 && ret != -25 && n < 8 {
        crate::pr_warn!("drm: {} failed ret={}", name, ret);
    }
}

fn drm_ioctl_inner(request: u32, arg: usize) -> i64 {
    match request {
        DRM_IOCTL_VERSION => drm_version(arg),
        DRM_IOCTL_SET_VERSION => 0, // accept whatever; nothing reads it back
        DRM_IOCTL_GET_CAP => drm_get_cap(arg),
        DRM_IOCTL_SET_CLIENT_CAP => drm_set_client_cap(arg),
        DRM_IOCTL_SET_MASTER | DRM_IOCTL_DROP_MASTER => 0, // single-client system
        DRM_IOCTL_AUTH_MAGIC => {
            // drmIsMaster() probes with AUTH_MAGIC(0): master fds get
            // EINVAL (invalid magic) in Linux, non-master get EACCES.
            -22 // EINVAL
        }
        DRM_IOCTL_GEM_CLOSE => gem_close(rdu32_or(arg)),
        DRM_IOCTL_PRIME_HANDLE_TO_FD => prime_handle_to_fd(arg),
        DRM_IOCTL_PRIME_FD_TO_HANDLE => prime_fd_to_handle(arg),
        DRM_IOCTL_WAIT_VBLANK => wait_vblank(arg),
        DRM_IOCTL_MODE_GETRESOURCES => get_resources(arg),
        DRM_IOCTL_MODE_GETCRTC => get_crtc(arg),
        DRM_IOCTL_MODE_SETCRTC => set_crtc(arg),
        DRM_IOCTL_MODE_GETENCODER => get_encoder(arg),
        DRM_IOCTL_MODE_GETCONNECTOR => get_connector(arg),
        DRM_IOCTL_MODE_GETPROPERTY => get_property(arg),
        DRM_IOCTL_MODE_OBJ_GETPROPERTIES => obj_get_properties(arg),
        DRM_IOCTL_MODE_SETPROPERTY => 0, // DPMS et al: accept
        DRM_IOCTL_MODE_GETPROPBLOB => get_prop_blob(arg),
        DRM_IOCTL_MODE_ADDFB => addfb(arg),
        DRM_IOCTL_MODE_ADDFB2 => addfb2(arg),
        DRM_IOCTL_MODE_RMFB => rmfb(rdu32_or(arg)),
        DRM_IOCTL_MODE_PAGE_FLIP => page_flip(arg),
        DRM_IOCTL_MODE_CREATE_DUMB => create_dumb(arg),
        DRM_IOCTL_MODE_MAP_DUMB => map_dumb(arg),
        DRM_IOCTL_MODE_DESTROY_DUMB => gem_close(rdu32_or(arg)),
        DRM_IOCTL_MODE_GETPLANERESOURCES => get_plane_resources(arg),
        DRM_IOCTL_MODE_GETPLANE => get_plane(arg),
        DRM_IOCTL_MODE_ATOMIC => mode_atomic(arg),
        DRM_IOCTL_MODE_CREATEPROPBLOB => create_prop_blob(arg),
        DRM_IOCTL_MODE_DESTROYPROPBLOB => destroy_prop_blob(rdu32_or(arg)),
        _ => -25, // ENOTTY
    }
}

/// Read a lone u32 ioctl argument (RMFB, GEM_CLOSE, DESTROY_DUMB...).
fn rdu32_or(arg: usize) -> u32 {
    match crate::arch::uaccess::access_ok(arg, 4).then(|| unsafe {
        crate::arch::uaccess::get_user(arg as *const u32)
    }) {
        Some(Some(v)) => v,
        _ => 0,
    }
}

fn drm_version(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<64>(arg, 64) else { return -14 };
    let name = b"rux-drm\0";
    let date = b"20261014\0";
    let desc = b"rux minimal DRM/KMS over virtio-gpu\0";

    let name_len = rdu64(&b, 16) as usize;
    let name_ptr = rdu64(&b, 24) as usize;
    let date_len = rdu64(&b, 32) as usize;
    let date_ptr = rdu64(&b, 40) as usize;
    let desc_len = rdu64(&b, 48) as usize;
    let desc_ptr = rdu64(&b, 56) as usize;

    wru32(&mut b, 0, 1); // major
    wru32(&mut b, 4, 4); // minor
    wru32(&mut b, 8, 0); // patch

    // copy strings (truncated to the caller's buffers)
    if name_ptr != 0 && name_len > 0 {
        let n = name.len().min(name_len);
        if !crate::arch::uaccess::access_ok(name_ptr, n) {
            return -14;
        }
        // SAFETY: name_ptr validated.
        if unsafe { crate::arch::uaccess::copy_to_user(name_ptr as *mut u8, name.as_ptr(), n) } != 0 {
            return -14;
        }
    }
    if date_ptr != 0 && date_len > 0 {
        let n = date.len().min(date_len);
        if !crate::arch::uaccess::access_ok(date_ptr, n) {
            return -14;
        }
        if unsafe { crate::arch::uaccess::copy_to_user(date_ptr as *mut u8, date.as_ptr(), n) } != 0 {
            return -14;
        }
    }
    if desc_ptr != 0 && desc_len > 0 {
        let n = desc.len().min(desc_len);
        if !crate::arch::uaccess::access_ok(desc_ptr, n) {
            return -14;
        }
        if unsafe { crate::arch::uaccess::copy_to_user(desc_ptr as *mut u8, desc.as_ptr(), n) } != 0 {
            return -14;
        }
    }
    wru64(&mut b, 16, name.len() as u64);
    wru64(&mut b, 32, date.len() as u64);
    wru64(&mut b, 48, desc.len() as u64);

    if !copy_out(arg, &b, 64) {
        return -14;
    }
    0
}

fn drm_get_cap(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let cap = rdu64(&b, 0);
    let val = match cap {
        DRM_CAP_DUMB_BUFFER | DRM_CAP_VBLANK_HIGH_CRTC => 1,
        DRM_CAP_PRIME => DRM_PRIME_CAPS,
        DRM_CAP_CURSOR_WIDTH | DRM_CAP_CURSOR_HEIGHT => 64,
        _ => return -22, // EINVAL: unknown capability
    };
    wru64(&mut b, 8, val);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

fn drm_set_client_cap(arg: usize) -> i64 {
    let Some(b) = copy_in::<16>(arg, 16) else { return -14 };
    let cap = rdu64(&b, 0);
    match cap {
        DRM_CLIENT_CAP_UNIVERSAL_PLANES | DRM_CLIENT_CAP_ATOMIC => 0,
        _ => -22, // EINVAL (Linux returns EOPNOTSUPP for unknown caps)
    }
}

// --- resources ------------------------------------------------------------

/// GETRESOURCES: one CRTC, one encoder, one connector (no fbs listed).
fn get_resources(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<64>(arg, 64) else { return -14 };
    let crtcs: [u32; 1] = [OBJ_CRTC];
    let connectors: [u32; 1] = [OBJ_CONNECTOR];
    let encoders: [u32; 1] = [OBJ_ENCODER];

    let crtc_ptr = rdu64(&b, 8) as usize;
    let conn_ptr = rdu64(&b, 16) as usize;
    let enc_ptr = rdu64(&b, 24) as usize;
    let n_crtc = rdu32(&b, 36);
    let n_conn = rdu32(&b, 40);
    let n_enc = rdu32(&b, 44);

    if n_crtc > 0 && crtc_ptr != 0 {
        let n = (n_crtc as usize).min(crtcs.len());
        if !crate::arch::uaccess::access_ok(crtc_ptr, n * 4) {
            return -14;
        }
        // SAFETY: ptr validated.
        if unsafe {
            crate::arch::uaccess::copy_to_user(crtc_ptr as *mut u8, crtcs.as_ptr() as *const u8, n * 4)
        } != 0
        {
            return -14;
        }
    }
    if n_conn > 0 && conn_ptr != 0 {
        let n = (n_conn as usize).min(connectors.len());
        if !crate::arch::uaccess::access_ok(conn_ptr, n * 4) {
            return -14;
        }
        if unsafe {
            crate::arch::uaccess::copy_to_user(conn_ptr as *mut u8, connectors.as_ptr() as *const u8, n * 4)
        } != 0
        {
            return -14;
        }
    }
    if n_enc > 0 && enc_ptr != 0 {
        let n = (n_enc as usize).min(encoders.len());
        if !crate::arch::uaccess::access_ok(enc_ptr, n * 4) {
            return -14;
        }
        if unsafe {
            crate::arch::uaccess::copy_to_user(enc_ptr as *mut u8, encoders.as_ptr() as *const u8, n * 4)
        } != 0
        {
            return -14;
        }
    }

    wru32(&mut b, 32, 0); // count_fbs
    wru32(&mut b, 36, crtcs.len() as u32);
    wru32(&mut b, 40, connectors.len() as u32);
    wru32(&mut b, 44, encoders.len() as u32);
    wru32(&mut b, 48, 0); // min_width
    wru32(&mut b, 52, 8192); // max_width
    wru32(&mut b, 56, 0); // min_height
    wru32(&mut b, 60, 8192); // max_height
    if !copy_out(arg, &b, 64) {
        return -14;
    }
    0
}

/// GETCRTC: report the fixed mode, no fb bound initially.
fn get_crtc(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<112>(arg, 112) else { return -14 };
    let mode = current_mode();
    wru32(&mut b, 12, OBJ_CRTC); // crtc_id
    wru32(&mut b, 16, 0); // fb_id
    wru32(&mut b, 28, 0); // gamma_size
    wru32(&mut b, 32, 1); // mode_valid
    // SAFETY: mode is a plain struct; the destination range is inside b.
    unsafe {
        core::ptr::copy_nonoverlapping(
            &mode as *const DrmModeInfo as *const u8,
            b.as_mut_ptr().add(36),
            core::mem::size_of::<DrmModeInfo>(),
        );
    }
    if !copy_out(arg, &b, 112) {
        return -14;
    }
    0
}

/// SETCRTC: legacy modeset — accept the fixed mode, reject others.
fn set_crtc(arg: usize) -> i64 {
    let Some(b) = copy_in::<112>(arg, 112) else { return -14 };
    let mode_valid = rdu32(&b, 32);
    if mode_valid == 0 {
        return 0; // dpms-off style disable: accept
    }
    let req_h = u16::from_ne_bytes(b[36 + 4..36 + 6].try_into().unwrap()) as u32;
    let req_v = u16::from_ne_bytes(b[36 + 10..36 + 12].try_into().unwrap()) as u32;
    let cur = current_mode();
    if req_h != cur.hdisplay as u32 || req_v != cur.vdisplay as u32 {
        return -22; // EINVAL: only the native mode is supported
    }
    0
}

fn get_encoder(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<24>(arg, 24) else { return -14 };
    wru32(&mut b, 0, OBJ_ENCODER);
    wru32(&mut b, 4, DRM_MODE_ENCODER_DSI);
    wru32(&mut b, 8, 0); // crtc_id (attached later by atomic commit)
    wru32(&mut b, 12, 1); // possible_crtcs: CRTC index 0
    wru32(&mut b, 16, 1); // possible_clones
    if !copy_out(arg, &b, 24) {
        return -14;
    }
    0
}

/// GETCONNECTOR: connected DSI panel with the fixed mode and the
/// possible encoder.
fn get_connector(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<80>(arg, 80) else { return -14 };
    let enc_ptr = rdu64(&b, 0) as usize;
    let modes_ptr = rdu64(&b, 8) as usize;
    let n_enc = rdu32(&b, 40);
    let n_modes = rdu32(&b, 32);

    let encoders: [u32; 1] = [OBJ_ENCODER];
    let mode = current_mode();

    if n_enc > 0 && enc_ptr != 0 {
        let n = (n_enc as usize).min(encoders.len());
        if !crate::arch::uaccess::access_ok(enc_ptr, n * 4) {
            return -14;
        }
        // SAFETY: ptr validated.
        if unsafe {
            crate::arch::uaccess::copy_to_user(enc_ptr as *mut u8, encoders.as_ptr() as *const u8, n * 4)
        } != 0
        {
            return -14;
        }
    }
    if n_modes > 0 && modes_ptr != 0 {
        let n = (n_modes as usize).min(1);
        if !crate::arch::uaccess::access_ok(modes_ptr, n * 72) {
            return -14;
        }
        if unsafe {
            crate::arch::uaccess::copy_to_user(
                modes_ptr as *mut u8,
                &mode as *const DrmModeInfo as *const u8,
                n * 72,
            )
        } != 0
        {
            return -14;
        }
    }

    wru32(&mut b, 32, 1); // count_modes
    wru32(&mut b, 36, 0); // count_props
    wru32(&mut b, 40, 1); // count_encoders
    wru32(&mut b, 44, OBJ_ENCODER); // encoder_id
    wru32(&mut b, 48, OBJ_CONNECTOR); // connector_id
    wru32(&mut b, 52, DRM_MODE_CONNECTOR_DSI);
    wru32(&mut b, 56, 1); // connector_type_id
    wru32(&mut b, 60, DRM_MODE_CONNECTED);
    wru32(&mut b, 64, 72); // mm_width
    wru32(&mut b, 68, 128); // mm_height
    wru32(&mut b, 72, 0); // subpixel
    wru32(&mut b, 76, 0); // pad
    if !copy_out(arg, &b, 80) {
        return -14;
    }
    0
}

fn get_plane_resources(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let ptr = rdu64(&b, 0) as usize;
    let n = rdu32(&b, 8);
    let planes: [u32; 1] = [OBJ_PLANE];
    if n > 0 && ptr != 0 {
        if !crate::arch::uaccess::access_ok(ptr, 4) {
            return -14;
        }
        // SAFETY: ptr validated.
        if unsafe {
            crate::arch::uaccess::copy_to_user(ptr as *mut u8, planes.as_ptr() as *const u8, 4)
        } != 0
        {
            return -14;
        }
    }
    wru32(&mut b, 8, planes.len() as u32);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

/// Formats advertised on the primary plane (32-bit RGB fourccs).
const PLANE_FORMATS: [u32; 4] = [
    0x3432_5241, // ARGB8888
    0x3432_5258, // XRGB8888
    0x3432_4241, // ABGR8888 (RGBA_8888)
    0x3432_4742, // BGRA8888
];

fn get_plane(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<32>(arg, 32) else { return -14 };
    wru32(&mut b, 0, OBJ_PLANE); // plane_id
    wru32(&mut b, 4, 0); // crtc_id
    wru32(&mut b, 8, 0); // fb_id
    wru32(&mut b, 12, 1); // possible_crtcs: bit 0
    wru32(&mut b, 16, 0); // gamma_size
    wru32(&mut b, 20, PLANE_FORMATS.len() as u32);
    let ptr = rdu64(&b, 24) as usize;
    if ptr != 0 {
        if !crate::arch::uaccess::access_ok(ptr, PLANE_FORMATS.len() * 4) {
            return -14;
        }
        // SAFETY: ptr validated.
        if unsafe {
            crate::arch::uaccess::copy_to_user(
                ptr as *mut u8,
                PLANE_FORMATS.as_ptr() as *const u8,
                PLANE_FORMATS.len() * 4,
            )
        } != 0
        {
            return -14;
        }
    }
    if !copy_out(arg, &b, 32) {
        return -14;
    }
    0
}

// --- properties ------------------------------------------------------------

/// Property lists per object.
fn props_for_obj(obj_id: u32) -> Option<Vec<PropDesc>> {
    if obj_id == OBJ_CRTC {
        Some(crtc_props().to_vec())
    } else if obj_id == OBJ_CONNECTOR {
        Some(connector_props().to_vec())
    } else if obj_id == OBJ_PLANE {
        let mut v = plane_props().to_vec();
        v.push(plane_name_prop());
        Some(v)
    } else {
        None
    }
}

/// OBJ_GETPROPERTIES: (prop_id, value) pairs for one object.
fn obj_get_properties(arg: usize) -> i64 {
    const N: usize = 32;
    let Some(b) = copy_in::<N>(arg, N) else { return -14 };
    let props_ptr = rdu64(&b, 0) as usize;
    let values_ptr = rdu64(&b, 8) as usize;
    let count = rdu32(&b, 16) as usize;
    let obj_id = rdu32(&b, 20);

    let Some(props) = props_for_obj(obj_id) else { return -22 };
    let total = props.len();
    let n = count.min(total);
    static OGP_TRACE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    if OGP_TRACE.fetch_add(1, Ordering::Relaxed) < 4 {
        crate::pr_info!(
            "drm: obj_getprops obj={} count={} n={} first_id={} props_ptr={:#x}",
            obj_id,
            count,
            n,
            props.first().map(|p| p.prop_id).unwrap_or(0),
            props_ptr
        );
    }
    if n > 0 && props_ptr != 0 && values_ptr != 0 {
        if !crate::arch::uaccess::access_ok(props_ptr, n * 4)
            || !crate::arch::uaccess::access_ok(values_ptr, n * 8)
        {
            return -14;
        }
        for i in 0..n {
            let pid = props[i].prop_id.to_ne_bytes();
            // SAFETY: pointers validated above.
            if unsafe {
                crate::arch::uaccess::copy_to_user((props_ptr as *mut u8).add(i * 4), pid.as_ptr(), 4)
            } != 0
            {
                return -14;
            }
            let val = props[i].value.to_ne_bytes();
            if unsafe {
                crate::arch::uaccess::copy_to_user((values_ptr as *mut u8).add(i * 8), val.as_ptr(), 8)
            } != 0
            {
                return -14;
            }
        }
    }
    // Update ONLY count_props (and echo obj_id) in the caller's struct —
    // libdrm's two-phase protocol re-issues the ioctl with the same
    // struct; overwriting the pointer fields with zeros (a fresh reply
    // buffer did exactly that) made the second call pass NULL arrays.
    let mut b = b;
    wru32(&mut b, 16, total as u32);
    wru32(&mut b, 20, obj_id);
    if !copy_out(arg, &b, N) {
        return -14;
    }
    0
}

/// GETPROPERTY: full description of one property.
fn get_property(arg: usize) -> i64 {
    const N: usize = 64;
    let Some(mut b) = copy_in::<N>(arg, N) else { return -14 };
    let prop_id = rdu32(&b, 16);
    let Some(props) = props_for_obj_by_prop(prop_id) else { return -22 };

    let values_ptr = rdu64(&b, 0) as usize;
    let enums_ptr = rdu64(&b, 8) as usize;

    let mut name = [0u8; 32];
    let nb = props.name.len().min(31);
    name[..nb].copy_from_slice(&props.name.as_bytes()[..nb]);

    let mut vals: Vec<u64> = Vec::new();
    if props.flags & (DRM_MODE_PROP_ENUM | DRM_MODE_PROP_BITMASK) != 0 {
        for (_, v) in props.enums {
            vals.push(*v);
        }
    } else {
        vals.extend_from_slice(props.values);
    }

    if !vals.is_empty() && values_ptr != 0 {
        if !crate::arch::uaccess::access_ok(values_ptr, vals.len() * 8) {
            return -14;
        }
        for (i, v) in vals.iter().enumerate() {
            // SAFETY: pointer validated above.
            if unsafe {
                crate::arch::uaccess::copy_to_user(
                    (values_ptr as *mut u8).add(i * 8),
                    v.to_ne_bytes().as_ptr(),
                    8,
                )
            } != 0
            {
                return -14;
            }
        }
    }
    if !props.enums.is_empty() && enums_ptr != 0 {
        if !crate::arch::uaccess::access_ok(enums_ptr, props.enums.len() * 40) {
            return -14;
        }
        for (i, (ename, ev)) in props.enums.iter().enumerate() {
            let mut rec = [0u8; 40];
            rec[..8].copy_from_slice(&ev.to_ne_bytes());
            let en = ename.len().min(31);
            rec[8..8 + en].copy_from_slice(&ename.as_bytes()[..en]);
            // SAFETY: pointer validated above.
            if unsafe {
                crate::arch::uaccess::copy_to_user((enums_ptr as *mut u8).add(i * 40), rec.as_ptr(), 40)
            } != 0
            {
                return -14;
            }
        }
    }

    b[24..56].copy_from_slice(&name);
    wru32(&mut b, 56, vals.len() as u32);
    wru32(&mut b, 60, props.enums.len() as u32);
    wru32(&mut b, 20, props.flags);
    if !copy_out(arg, &b, N) {
        return -14;
    }
    0
}

/// Find a property descriptor by its id (search all three lists).
fn props_for_obj_by_prop(prop_id: u32) -> Option<PropDesc> {
    if let Some(p) = crtc_props().iter().find(|p| p.prop_id == prop_id) {
        return Some(clone_prop(p));
    }
    if let Some(p) = connector_props().iter().find(|p| p.prop_id == prop_id) {
        return Some(clone_prop(p));
    }
    if let Some(p) = plane_props().iter().find(|p| p.prop_id == prop_id) {
        return Some(clone_prop(p));
    }
    if plane_name_prop().prop_id == prop_id {
        return Some(plane_name_prop());
    }
    None
}

/// PropDesc carries &'static data — cheap to clone out of the arrays.
fn clone_prop(p: &PropDesc) -> PropDesc {
    PropDesc {
        prop_id: p.prop_id,
        name: p.name,
        flags: p.flags,
        enums: p.enums,
        value: p.value,
        values: p.values,
    }
}

// --- blobs ------------------------------------------------------------

fn create_prop_blob(arg: usize) -> i64 {
    let Some(b) = copy_in::<16>(arg, 16) else { return -14 };
    let data = rdu64(&b, 0) as usize;
    let len = rdu32(&b, 8) as usize;
    if len > 4096 {
        return -22;
    }
    let mut buf = vec![0u8; len];
    if len > 0 {
        if !crate::arch::uaccess::access_ok(data, len) {
            return -14;
        }
        // SAFETY: data validated; buf is len bytes.
        if unsafe { crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), data as *const u8, len) } != 0 {
            return -14;
        }
    }
    let id = NEXT_BLOB_ID.fetch_add(1, Ordering::Relaxed);
    let mut st = DRM_STATE.lock_irqsave();
    st.blobs.insert(id, buf);
    drop(st);

    let mut out = [0u8; 16];
    wru32(&mut out, 12, id);
    if !copy_out(arg, &out, 16) {
        return -14;
    }
    0
}

fn destroy_prop_blob(blob_id: u32) -> i64 {
    let mut st = DRM_STATE.lock_irqsave();
    match st.blobs.remove(&blob_id) {
        Some(_) => 0,
        None => -22,
    }
}

fn get_prop_blob(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let blob_id = rdu32(&b, 0);
    let data_ptr = rdu64(&b, 8) as usize;

    let len = {
        let st = DRM_STATE.lock_irqsave();
        match st.blobs.get(&blob_id) {
            Some(v) => v.len(),
            None => return -22,
        }
    };
    if data_ptr != 0 {
        let st = DRM_STATE.lock_irqsave();
        if let Some(v) = st.blobs.get(&blob_id) {
            if !crate::arch::uaccess::access_ok(data_ptr, v.len()) {
                drop(st);
                return -14;
            }
            // SAFETY: data_ptr validated.
            let ret = unsafe {
                crate::arch::uaccess::copy_to_user(data_ptr as *mut u8, v.as_ptr(), v.len())
            };
            drop(st);
            if ret != 0 {
                return -14;
            }
        }
    }
    wru32(&mut b, 4, len as u32);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

// --- GEM / dumb buffers ------------------------------------------------------------

fn create_dumb(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<32>(arg, 32) else { return -14 };
    let height = rdu32(&b, 0);
    let width = rdu32(&b, 4);
    let bpp = rdu32(&b, 8);
    if width == 0 || height == 0 || (bpp != 32 && bpp != 16 && bpp != 24) {
        return -22;
    }
    let pitch = width * (bpp / 8);
    let size = (pitch as u64 * height as u64) as usize;
    let layout = match Layout::from_size_align(size, 4096) {
        Ok(l) => l,
        Err(_) => return -22,
    };
    // SAFETY: layout has non-zero size; null-checked below.
    let kva = unsafe { alloc_zeroed(layout) };
    if kva.is_null() {
        return -12; // ENOMEM
    }
    let phys = crate::arch::mm::virt_to_phys(crate::arch::mm::VirtAddr::new(kva as u64)).0;
    let id = NEXT_GEM_ID.fetch_add(1, Ordering::Relaxed);
    let gem = Arc::new(GemObject {
        id,
        kva,
        phys,
        size,
        layout,
        virtio_res: AtomicU32::new(0),
    });
    let mut st = DRM_STATE.lock_irqsave();
    st.gems.insert(id, gem);
    drop(st);

    wru32(&mut b, 16, id); // handle
    wru32(&mut b, 20, pitch);
    wru64(&mut b, 24, size as u64);
    if !copy_out(arg, &b, 32) {
        return -14;
    }
    0
}

fn gem_close(handle: u32) -> i64 {
    if handle == 0 {
        return -22;
    }
    let taken = {
        let mut st = DRM_STATE.lock_irqsave();
        st.gems.remove(&handle)
    };
    match taken {
        // Drop OUTSIDE the state lock: GemObject::drop runs virtio
        // RESOURCE_UNREF, which polls the device for milliseconds.
        Some(_) => 0,
        None => -22,
    }
}

fn map_dumb(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let handle = rdu32(&b, 0);
    let exists = DRM_STATE.lock_irqsave().gems.contains_key(&handle);
    if !exists {
        return -22;
    }
    // Fake offset: the mmap path on the DRM fd decodes the GEM id from
    // the low bits of the file offset.
    wru64(&mut b, 8, (handle as u64) << 12);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

// --- prime import/export ------------------------------------------------------------

/// PRIME_HANDLE_TO_FD: install an anonymous dma-buf-style file that
/// carries the GEM object (mmap + DMA_BUF_IOCTL_SYNC + FD_TO_HANDLE).
fn prime_handle_to_fd(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let handle = rdu32(&b, 0);
    let flags = rdu32(&b, 4);
    let gem = {
        let st = DRM_STATE.lock_irqsave();
        st.gems.get(&handle).cloned()
    };
    let Some(gem) = gem else { return -22 };

    let fdt = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => return -9,
    };
    let Some(fd) = fdt.alloc_fd() else { return -24 }; // EMFILE
    let mut file_flags = crate::fs::file::FileFlags::O_RDWR;
    if flags & 0x80000 != 0 {
        // DRM_CLOEXEC
        file_flags |= crate::fs::file::FileFlags::O_CLOEXEC;
    }
    let file = Arc::new(crate::fs::file::File::new(crate::fs::file::FileFlags::new(
        file_flags,
    )));
    file.set_ops(&PRIME_OPS);
    // SAFETY: private_data is a Box<Arc<GemObject>>; prime_close frees it.
    file.set_private_data(alloc::boxed::Box::into_raw(alloc::boxed::Box::new(gem)) as *mut u8);
    if fdt.install_fd(fd, file).is_err() {
        return -24;
    }
    if flags & 0x80000 != 0 {
        fdt.set_fd_cloexec(fd, true);
    }

    wru32(&mut b, 8, fd as u32);
    if !copy_out(arg, &b, 16) {
        // Best effort: the fd is installed; userspace will just fail.
        return 0;
    }
    0
}

/// PRIME_FD_TO_HANDLE: resolve a prime fd back to a GEM handle. The fd
/// may have been exported on the other node (or another process).
fn prime_fd_to_handle(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let prime_fd = rdu32(&b, 8) as i32;
    if prime_fd < 0 {
        return -22;
    }
    let Some(file) = (unsafe { crate::fs::file::get_file_fd(prime_fd as usize) }) else {
        return -9; // EBADF
    };
    if !is_prime_file(&file) {
        return -22; // EINVAL: not a dma-buf
    }
    let Some(gem) = prime_file_gem(&file) else { return -22 };

    // Re-register under the gem's id if the handle side was already
    // GEM_CLOSEd (cross-process import): the object stays alive through
    // the prime fd's Arc.
    let handle = gem.id;
    {
        let mut st = DRM_STATE.lock_irqsave();
        if !st.gems.contains_key(&handle) {
            st.gems.insert(handle, gem);
        }
    }
    wru32(&mut b, 0, handle);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

/// Ioctls on a prime fd (dma-buf file).
pub fn prime_file_ioctl(_file: &crate::fs::file::File, request: u32, _arg: usize) -> Option<i64> {
    match request {
        DMA_BUF_IOCTL_SYNC => Some(0), // cache coherent: nothing to do
        _ => Some(-25),                // ENOTTY
    }
}

// --- framebuffers ------------------------------------------------------------

fn addfb(arg: usize) -> i64 {
    let Some(b) = copy_in::<24>(arg, 24) else { return -14 };
    let width = rdu32(&b, 4);
    let height = rdu32(&b, 8);
    let pitch = rdu32(&b, 12);
    let bpp = rdu32(&b, 16);
    // struct drm_mode_fb_cmd: fb_id, width, height, pitch, bpp, depth, handle
    let handle = rdu32(&b, 24);
    let fourcc = match bpp {
        32 => 0x3432_5241, // ARGB8888
        _ => return -22,
    };
    add_fb_common(arg, 24, handle, width, height, pitch, fourcc)
}

fn addfb2(arg: usize) -> i64 {
    let Some(b) = copy_in::<104>(arg, 104) else { return -14 };
    let width = rdu32(&b, 4);
    let height = rdu32(&b, 8);
    let fourcc = rdu32(&b, 12);
    let handle = rdu32(&b, 20); // handles[0]
    let pitch = rdu32(&b, 36); // pitches[0]
    let offset = rdu32(&b, 52); // offsets[0]
    if offset != 0 {
        return -22; // single-plane only
    }
    add_fb_common(arg, 104, handle, width, height, pitch, fourcc)
}

fn add_fb_common(arg: usize, size: usize, handle: u32, width: u32, height: u32, pitch: u32, fourcc: u32) -> i64 {
    if width == 0 || height == 0 || pitch == 0 {
        return -22;
    }
    let Some(vfmt) = virtio_format_for(fourcc) else {
        return -22; // unsupported pixel format
    };
    let gem = {
        let st = DRM_STATE.lock_irqsave();
        st.gems.get(&handle).cloned()
    };
    let Some(gem) = gem else { return -22 };
    // The GEM buffer must actually cover the framebuffer.
    if pitch as u64 * height as u64 > gem.size as u64 {
        return -22;
    }

    let fb_id = NEXT_FB_ID.fetch_add(1, Ordering::Relaxed);
    let mut st = DRM_STATE.lock_irqsave();
    st.fbs.insert(
        fb_id,
        FbObject {
            _gem: gem,
            width,
            height,
            virtio_format: vfmt,
        },
    );
    drop(st);

    let mut out = [0u8; 104];
    wru32(&mut out, 0, fb_id);
    if !copy_out(arg, &out, size) {
        return -14;
    }
    0
}

fn rmfb(fb_id: u32) -> i64 {
    let mut st = DRM_STATE.lock_irqsave();
    match st.fbs.remove(&fb_id) {
        Some(_) => 0,
        None => -22,
    }
}

// --- page flip ------------------------------------------------------------

/// Display one framebuffer on scanout 0 (the whole atomic model here:
/// single plane, single CRTC, mode fixed).
fn present_fb(fb_id: u32) -> i64 {
    // Snapshot the FB data under the lock, then run the virtio sequence
    // lock-free (device transactions can take milliseconds under TCG).
    let (gem, width, height, vfmt) = {
        let st = DRM_STATE.lock_irqsave();
        match st.fbs.get(&fb_id) {
            Some(fb) => (fb._gem.clone(), fb.width, fb.height, fb.virtio_format),
            None => return -22,
        }
    };

    let ok = crate::drivers::gpu::with_gpu_device(|gpu| {
        // First flip of this buffer: create its host-side resource.
        let mut res = gem.virtio_res.load(Ordering::Acquire);
        if res == 0 {
            let id = NEXT_VIRTIO_RES.fetch_add(1, Ordering::Relaxed);
            if gpu.create_resource_2d_id(id, width, height, vfmt).is_none() {
                return false;
            }
            if gpu.attach_backing_id(id, gem.phys, gem.size as u32).is_none() {
                return false;
            }
            gem.virtio_res.store(id, Ordering::Release);
            res = id;
        }
        // Guest RAM -> host resource storage.
        if !gpu.transfer_to_host_2d_id(res, 0, width, height) {
            return false;
        }
        // Bind to scanout when the resource changed since last flip.
        let cur = DRM_STATE.lock_irqsave().scanout_res;
        if cur != res {
            if !gpu.set_scanout_id(0, res, width, height) {
                return false;
            }
            DRM_STATE.lock_irqsave().scanout_res = res;
        }
        // Present.
        gpu.resource_flush_id(res, width, height)
    });
    match ok {
        Some(true) => {
            static FIRST_FLIP: AtomicU32 = AtomicU32::new(0);
            if FIRST_FLIP.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::pr_info!(
                    "drm: first page flip on screen (fb {} {}x{})",
                    fb_id,
                    width,
                    height
                );
            }
            0
        }
        Some(false) => {
            crate::pr_warn!("drm: flip to fb {} failed (virtio-gpu error)", fb_id);
            -5 // EIO
        }
        None => {
            crate::pr_warn!("drm: flip requested but no virtio-gpu device");
            -19 // ENODEV
        }
    }
}

fn page_flip(arg: usize) -> i64 {
    let Some(b) = copy_in::<24>(arg, 24) else { return -14 };
    let fb_id = rdu32(&b, 4);
    if rdu32(&b, 0) != OBJ_CRTC {
        return -22;
    }
    present_fb(fb_id)
}

/// MODE_ATOMIC: walk the OH-layout request arrays, pick up the primary
/// plane's FB_ID (and CRTC attach), then present.
fn mode_atomic(arg: usize) -> i64 {
    const MAX_OBJS: usize = 8;
    const MAX_PROPS: usize = 96;
    let Some(b) = copy_in::<56>(arg, 56) else { return -14 };
    let flags = rdu32(&b, 0);
    let count_objs = rdu32(&b, 4) as usize;
    let objs_ptr = rdu64(&b, 8) as usize;
    let counts_ptr = rdu64(&b, 16) as usize;
    let props_ptr = rdu64(&b, 24) as usize;
    let values_ptr = rdu64(&b, 32) as usize;

    const DRM_MODE_ATOMIC_TEST_ONLY: u32 = 0x0100;

    if count_objs > MAX_OBJS {
        return -7; // E2BIG
    }
    if count_objs == 0 {
        return -22;
    }
    if !crate::arch::uaccess::access_ok(objs_ptr, count_objs * 4)
        || !crate::arch::uaccess::access_ok(counts_ptr, count_objs * 4)
    {
        return -14;
    }
    let mut objs = [0u32; MAX_OBJS];
    let mut counts = [0u32; MAX_OBJS];
    let mut total = 0usize;
    // SAFETY: pointers validated above.
    unsafe {
        for i in 0..count_objs {
            let mut tmp = [0u8; 4];
            if crate::arch::uaccess::copy_from_user(tmp.as_mut_ptr(), (objs_ptr as *const u8).add(i * 4), 4) != 0 {
                return -14;
            }
            objs[i] = u32::from_ne_bytes(tmp);
            if crate::arch::uaccess::copy_from_user(tmp.as_mut_ptr(), (counts_ptr as *const u8).add(i * 4), 4) != 0 {
                return -14;
            }
            counts[i] = u32::from_ne_bytes(tmp);
            total += counts[i] as usize;
        }
    }
    if total > MAX_PROPS {
        return -7;
    }
    if total > 0
        && (!crate::arch::uaccess::access_ok(props_ptr, total * 4)
            || !crate::arch::uaccess::access_ok(values_ptr, total * 8))
    {
        return -14;
    }
    let mut props = [0u32; MAX_PROPS];
    let mut values = [0u64; MAX_PROPS];
    // SAFETY: pointers validated above.
    unsafe {
        for i in 0..total {
            let mut tmp4 = [0u8; 4];
            if crate::arch::uaccess::copy_from_user(tmp4.as_mut_ptr(), (props_ptr as *const u8).add(i * 4), 4) != 0 {
                return -14;
            }
            props[i] = u32::from_ne_bytes(tmp4);
            let mut tmp8 = [0u8; 8];
            if crate::arch::uaccess::copy_from_user(tmp8.as_mut_ptr(), (values_ptr as *const u8).add(i * 8), 8) != 0 {
                return -14;
            }
            values[i] = u64::from_ne_bytes(tmp8);
        }
    }

    // Walk the flattened arrays tracking the object each property
    // belongs to.
    let mut flip_fb: u32 = 0;
    let mut idx = 0usize;
    for oi in 0..count_objs {
        for _ in 0..counts[oi] as usize {
            if idx >= total {
                break;
            }
            let obj = objs[oi];
            let prop = props[idx];
            let value = values[idx];
            idx += 1;
            if obj == OBJ_PLANE && prop == PROP_PLANE_FB_ID {
                flip_fb = value as u32;
            }
            // ACTIVE / MODE_ID / CRTC_ID / zpos / OUT_FENCE_PTR: mode is
            // fixed, fences unsupported — accepted and ignored.
            let _ = (obj, value);
        }
    }

    if flags & DRM_MODE_ATOMIC_TEST_ONLY != 0 {
        return 0; // validate only
    }

    if flip_fb != 0 {
        return present_fb(flip_fb);
    }
    0
}

// --- vblank ------------------------------------------------------------

/// Nominal vblank period (60 Hz), nanoseconds.
const VBLANK_NS: u64 = 16_666_666;
/// _DRM_VBLANK_RELATIVE.
const DRM_VBLANK_RELATIVE: u32 = 0x1;
/// _DRM_VBLANK_ABSOLUTE.
const DRM_VBLANK_TYPES_MASK: u32 = 0x3;
const DRM_VBLANK_EVENT: u32 = 0x400_0000;
/// _DRM_VBLANK_SECONDARY selects pipe 1 — only pipe 0 exists here, so it
/// is ignored (the composer only sets it for pipe-1 CRTCs).
const DRM_VBLANK_SECONDARY: u32 = 0x2000_0000;
const DRM_VBLANK_HIGH_CRTC_MASK: u32 = 0x3e;

fn vblank_seq_at(ns: u64) -> u64 {
    ns / VBLANK_NS
}

/// Sleep (interruptible) until `deadline_ns` (monotonic since boot).
/// Returns false when a signal interrupted the wait.
fn sleep_until_ns(deadline_ns: u64) -> bool {
    use crate::process::task::TaskState;

    if crate::drivers::timer::monotonic_ns_since_boot() >= deadline_ns {
        return true;
    }
    let current = match crate::sched::current() {
        Some(t) => t as *mut crate::process::task::Task,
        None => return true,
    };
    // SAFETY: current is this task's own pointer (kfbflush discipline).
    let my_pid = unsafe { (*current).pid() };
    const NS_PER_JIFFY: u64 = 1_000_000_000 / (crate::config::KERNEL_HZ as u64);

    loop {
        if crate::drivers::timer::monotonic_ns_since_boot() >= deadline_ns {
            return true;
        }
        if crate::signal::signal_pending() {
            return false;
        }
        let remaining = deadline_ns.saturating_sub(crate::drivers::timer::monotonic_ns_since_boot());
        let target =
            crate::drivers::timer::get_jiffies() + remaining / NS_PER_JIFFY + 1;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        if timer_id != 0 {
            // SAFETY: current is this task's own pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            if crate::drivers::timer::get_jiffies() >= target {
                // SAFETY: current is this task's own pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    crate::sched::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::cpu::restore_irq(true);
                crate::sched::schedule();
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            crate::sched::schedule();
        }
    }
}

/// WAIT_VBLANK: timer-driven 60 Hz vblank model (no real display IRQ in
/// virtio-gpu 2D mode). Relative waits sleep to the next boundary,
/// sequence 0 returns the current counter immediately.
fn wait_vblank(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<24>(arg, 24) else { return -14 };
    let req_type = rdu32(&b, 0);
    let req_seq = rdu32(&b, 4);

    if req_type & DRM_VBLANK_EVENT != 0 {
        return -22; // no drm_event read() path
    }
    // Reject only unknown type/flag bits (SECONDARY/HIGH_CRTC are pipe
    // selectors — irrelevant on the single pipe, tolerated).
    const KNOWN_FLAGS: u32 =
        DRM_VBLANK_EVENT | DRM_VBLANK_SECONDARY | DRM_VBLANK_HIGH_CRTC_MASK | 0x800_0000; /* _DRM_VBLANK_SIGNAL */
    if req_type & !DRM_VBLANK_TYPES_MASK & !KNOWN_FLAGS != 0 {
        return -22;
    }

    let now_ns = crate::drivers::timer::monotonic_ns_since_boot();
    let now_seq = vblank_seq_at(now_ns);
    let target_seq = if req_type & DRM_VBLANK_TYPES_MASK == DRM_VBLANK_RELATIVE {
        now_seq + req_seq as u64
    } else {
        // ABSOLUTE: a past sequence reports the current one.
        (req_seq as u64).max(now_seq)
    };
    if target_seq > now_seq {
        let deadline = target_seq * VBLANK_NS;
        if !sleep_until_ns(deadline) {
            return -4; // EINTR
        }
    }

    let fired_ns = crate::drivers::timer::monotonic_ns_since_boot();
    let fired_seq = vblank_seq_at(fired_ns);
    wru32(&mut b, 0, req_type & DRM_VBLANK_TYPES_MASK);
    wru32(&mut b, 4, fired_seq as u32);
    wru64(&mut b, 8, (fired_ns / 1_000_000_000) as u64); // tval_sec
    wru64(&mut b, 16, ((fired_ns % 1_000_000_000) / 1000) as u64); // tval_usec
    if !copy_out(arg, &b, 24) {
        return -14;
    }
    0
}

// ============================================================================
// mmap resolution (called from the mmap syscall path)
// ============================================================================

/// Resolve an mmap on a DRM file into a (phys, len) pair the syscall
/// path can map. `offset` is 0 for prime fds (their private GEM) or the
/// fake offset from MAP_DUMB for /dev/dri nodes.
pub fn gem_mmap_resolve(
    file: &crate::fs::file::File,
    offset: u64,
    len: usize,
) -> Option<(u64, usize)> {
    if is_prime_file(file) {
        let gem = prime_file_gem(file)?;
        let n = len.min(gem.size);
        return Some((gem.phys + offset, n));
    }
    if is_drm_file(file) {
        let handle = (offset >> 12) as u32;
        let gem = {
            let st = DRM_STATE.lock_irqsave();
            st.gems.get(&handle).cloned()
        }?;
        let off = offset & 0xfff;
        let n = len.min(gem.size.saturating_sub(off as usize));
        return Some((gem.phys + off, n));
    }
    None
}

// ============================================================================
// init
// ============================================================================

/// Register the DRM character devices and create the /dev/dri nodes.
/// Called after the GPU framebuffer came up (the flip path needs the
/// virtio-gpu device).
pub fn init_drm() -> Result<(), ()> {
    crate::fs::devfs::registry::register_char_device(DEV_DRM_CARD0, &DRM_OPS)?;
    crate::fs::devfs::registry::register_char_device(DEV_DRM_RENDER, &DRM_OPS)?;

    crate::fs::devfs::mkdir("dri")?;
    crate::fs::devfs::mknod("dri/card0", DEV_DRM_CARD0, 0o666)?;
    crate::fs::devfs::mknod("dri/renderD128", DEV_DRM_RENDER, 0o666)?;

    // DrmDisplay::SetDisplayPowerStatus opens /dev/graphics/fb0 and
    // FBIOBLANKs it; alias the existing fb0 node (the OH power-state
    // path reports NOT_SUPPORT when it is missing).
    crate::fs::devfs::mkdir("graphics")?;
    crate::fs::devfs::mknod("graphics/fb0", crate::fs::dev_t::DEV_FB0, 0o666)?;

    crate::pr_info!(
        "drm: /dev/dri/card0 + renderD128 registered (crtc={} conn={} enc={} plane={})",
        OBJ_CRTC,
        OBJ_CONNECTOR,
        OBJ_ENCODER,
        OBJ_PLANE
    );
    Ok(())
}
