//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Framebuffer character device (/dev/fb0)
//!
//! Implements framebuffer device interface

use super::FrameBufferInfo;

/// ioctl command codes
/// Get variable screen information
pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
/// Get fixed screen information
pub const FBIOGET_FSCREENINFO: u32 = 0x4602;
/// Flush framebuffer (VirtIO-GPU specific)
/// VirtIO-GPU requires explicit flush to display updated content
pub const FBIO_FLUSH: u32 = 0x4610;

/// Framebuffer type
pub const FB_TYPE_PACKED_PIXELS: u32 = 0;

/// Framebuffer visual type
pub const FB_VISUAL_TRUECOLOR: u32 = 2;

/// Color bitfield
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbBitfield {
    /// Offset (from LSB)
    pub offset: u32,
    /// Number of bits
    pub length: u32,
    /// MSB first
    pub msb_right: u32,
}

/// Fixed screen information
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbFixScreeninfo {
    /// Driver name (16 bytes)
    pub id: [u8; 16],
    /// Physical memory start address
    pub smem_start: u64,
    /// Physical memory length
    pub smem_len: u32,
    /// Framebuffer type
    pub type_: u32,
    /// Visual type
    pub visual: u32,
    /// Line length (bytes)
    pub line_length: u32,
    /// MMIO start address
    pub mmio_start: u64,
    /// MMIO length
    pub mmio_len: u32,
    /// Acceleration type
    pub accel: u32,
    /// Performance info flags
    pub capabilities: u16,
    /// Reserved
    pub reserved: [u16; 2],
}

impl Default for FbFixScreeninfo {
    fn default() -> Self {
        Self {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: FB_TYPE_PACKED_PIXELS,
            visual: FB_VISUAL_TRUECOLOR,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        }
    }
}

/// Variable screen information
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbVarScreeninfo {
    /// Visible resolution
    pub xres: u32,
    pub yres: u32,
    /// Virtual resolution
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    /// Offset from virtual to visible
    pub xoffset: u32,
    pub yoffset: u32,
    /// Bits per pixel
    pub bits_per_pixel: u32,
    /// Grayscale levels (0 = color)
    pub grayscale: u32,
    /// Red bitfield
    pub red: FbBitfield,
    /// Green bitfield
    pub green: FbBitfield,
    /// Blue bitfield
    pub blue: FbBitfield,
    /// Transparency bitfield
    pub transp: FbBitfield,
    /// Non-standard mode
    pub nonstd: u32,
    /// Activation flags
    pub activate: u32,
    /// Display height (mm)
    pub height: u32,
    /// Display width (mm)
    pub width: u32,
    /// Timing flags
    pub accel_flags: u32,
    /// Pixel clock (ps)
    pub pixclock: u32,
    /// Timing parameters
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    /// Sync flags
    pub sync: u32,
    /// Video mode
    pub vmode: u32,
    /// Rotation angle
    pub rotate: u32,
    /// Color space
    pub colorspace: u32,
    /// Reserved
    pub reserved: [u32; 4],
}

impl Default for FbVarScreeninfo {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 32,
            grayscale: 0,
            red: FbBitfield { offset: 16, length: 8, msb_right: 0 },
            green: FbBitfield { offset: 8, length: 8, msb_right: 0 },
            blue: FbBitfield { offset: 0, length: 8, msb_right: 0 },
            transp: FbBitfield { offset: 24, length: 8, msb_right: 0 },
            nonstd: 0,
            activate: 0,
            height: 0,
            width: 0,
            accel_flags: 0,
            pixclock: 0,
            left_margin: 0,
            right_margin: 0,
            upper_margin: 0,
            lower_margin: 0,
            hsync_len: 0,
            vsync_len: 0,
            sync: 0,
            vmode: 0,
            rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        }
    }
}

/// Create FbFixScreeninfo from FrameBufferInfo
pub fn create_fix_screeninfo(info: &FrameBufferInfo) -> FbFixScreeninfo {
    let mut fix = FbFixScreeninfo::default();

    // Set driver name
    let name = b"virtio-gpu\0";
    let len = name.len().min(16);
    fix.id[..len].copy_from_slice(&name[..len]);

    fix.smem_start = info.addr;
    fix.smem_len = info.size;
    fix.line_length = info.stride; // stride is already in bytes

    fix
}

/// Create FbVarScreeninfo from FrameBufferInfo
pub fn create_var_screeninfo(info: &FrameBufferInfo) -> FbVarScreeninfo {
    let mut var = FbVarScreeninfo::default();

    var.xres = info.width;
    var.yres = info.height;
    var.xres_virtual = info.width;
    var.yres_virtual = info.height;
    var.bits_per_pixel = 32;

    // xRGB format (little-endian)
    var.red = FbBitfield { offset: 16, length: 8, msb_right: 0 };
    var.green = FbBitfield { offset: 8, length: 8, msb_right: 0 };
    var.blue = FbBitfield { offset: 0, length: 8, msb_right: 0 };
    var.transp = FbBitfield { offset: 24, length: 8, msb_right: 0 };

    var
}

/// Handle framebuffer ioctl commands
/// Returns: 0 on success, negative error code on failure
pub fn fbdev_ioctl(cmd: u32, arg: usize) -> i64 {
    let info = match super::get_framebuffer_info() {
        Some(info) => info,
        None => return -6, // ENXIO: device does not exist
    };

    match cmd {
        FBIOGET_FSCREENINFO => {
            let fix = create_fix_screeninfo(&info);
            // SAFETY: arg is a valid kernel pointer provided by the ioctl caller;
            // fix is a properly initialized FbFixScreeninfo value.
            unsafe {
                // Copy struct to user space
                let dest = arg as *mut FbFixScreeninfo;
                core::ptr::write_volatile(dest, fix);
            }
            0
        }
        FBIOGET_VSCREENINFO => {
            let var = create_var_screeninfo(&info);
            // SAFETY: arg is a valid kernel pointer provided by the ioctl caller;
            // var is a properly initialized FbVarScreeninfo value.
            unsafe {
                let dest = arg as *mut FbVarScreeninfo;
                core::ptr::write_volatile(dest, var);
            }
            0
        }
        FBIO_FLUSH => {
            // Flush framebuffer to display device
            // VirtIO-GPU requires explicit flush to display updated content
            if super::flush_framebuffer() {
                0
            } else {
                -6 // ENXIO: device does not exist
            }
        }
        _ => -25, // ENOTTY: unsupported ioctl command
    }
}
