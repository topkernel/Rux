//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! GPU driver module
//!
//! Provides graphics display support
//!
//! Current implementation:
//! - VirtIO-GPU driver (compliant with VirtIO 1.2 specification)
//! - Simplified MMIO framebuffer (QEMU RISC-V virt)

pub mod framebuffer;
pub mod fb_simple;
pub mod fbdev;
pub mod virtio_cmd;
pub mod virtio_gpu;

pub use framebuffer::{FrameBuffer, FrameBufferInfo};
pub use fb_simple::{probe_simple_framebuffer, create_framebuffer, SimpleFrameBufferInfo};
pub use virtio_gpu::{VirtioGpuDevice, probe_virtio_gpu};
pub use fbdev::{
    fbdev_ioctl, create_fix_screeninfo, create_var_screeninfo,
    FbFixScreeninfo, FbVarScreeninfo, FbBitfield,
    FBIOGET_FSCREENINFO, FBIOGET_VSCREENINFO, FBIO_FLUSH,
};

use crate::sync::spinlock::Spinlock;

/// Global framebuffer information storage
/// Used for user-space access to framebuffer via mmap
static FRAMEBUFFER_INFO: Spinlock<Option<FrameBufferInfo>> = Spinlock::new(None);

/// Global GPU device storage
/// Used for flushing framebuffer
static GPU_DEVICE: Spinlock<Option<VirtioGpuDevice>> = Spinlock::new(None);

/// Set global framebuffer info (called during GPU initialization)
pub fn set_framebuffer_info(info: FrameBufferInfo) {
    *FRAMEBUFFER_INFO.lock() = Some(info);
}

/// Get global framebuffer info (used during mmap)
pub fn get_framebuffer_info() -> Option<FrameBufferInfo> {
    FRAMEBUFFER_INFO.lock().clone()
}

/// Set global GPU device (called during initialization)
pub fn set_gpu_device(device: VirtioGpuDevice) {
    *GPU_DEVICE.lock() = Some(device);
}

/// Flush framebuffer to display device
/// VirtIO-GPU requires explicit flush to display updated content
pub fn flush_framebuffer() -> bool {
    // Snapshot the device pointer under the lock and RELEASE before the
    // flush. device.flush() submits virtio commands and polls for the
    // device response, which under TCG/BQL contention can take hundreds
    // of ms; holding GPU_DEVICE across that stall made every concurrent
    // flusher (kfbflush every 30ms, each FBIO_FLUSH ioctl) spin on this
    // lock for a whole device transaction — observed as
    // "DEADLOCK: spinlock stuck cpu=1 lock=GPU_DEVICE" in soak-r1
    // (smp4 MTTCG + I/O churn, t=948s).
    // SAFETY: GPU_DEVICE is a static set once by set_gpu_device() during
    // boot and never cleared or replaced afterwards, so the pointed-to
    // VirtioGpuDevice outlives this function; flush() takes &self only.
    let device: *const VirtioGpuDevice = {
        let guard = GPU_DEVICE.lock();
        match guard.as_ref() {
            Some(d) => d as *const VirtioGpuDevice,
            None => return false,
        }
    };
    unsafe { (*device).flush() };
    true
}

/// Run `f` with the global VirtIO-GPU device (None when no GPU was
/// probed). Same lock discipline as flush_framebuffer: the device
/// pointer is snapshotted and the lock released BEFORE `f` runs — virtio
/// transactions inside `f` can take milliseconds under TCG and must not
/// hold GPU_DEVICE.
// SAFETY: GPU_DEVICE is set once during boot and never cleared; the
// pointed-to VirtioGpuDevice outlives the call (see flush_framebuffer).
pub fn with_gpu_device<R>(f: impl FnOnce(&VirtioGpuDevice) -> R) -> Option<R> {
    let device: *const VirtioGpuDevice = {
        let guard = GPU_DEVICE.lock();
        match guard.as_ref() {
            Some(d) => d as *const VirtioGpuDevice,
            None => return None,
        }
    };
    Some(f(unsafe { &*device }))
}
