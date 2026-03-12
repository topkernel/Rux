//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Simplified framebuffer driver (for QEMU RISC-V virt platform)
//!
//! Directly uses QEMU's framebuffer MMIO region
//!
//! QEMU RISC-V virt platform default framebuffer configuration:
//! - Address: 0x10000000 (but usually configured via device tree)
//! - Size: 1024x768 (default)
//! - Format: xRGB 32bpp

use crate::println;
use super::framebuffer::{FrameBuffer, FrameBufferInfo};

/// QEMU RISC-V virt platform default framebuffer address
const FB_DEFAULT_ADDR: u64 = 0x10000000;

/// Default framebuffer dimensions
const FB_DEFAULT_WIDTH: u32 = 1024;
const FB_DEFAULT_HEIGHT: u32 = 768;

/// Simplified Framebuffer information
pub struct SimpleFrameBufferInfo {
    /// Framebuffer physical address
    pub addr: u64,
    /// Framebuffer size (bytes)
    pub size: u32,
    /// Width (pixels)
    pub width: u32,
    /// Height (pixels)
    pub height: u32,
    /// Bytes per row
    pub stride: u32,
}

/// Probe and initialize simplified framebuffer
pub fn probe_simple_framebuffer() -> Option<SimpleFrameBufferInfo> {
    // Use default configuration for now
    // TODO: Read actual configuration from device tree
    let fb_addr = FB_DEFAULT_ADDR;
    let fb_width = FB_DEFAULT_WIDTH;
    let fb_height = FB_DEFAULT_HEIGHT;
    let fb_stride = fb_width * 4; // 32bpp
    let fb_size = fb_stride * fb_height;

    Some(SimpleFrameBufferInfo {
        addr: fb_addr,
        size: fb_size,
        width: fb_width,
        height: fb_height,
        stride: fb_stride,
    })
}

/// Create a simplified framebuffer
pub fn create_framebuffer(info: &SimpleFrameBufferInfo) -> Option<FrameBuffer> {
    unsafe {
        // Map physical address to virtual address (assume identity mapping)
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
