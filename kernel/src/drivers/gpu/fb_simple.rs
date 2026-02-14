//! 简化的 framebuffer 驱动（用于 QEMU RISC-V virt 平台）
//!
//! 直接使用 QEMU 的 framebuffer MMIO 区域
//!
//! QEMU RISC-V virt 平台默认 framebuffer 配置：
//! - 地址：0x10000000 (但通常通过设备树配置）
//! - 尺寸：1024x768 (默认)
//! - 格式：xRGB 32bpp

use crate::println;
use super::framebuffer::{FrameBuffer, FrameBufferInfo};

/// QEMU RISC-V virt 平台的默认 framebuffer 地址
const FB_DEFAULT_ADDR: u64 = 0x10000000;

/// 默认 framebuffer 尺寸
const FB_DEFAULT_WIDTH: u32 = 1024;
const FB_DEFAULT_HEIGHT: u32 = 768;

/// 简化的 Framebuffer 信息
pub struct SimpleFrameBufferInfo {
    /// Framebuffer 物理地址
    pub addr: u64,
    /// Framebuffer 大小（字节）
    pub size: u32,
    /// 宽度（像素）
    pub width: u32,
    /// 高度（像素）
    pub height: u32,
    /// 每行字节数
    pub stride: u32,
}

/// 探测并初始化简化的 framebuffer
pub fn probe_simple_framebuffer() -> Option<SimpleFrameBufferInfo> {
    println!("gpu: Probing for simple framebuffer...");

    // 暂时使用默认配置
    // TODO: 从设备树读取实际配置
    let fb_addr = FB_DEFAULT_ADDR;
    let fb_width = FB_DEFAULT_WIDTH;
    let fb_height = FB_DEFAULT_HEIGHT;
    let fb_stride = fb_width * 4; // 32bpp
    let fb_size = fb_stride * fb_height;

    println!("gpu: Found framebuffer: addr={:#x}, size={}x{}, stride={}",
             fb_addr, fb_width, fb_height, fb_stride);

    Some(SimpleFrameBufferInfo {
        addr: fb_addr,
        size: fb_size,
        width: fb_width,
        height: fb_height,
        stride: fb_stride,
    })
}

/// 创建简化的 framebuffer
pub fn create_framebuffer(info: &SimpleFrameBufferInfo) -> Option<FrameBuffer> {
    unsafe {
        // 将物理地址映射为虚拟地址（假设恒等映射）
        let fb = FrameBuffer::new(info.addr, FrameBufferInfo {
            addr: info.addr,
            size: info.size,
            width: info.width,
            height: info.height,
            stride: info.stride,
            format: 1, // xRGB
        });
        Some(fb)
    }
}
