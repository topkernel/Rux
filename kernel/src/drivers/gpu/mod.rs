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
    if let Some(ref device) = *GPU_DEVICE.lock() {
        device.flush();
        true
    } else {
        false
    }
}
