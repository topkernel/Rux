//! GPU 驱动模块
//!
//! 提供简化的 framebuffer 支持
//!
//! 当前实现：
//! - 简化 MMIO framebuffer (QEMU RISC-V virt)
//!
//! 计划实现：
//! - VirtIO-GPU 驱动 (符合 VirtIO 1.0 规范)

pub mod framebuffer;
pub mod fb_simple;

pub use framebuffer::{FrameBuffer, FrameBufferInfo};
pub use fb_simple::{probe_simple_framebuffer, create_framebuffer, SimpleFrameBufferInfo};
