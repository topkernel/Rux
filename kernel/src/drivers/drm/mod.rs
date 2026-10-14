//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! DRM render node (/dev/dri/renderD128) — GEM/dumb-buffer device for the
//! OpenHarmony buffer allocator (agent oh-gralloc).
//!
//! The OH display chain uses TWO DRM devices:
//! - /dev/dri/card0 — the composer's KMS device (agent oh-display's
//!   territory; the KMS ioctl surface lives in that branch. This module
//!   registers the card0 node so both appear in devfs, but KMS ioctls
//!   answer ENOTTY here).
//! - /dev/dri/renderD128 — THIS module: the allocator's buffer device.
//!
//! The OH gralloc (drivers/peripheral/display/buffer/vdi_base/src/) is
//! NOT Mesa GBM — `hi_gbm.cpp` is a shim that issues raw dumb-buffer
//! ioctls on the render node:
//!
//!   InitGbmDevice:  open("/dev/dri/renderD128", O_RDWR) +
//!                   drmDropMaster() (return ignored)
//!   GbmAllocMem:    DRM_IOCTL_MODE_CREATE_DUMB (bpp 8/16/24/32, width
//!                   aligned to 8, height to 2) → drmPrimeHandleToFD
//!                   (DRM_CLOEXEC|DRM_RDWR) → DRM_IOCTL_MODE_DESTROY_DUMB
//!   GbmMmap:        mmap(NULL, size, RW, MAP_SHARED, prime_fd, 0)
//!   cache sync:     ioctl(prime_fd, DMA_BUF_IOCTL_SYNC)
//!   GbmFreeMem:     munmap + close(prime_fd)
//!
//! ioctl encodings below were verified against the OH image's libdrm
//! fork (third_party/libdrm/include/drm/drm.h — note DRM_IOCTL_VERSION
//! is _IOWR there, unlike mainline's _IOR) and cross-checked with the
//! host LP64 compiler. The load-bearing size: `struct drm_prime_handle`
//! is 12 bytes, not 16.

use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::sync::spinlock::Spinlock;

// ============================================================================
// Device numbers / nodes
// ============================================================================

/// Linux DRM char-device major.
const DRM_MAJOR: u32 = 226;
/// /dev/dri/card0 — primary node (composer KMS; oh-display territory).
pub const DEV_DRM_CARD0: crate::fs::dev_t::DevNo = crate::fs::dev_t::DevNo::new(DRM_MAJOR, 0);
/// /dev/dri/renderD128 — render node (gralloc/GEM; this module).
pub const DEV_DRM_RENDER: crate::fs::dev_t::DevNo = crate::fs::dev_t::DevNo::new(DRM_MAJOR, 128);

// ============================================================================
// ioctl numbers — Linux uapi encodings, x86_64/riscv64 LP64.
// _IOC(dir,size,nr) with type 'd' = 0x64; dir: 0=none 1=write 2=read 3=rw.
// Sizes verified against the OH libdrm fork + host gcc (see header comment).
// ============================================================================

const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | (0x64 << 8) | nr
}

const DRM_IOCTL_VERSION: u32 = ioc(3, 0x00, 64); // 0xC0406400 (OH fork: _IOWR)
const DRM_IOCTL_SETVERSION: u32 = ioc(3, 0x07, 16);
const DRM_IOCTL_GEM_CLOSE: u32 = ioc(1, 0x09, 8);
const DRM_IOCTL_GEM_FLINK: u32 = ioc(3, 0x0a, 16);
const DRM_IOCTL_GEM_OPEN: u32 = ioc(3, 0x0b, 16);
const DRM_IOCTL_GET_CAP: u32 = ioc(3, 0x0c, 16);
const DRM_IOCTL_SET_CLIENT_CAP: u32 = ioc(1, 0x0d, 16);
const DRM_IOCTL_AUTH_MAGIC: u32 = ioc(1, 0x11, 4);
const DRM_IOCTL_SET_MASTER: u32 = ioc(0, 0x1e, 0);
const DRM_IOCTL_DROP_MASTER: u32 = ioc(0, 0x1f, 0);
/// `struct drm_prime_handle` is 12 bytes (handle/flags/fd u32 each).
const DRM_IOCTL_PRIME_HANDLE_TO_FD: u32 = ioc(3, 0x2d, 12);
const DRM_IOCTL_PRIME_FD_TO_HANDLE: u32 = ioc(3, 0x2e, 12);
const DRM_IOCTL_MODE_CREATE_DUMB: u32 = ioc(3, 0xb2, 32);
const DRM_IOCTL_MODE_MAP_DUMB: u32 = ioc(3, 0xb3, 16);
const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = ioc(3, 0xb4, 4);

/// DRM_CAP_* ids queried through DRM_IOCTL_GET_CAP (uapi drm.h).
const DRM_CAP_DUMB_BUFFER: u64 = 0x1;
const DRM_CAP_VBLANK_HIGH_CRTC: u64 = 0x2;
const DRM_CAP_DUMB_PREFERRED_DEPTH: u64 = 0x3;
const DRM_CAP_DUMB_PREFERRED_BPP: u64 = 0x4;
const DRM_CAP_PRIME: u64 = 0x5;
const DRM_CAP_TIMESTAMP_MONOTONIC: u64 = 0x6;
const DRM_CAP_CURSOR_WIDTH: u64 = 0x8;
const DRM_CAP_CURSOR_HEIGHT: u64 = 0x9;
/// DRM_PRIME_CAP_IMPORT | DRM_PRIME_CAP_EXPORT.
const DRM_PRIME_CAPS: u64 = 0x3;

/// DRM_CLIENT_CAP_UNIVERSAL_PLANES / _ATOMIC (accepted, value untracked).
const DRM_CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
const DRM_CLIENT_CAP_ATOMIC: u64 = 3;

/// DMA_BUF_IOCTL_SYNC — _IOW('b', 0, u64), issued on prime fds.
const DMA_BUF_IOCTL_SYNC: u32 = 0x4008_6200;

// ============================================================================
// GEM objects
// ============================================================================

/// A dumb-buffer backing store: kernel-heap allocated (same discipline as
/// the virtio-gpu framebuffer — physically contiguous in the linear map),
/// mapped to userspace through the mmap path, shareable across processes
/// as a prime fd.
pub struct GemObject {
    pub id: u32,
    /// Kernel virtual address of the backing (linear map).
    pub kva: *mut u8,
    /// Physical address (what virtio ATTACH_BACKING wants on the scanout
    /// side, and what the mmap path installs in user PTEs).
    pub phys: u64,
    pub size: usize,
    /// Stride of the last CREATE_DUMB (informational; AddFB2 on card0
    /// receives the pitch from userspace anyway).
    pub pitch: u32,
    layout: Layout,
    /// Global GEM name (flink); 0 = unnamed. Lazily assigned under
    /// GEM_STATE (CAS so a racing flink cannot hand out two names).
    name: AtomicU32,
}

// SAFETY: GemObject's mutable state (`name`) is an AtomicU32 only ever
// CASed under GEM_STATE; kva/phys/layout are immutable for the object's
// lifetime.
unsafe impl Send for GemObject {}
unsafe impl Sync for GemObject {}

impl Drop for GemObject {
    fn drop(&mut self) {
        // SAFETY: kva was allocated with exactly this layout in
        // create_dumb and never reallocated.
        unsafe { dealloc(self.kva, self.layout) };
    }
}

struct GemState {
    /// Open handles → objects. Handle namespaces are per-drm-fd in
    /// Linux; here one global namespace (single-driver system, ids
    /// drawn from a global counter — no collisions).
    gems: BTreeMap<u32, Arc<GemObject>>,
    /// Global names (flink) → objects. Weak: the name must not itself
    /// keep the buffer alive; it expires when the last handle AND prime
    /// fd have closed (Linux GEM-name lifetime).
    names: BTreeMap<u32, alloc::sync::Weak<GemObject>>,
}

static GEM_STATE: Spinlock<GemState> = Spinlock::new(GemState {
    gems: BTreeMap::new(),
    names: BTreeMap::new(),
});

static NEXT_GEM_HANDLE: AtomicU32 = AtomicU32::new(1000);
static NEXT_GEM_NAME: AtomicU32 = AtomicU32::new(1);

/// Largest single dumb buffer we will hand out (matches the kernel heap's
/// ability to serve aligned multi-MB allocations; larger requests fail
/// ENOMEM rather than OOM-panic the heap).
const MAX_DUMB_SIZE: usize = 64 * 1024 * 1024;

// ============================================================================
// File operations
// ============================================================================

fn drm_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    0 // EOF: DRM nodes have no readable event stream (no vblank events)
}

fn drm_write(_file: &crate::fs::file::File, buf: &[u8]) -> isize {
    buf.len() as isize
}

/// File ops for /dev/dri/card0 and /dev/dri/renderD128 (both nodes are the
/// same driver, as in Linux).
pub static DRM_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(drm_read),
    write: Some(drm_write),
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
    -25 // ENOTTY: dma-buf is mmap + DMA_BUF_IOCTL_SYNC only
}

fn prime_write(_file: &crate::fs::file::File, _buf: &[u8]) -> isize {
    -25
}

/// Close a prime fd: drop the Arc<GemObject> the export stashed in
/// private_data. When this is the last reference the backing store is
/// freed (the exporter's DESTROY_DUMB may already have removed the
/// handle — Linux keeps the buffer alive through the dma-buf the same
/// way).
fn prime_close(file: &crate::fs::file::File) -> i32 {
    // SAFETY: private_data was installed by prime_handle_to_fd as a
    // Box<Arc<GemObject>>; reconstruct and drop it exactly once (the
    // close_fd path calls ops.close a single time — pipe discipline).
    unsafe {
        let cell = file.private_data.get();
        if let Some(ptr) = *cell {
            *cell = None;
            drop(Box::from_raw(ptr as *mut Arc<GemObject>));
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

/// Copy `size` bytes from user `arg` into a fixed stack buffer of N bytes
/// (zero-padded). Returns None on EFAULT.
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

/// Read a lone u32 ioctl argument (DESTROY_DUMB and friends).
fn rdu32_arg(arg: usize) -> u32 {
    match crate::arch::uaccess::access_ok(arg, 4).then(|| unsafe {
        crate::arch::uaccess::get_user(arg as *const u32)
    }) {
        Some(Some(v)) => v,
        _ => 0,
    }
}

/// Copy a NUL-terminated tag string to a userspace (ptr,len) ioctl field.
fn str_to_user(ptr: usize, len: u64, s: &[u8]) -> bool {
    let ptr = ptr as usize;
    let len = len as usize;
    if ptr == 0 || len == 0 {
        return true;
    }
    let n = s.len().min(len);
    if !crate::arch::uaccess::access_ok(ptr, n) {
        return false;
    }
    // SAFETY: ptr validated; s is 'static.
    unsafe { crate::arch::uaccess::copy_to_user(ptr as *mut u8, s.as_ptr(), n) == 0 }
}

// ============================================================================
// ioctl implementation
// ============================================================================

/// Handle a DRM ioctl on a /dev/dri node.
pub fn drm_ioctl(request: u32, arg: usize) -> i64 {
    let ret = drm_ioctl_inner(request, arg);
    trace_ioctl(request, ret);
    ret
}

// ============================================================================
// Console tracing (bring-up): hilog never reaches the serial console on
// this image, so the first use of each ioctl plus throttled counters are
// printed straight to the console — this is how "what GBM calls succeed"
// is observed.
// ============================================================================

static TRACE_COUNTS: [AtomicU32; 7] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
const T_DUMB: usize = 0; // CREATE_DUMB
const T_DESTROY: usize = 1; // DESTROY_DUMB / GEM_CLOSE
const T_EXPORT: usize = 2; // PRIME_HANDLE_TO_FD
const T_IMPORT: usize = 3; // PRIME_FD_TO_HANDLE
const T_GEMOPEN: usize = 4; // GEM_OPEN / FLINK
const T_MMAP: usize = 5; // gem_mmap_resolve calls
const T_UNHANDLED: usize = 6; // ENOTTY answers

fn trace_ioctl(request: u32, ret: i64) {
    let (slot, name) = match request {
        DRM_IOCTL_VERSION => (T_UNHANDLED, "version"),
        DRM_IOCTL_SETVERSION
        | DRM_IOCTL_GET_CAP
        | DRM_IOCTL_SET_CLIENT_CAP
        | DRM_IOCTL_SET_MASTER
        | DRM_IOCTL_AUTH_MAGIC => return,
        // The OH gralloc's InitGbmDevice issues exactly open() +
        // DROP_MASTER at HDI load — this trace line is the "renderD128
        // opened and alive" signal (the open itself has no ioctl).
        DRM_IOCTL_DROP_MASTER => {
            static FIRST: AtomicU32 = AtomicU32::new(0);
            if FIRST.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::pr_info!("drm: first drop_master (ret {}) — renderD128 opened", ret);
            }
            return;
        }
        DRM_IOCTL_MODE_CREATE_DUMB => (T_DUMB, "create_dumb"),
        DRM_IOCTL_MODE_DESTROY_DUMB | DRM_IOCTL_GEM_CLOSE => (T_DESTROY, "destroy_dumb/gem_close"),
        DRM_IOCTL_PRIME_HANDLE_TO_FD => (T_EXPORT, "prime_handle_to_fd"),
        DRM_IOCTL_PRIME_FD_TO_HANDLE => (T_IMPORT, "prime_fd_to_handle"),
        DRM_IOCTL_GEM_OPEN | DRM_IOCTL_GEM_FLINK => (T_GEMOPEN, "gem_open/flink"),
        _ if ret == -25 => (T_UNHANDLED, "ENOTTY"),
        _ => return,
    };
    let n = TRACE_COUNTS[slot].fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        crate::pr_info!("drm: first {} (ret {})", name, ret);
    } else if (slot == T_DUMB || slot == T_DESTROY || slot == T_EXPORT) && n % 200 == 0 {
        crate::pr_info!("drm: {} calls so far: {}", name, n + 1);
    }
    if ret < 0 && ret != -25 && n < 8 {
        crate::pr_warn!("drm: {} failed ret={}", name, ret);
    }
}

fn drm_ioctl_inner(request: u32, arg: usize) -> i64 {
    match request {
        DRM_IOCTL_VERSION => drm_version(arg),
        DRM_IOCTL_SETVERSION => 0, // accept whatever; nothing reads it back
        DRM_IOCTL_GET_CAP => drm_get_cap(arg),
        DRM_IOCTL_SET_CLIENT_CAP => drm_set_client_cap(arg),
        // Single-master system: both succeed (the gralloc calls
        // drmDropMaster() on the render node right after open and
        // ignores the return; giving it 0 matches Linux render nodes,
        // where DROP_MASTER on a non-master fd is 0).
        DRM_IOCTL_SET_MASTER | DRM_IOCTL_DROP_MASTER => 0,
        // drmIsMaster() probes with AUTH_MAGIC(0): master fds get EINVAL
        // (invalid magic) in Linux, non-master get EACCES. EINVAL it is.
        DRM_IOCTL_AUTH_MAGIC => -22,
        DRM_IOCTL_GEM_CLOSE => gem_close(rdu32_arg(arg)),
        DRM_IOCTL_GEM_FLINK => gem_flink(arg),
        DRM_IOCTL_GEM_OPEN => gem_open(arg),
        DRM_IOCTL_PRIME_HANDLE_TO_FD => prime_handle_to_fd(arg),
        DRM_IOCTL_PRIME_FD_TO_HANDLE => prime_fd_to_handle(arg),
        DRM_IOCTL_MODE_CREATE_DUMB => create_dumb(arg),
        DRM_IOCTL_MODE_MAP_DUMB => map_dumb(arg),
        // DESTROY_DUMB and GEM_CLOSE have identical semantics here
        // (handle lifetime is refcounted through prime fds).
        DRM_IOCTL_MODE_DESTROY_DUMB => gem_close(rdu32_arg(arg)),
        // KMS ioctls (GETRESOURCES/GETCRTC/planes/atomic/...) are the
        // composer side — card0 territory (feature/oh-display). ENOTTY
        // on this branch.
        _ => -25, // ENOTTY
    }
}

/// DRM_IOCTL_VERSION: name "virtio_gpu" — some userspace (mesa GBM,
/// drmGetProviderName callers) keys off the driver name.
fn drm_version(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<64>(arg, 64) else { return -14 };
    let name = b"virtio_gpu\0";
    let date = b"20261014\0";
    let desc = b"Rux DRM render node (dumb/prime)\0";

    let name_len = rdu64(&b, 16);
    let name_ptr = rdu64(&b, 24);
    let date_len = rdu64(&b, 32);
    let date_ptr = rdu64(&b, 40);
    let desc_len = rdu64(&b, 48);
    let desc_ptr = rdu64(&b, 56);

    wru32(&mut b, 0, 1); // major
    wru32(&mut b, 4, 1); // minor
    wru32(&mut b, 8, 0); // patch

    if !str_to_user(name_ptr as usize, name_len, name)
        || !str_to_user(date_ptr as usize, date_len, date)
        || !str_to_user(desc_ptr as usize, desc_len, desc)
    {
        return -14;
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
        DRM_CAP_DUMB_BUFFER
        | DRM_CAP_VBLANK_HIGH_CRTC
        | DRM_CAP_TIMESTAMP_MONOTONIC => 1,
        DRM_CAP_DUMB_PREFERRED_DEPTH => 24,
        DRM_CAP_DUMB_PREFERRED_BPP => 32,
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

// --- dumb buffers ------------------------------------------------------------

/// DRM_IOCTL_MODE_CREATE_DUMB. The OH hi_gbm shim aligns width to 8 and
/// height to 2 in userspace and passes bitsPerPixel straight from its
/// format table — 8 for YUV multiplanar, 16/24/32 for RGB.
fn create_dumb(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<32>(arg, 32) else { return -14 };
    let height = rdu32(&b, 0);
    let width = rdu32(&b, 4);
    let bpp = rdu32(&b, 8);
    if width == 0 || height == 0 || (bpp != 8 && bpp != 16 && bpp != 24 && bpp != 32) {
        return -22;
    }
    // 64-byte stride alignment: harmless for CPU access, friendlier to
    // any later scanout import on card0.
    let bytes_per_pixel = bpp as u64 / 8;
    let pitch = (width as u64 * bytes_per_pixel).div_ceil(64) * 64;
    let size = pitch * height as u64;
    if size == 0 || size > MAX_DUMB_SIZE as u64 {
        return -12; // ENOMEM
    }
    let layout = match Layout::from_size_align(size as usize, 4096) {
        Ok(l) => l,
        Err(_) => return -22,
    };
    // SAFETY: layout has non-zero size; null-checked below.
    let kva = unsafe { alloc_zeroed(layout) };
    if kva.is_null() {
        return -12; // ENOMEM
    }
    let phys = crate::arch::mm::virt_to_phys(crate::arch::mm::VirtAddr::new(kva as u64)).0;
    let handle = NEXT_GEM_HANDLE.fetch_add(1, Ordering::Relaxed);
    let gem = Arc::new(GemObject {
        id: handle,
        kva,
        phys,
        size: size as usize,
        pitch: pitch as u32,
        layout,
        name: AtomicU32::new(0),
    });
    GEM_STATE.lock_irqsave().gems.insert(handle, gem);

    wru32(&mut b, 16, handle);
    wru32(&mut b, 20, pitch as u32);
    wru64(&mut b, 24, size);
    if !copy_out(arg, &b, 32) {
        return -14;
    }
    0
}

/// Drop one handle reference (GEM_CLOSE / DESTROY_DUMB). The backing
/// store survives while a prime fd still holds an Arc.
fn gem_close(handle: u32) -> i64 {
    if handle == 0 {
        return -22;
    }
    let taken = GEM_STATE.lock_irqsave().gems.remove(&handle);
    match taken {
        // Dropping the Arc here frees the backing store only when no
        // prime fd holds another reference; the free is plain heap
        // dealloc — no device I/O — safe in this scope.
        Some(_) => 0,
        None => -22,
    }
}

/// DRM_IOCTL_MODE_MAP_DUMB: fake offset whose low 12 bits are 0 and whose
/// high bits carry the handle (decoded by gem_mmap_resolve).
fn map_dumb(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let handle = rdu32(&b, 0);
    if !GEM_STATE.lock_irqsave().gems.contains_key(&handle) {
        return -22;
    }
    wru64(&mut b, 8, (handle as u64) << 12);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

// --- global names (flink/open) --------------------------------------------------

/// DRM_IOCTL_GEM_FLINK: publish the object under a global name.
fn gem_flink(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let handle = rdu32(&b, 0);
    let gem = GEM_STATE.lock_irqsave().gems.get(&handle).cloned();
    let Some(gem) = gem else { return -22 };
    // Lazily assign the name: CAS 0 -> new so a racing flink cannot hand
    // out two names for one object.
    let mut name = gem.name.load(Ordering::Acquire);
    if name == 0 {
        let n = NEXT_GEM_NAME.fetch_add(1, Ordering::Relaxed);
        // Every object draws a fresh id, so the CAS can only fail to
        // another flink of THIS object — then reuse its id.
        match gem.name.compare_exchange(0, n, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => name = n,
            Err(existing) => name = existing,
        }
        let mut st = GEM_STATE.lock_irqsave();
        st.names.insert(name, Arc::downgrade(&gem));
    }
    wru64(&mut b, 8, name as u64);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

/// DRM_IOCTL_GEM_OPEN: get a handle for a global name (cross-process
/// sharing without prime fds). The object may currently have NO open
/// handle (the exporter gem_close'd) — it stays alive through prime
/// fds, which the Weak name registry tracks.
fn gem_open(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let name = rdu32(&b, 0);
    let weak = GEM_STATE.lock_irqsave().names.get(&name).cloned();
    let Some(weak) = weak else { return -22 };
    let Some(gem) = weak.upgrade() else {
        // Object fully closed: forget the name (Linux invalidates it).
        GEM_STATE.lock_irqsave().names.remove(&name);
        return -22;
    };
    let handle = gem.id;
    let size = gem.size as u64;
    // Re-insert under the original id (handles and names alias the same
    // object).
    GEM_STATE.lock_irqsave().gems.insert(handle, gem);
    wru32(&mut b, 4, handle);
    wru64(&mut b, 8, size);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

// --- prime import/export ----------------------------------------------------

/// DRM_IOCTL_PRIME_HANDLE_TO_FD: install an anonymous dma-buf-style file
/// carrying the GEM object (mmap + DMA_BUF_IOCTL_SYNC + FD_TO_HANDLE).
fn prime_handle_to_fd(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<12>(arg, 12) else { return -14 };
    let handle = rdu32(&b, 0);
    let flags = rdu32(&b, 4);
    let gem = GEM_STATE.lock_irqsave().gems.get(&handle).cloned();
    let Some(gem) = gem else { return -22 };

    let Some(fdt) = crate::sched::get_current_fdtable() else { return -9 };
    let Some(fd) = fdt.alloc_fd() else { return -24 }; // EMFILE
    let file = Arc::new(crate::fs::file::File::new(
        crate::fs::file::FileFlags::new(crate::fs::file::FileFlags::O_RDWR),
    ));
    file.set_ops(&PRIME_OPS);
    // SAFETY: private_data is a Box<Arc<GemObject>>; prime_close frees it.
    file.set_private_data(Box::into_raw(Box::new(gem)) as *mut u8);
    if fdt.install_fd(fd, file).is_err() {
        return -24;
    }
    // DRM_CLOEXEC (== O_CLOEXEC) in the prime flags maps to the fd flag;
    // DRM_RDWR is the fd's access mode, already O_RDWR.
    if flags & 0x80000 != 0 {
        fdt.set_fd_cloexec(fd, true);
    }

    wru32(&mut b, 8, fd as u32);
    if !copy_out(arg, &b, 12) {
        // Best effort: the fd is installed; userspace just fails.
        return 0;
    }
    0
}

/// DRM_IOCTL_PRIME_FD_TO_HANDLE: resolve a prime fd back to a GEM handle.
/// The fd may have been exported on the other node or another process
/// (allocator_host exports, composer_host imports).
fn prime_fd_to_handle(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<12>(arg, 12) else { return -14 };
    let prime_fd = rdu32(&b, 8) as i32;
    if prime_fd < 0 {
        return -22;
    }
    // SAFETY: get_file_fd returns a valid Arc<File> or None.
    let Some(file) = (unsafe { crate::fs::file::get_file_fd(prime_fd as usize) }) else {
        return -9; // EBADF
    };
    if !is_prime_file(&file) {
        return -22; // EINVAL: not a dma-buf
    }
    let Some(gem) = prime_file_gem(&file) else { return -22 };

    let handle = gem.id;
    let mut st = GEM_STATE.lock_irqsave();
    if !st.gems.contains_key(&handle) {
        st.gems.insert(handle, gem);
    }
    drop(st);
    wru32(&mut b, 0, handle);
    if !copy_out(arg, &b, 12) {
        return -14;
    }
    0
}

/// Ioctls on a prime fd (dma-buf file).
pub fn prime_file_ioctl(_file: &crate::fs::file::File, request: u32, _arg: usize) -> Option<i64> {
    match request {
        // Cache-coherent unified memory: sync is a no-op. The gralloc
        // issues DMA_BUF_IOCTL_SYNC up to 7 times per cache op (its
        // retry loop) — all must return 0.
        DMA_BUF_IOCTL_SYNC => Some(0),
        _ => Some(-25), // ENOTTY
    }
}

// ============================================================================
// mmap resolution (called from the mmap syscall path)
// ============================================================================

/// Resolve an mmap against a DRM file into a (phys, len) pair the syscall
/// path can map. `offset` is 0 for prime fds (their private GEM — the
/// gralloc's mmap of the exported dma-buf) or the fake offset from
/// MAP_DUMB for /dev/dri nodes.
pub fn gem_mmap_resolve(
    file: &crate::fs::file::File,
    offset: u64,
    len: usize,
) -> Option<(u64, usize)> {
    let trace_first = TRACE_COUNTS[T_MMAP].fetch_add(1, Ordering::Relaxed) == 0;
    if is_prime_file(file) {
        let gem = prime_file_gem(file)?;
        let n = len.min(gem.size);
        let r = (gem.phys + offset, n);
        if trace_first {
            crate::pr_info!("drm: first prime-fd mmap (phys {:#x}, {} bytes)", r.0, r.1);
        }
        return Some(r);
    }
    if is_drm_file(file) {
        let handle = (offset >> 12) as u32;
        let gem = GEM_STATE.lock_irqsave().gems.get(&handle).cloned()?;
        let off = (offset & 0xfff) as usize;
        if off >= gem.size {
            return None;
        }
        let n = len.min(gem.size - off);
        let r = (gem.phys + off as u64, n);
        if trace_first {
            crate::pr_info!("drm: first node mmap (gem {}, phys {:#x}, {} bytes)", handle, r.0, r.1);
        }
        return Some(r);
    }
    None
}

// ============================================================================
// init
// ============================================================================

/// Register the DRM character devices and create the /dev/dri nodes.
/// No virtio-gpu dependency: the allocator path is pure memory
/// management (the console-mode OH boot has no GPU device at all).
pub fn init_drm() -> Result<(), ()> {
    crate::fs::devfs::registry::register_char_device(DEV_DRM_CARD0, &DRM_OPS)?;
    crate::fs::devfs::registry::register_char_device(DEV_DRM_RENDER, &DRM_OPS)?;

    crate::fs::devfs::mkdir("dri")?;
    crate::fs::devfs::mknod("dri/card0", DEV_DRM_CARD0, 0o666)?;
    crate::fs::devfs::mknod("dri/renderD128", DEV_DRM_RENDER, 0o666)?;

    crate::pr_info!("drm: /dev/dri/card0 + renderD128 registered (render node: dumb/GEM/prime)");
    Ok(())
}
