//! GPU 驱动模块
//!
//! 提供图形显示支持
//!
//! 当前实现：
//! - VirtIO-GPU 驱动 (符合 VirtIO 1.2 规范)
//! - 简化 MMIO framebuffer (QEMU RISC-V virt)

pub mod framebuffer;
pub mod fb_simple;
pub mod virtio_cmd;
pub mod virtio_gpu;

pub use framebuffer::{FrameBuffer, FrameBufferInfo};
pub use fb_simple::{probe_simple_framebuffer, create_framebuffer, SimpleFrameBufferInfo};
pub use virtio_gpu::{VirtioGpuDevice, probe_virtio_gpu};
