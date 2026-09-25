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
#[derive(Clone, Copy)]
/// Framebuffer fixed information — layout matches Linux
/// `struct fb_fix_screeninfo` byte-for-byte (review BUG:
/// "FbFixScreeninfo 布局错位"): type_aux and the three pan steps were
/// missing, shifting visual/line_length/mmio_* by 8+ bytes, so user fb
/// programs (fbset, SDL FB_DEV) read garbage strides and MMIO offsets.
///
/// Offsets: id 0, smem_start 16, smem_len 24, type 28, type_aux 32,
/// visual 36, xpanstep 40, ypanstep 42, ywrapstep 44, (pad 46),
/// line_length 48, (pad 52), mmio_start 56, mmio_len 64, accel 68,
/// capabilities 72, reserved 74; sizeof = 80.
#[repr(C)]
pub struct FbFixScreeninfo {
    /// Driver name (16 bytes)
    pub id: [u8; 16],
    /// Physical memory start address
    pub smem_start: u64,
    /// Physical memory length
    pub smem_len: u32,
    /// Framebuffer type
    pub type_: u32,
    /// Interleave for interleaved framebuffers
    pub type_aux: u32,
    /// Visual type
    pub visual: u32,
    /// Zero if no hardware panning
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
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
            type_aux: 0,
            visual: FB_VISUAL_TRUECOLOR,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
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

    // Bitfields measured empirically against QEMU's virtio-gpu (resource
    // created with format 3): on-screen R comes from V[15:8], G from
    // V[23:16], B from V[31:24]; V[7:0] is ignored. Advertising these
    // offsets lets generic fbdev programs build correct pixels.
    var.red = FbBitfield { offset: 8, length: 8, msb_right: 0 };
    var.green = FbBitfield { offset: 16, length: 8, msb_right: 0 };
    var.blue = FbBitfield { offset: 24, length: 8, msb_right: 0 };
    var.transp = FbBitfield { offset: 0, length: 0, msb_right: 0 };

    var
}

/// /dev/fb0 file operations: a real char device so ioctls and mmap
/// dispatch on the FILE's ops identity (no fd-number heuristics).
pub static FB_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(fb_read),
    write: Some(fb_write),
    lseek: None,
    close: None,
    poll: None,
};

fn fb_read(_file: &crate::fs::File, _buf: &mut [u8]) -> isize {
    -29 // ESPIPE-like: reads unsupported (mmap instead)
}

fn fb_write(_file: &crate::fs::File, _buf: &[u8]) -> isize {
    -29
}

/// True when the File is the /dev/fb0 char device.
pub fn is_fb_file(file: &crate::fs::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &FB_OPS as *const _),
        None => false,
    }
}

/// Register the fb char device and create /dev/fb0 (called from boot
/// after devfs is up and the GPU framebuffer exists).
pub fn init_fbdev() -> Result<(), ()> {
    crate::fs::devfs::registry::register_char_device(
        crate::fs::dev_t::DEV_FB0,
        &FB_OPS,
    )?;
    crate::fs::devfs::mknod("/fb0", crate::fs::dev_t::DEV_FB0, 0o666)
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
            if !crate::arch::riscv64::uaccess::access_ok(arg, core::mem::size_of::<FbFixScreeninfo>()) {
                return -14; // EFAULT
            }
            // SAFETY: access_ok validated the user pointer; fix is a properly initialized value.
            unsafe {
                let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    &fix as *const FbFixScreeninfo as *const u8,
                    core::mem::size_of::<FbFixScreeninfo>(),
                );
                if uncopied != 0 { return -14; }
            }
            0
        }
        FBIOGET_VSCREENINFO => {
            let var = create_var_screeninfo(&info);
            if !crate::arch::riscv64::uaccess::access_ok(arg, core::mem::size_of::<FbVarScreeninfo>()) {
                return -14; // EFAULT
            }
            // SAFETY: access_ok validated the user pointer; var is a properly initialized value.
            unsafe {
                let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    &var as *const FbVarScreeninfo as *const u8,
                    core::mem::size_of::<FbVarScreeninfo>(),
                );
                if uncopied != 0 { return -14; }
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
