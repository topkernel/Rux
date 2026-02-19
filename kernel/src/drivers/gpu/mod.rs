//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!


//! GPU 驱动模块
//!
//! 提供图形显示支持
//!
//! 当前实现：
//! - VirtIO-GPU 驱动 (符合 VirtIO 1.2 规范)
//! - 简化 MMIO framebuffer (QEMU RISC-V virt)

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
    FBIOGET_FSCREENINFO, FBIOGET_VSCREENINFO,
};

use spin::Mutex;

/// 全局 Framebuffer 信息存储
/// 用于用户态通过 mmap 访问帧缓冲区
static FRAMEBUFFER_INFO: Mutex<Option<FrameBufferInfo>> = Mutex::new(None);

/// 设置全局 framebuffer 信息（GPU 初始化时调用）
pub fn set_framebuffer_info(info: FrameBufferInfo) {
    *FRAMEBUFFER_INFO.lock() = Some(info);
}

/// 获取全局 framebuffer 信息（mmap 时使用）
pub fn get_framebuffer_info() -> Option<FrameBufferInfo> {
    FRAMEBUFFER_INFO.lock().clone()
}
