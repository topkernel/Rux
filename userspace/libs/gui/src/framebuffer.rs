//! Framebuffer basic drawing interface
//!
//! Provides basic pixel-level drawing operations

use core::ptr::write_volatile;
use core::ptr::read_volatile;

/// System call numbers (RISC-V Linux ABI)
mod syscall {
    pub const SYS_OPENAT: usize = 56;
    pub const SYS_IOCTL: usize = 29;
    pub const SYS_MMAP: usize = 222;
    pub const SYS_CLOSE: usize = 57;

    /// Framebuffer ioctl commands
    pub const FBIOGET_FSCREENINFO: u32 = 0x4602;
    pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
    /// Flush framebuffer (VirtIO-GPU specific)
    pub const FBIO_FLUSH: u32 = 0x4610;
}

/// Protection flags
mod prot {
    pub const PROT_READ: u32 = 0x1;
    pub const PROT_WRITE: u32 = 0x2;
}

/// Mapping flags
mod map {
    pub const MAP_SHARED: u32 = 0x01;
}

/// openat flags
mod open_flags {
    pub const O_RDWR: u32 = 0x2;
}

/// AT_FDCWD
const AT_FDCWD: isize = -100;

/// Special fd representing framebuffer device
/// (kernel convention: fd >= 1000 indicates framebuffer)
pub const FBDEV_FD: i32 = 1000;

/// Fixed screen info (corresponds to kernel fbdev.rs)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbFixScreeninfo {
    pub id: [u8; 16],
    pub smem_start: u64,
    pub smem_len: u32,
    pub type_: u32,
    pub visual: u32,
    pub line_length: u32,
    pub mmio_start: u64,
    pub mmio_len: u32,
    pub accel: u32,
    pub capabilities: u16,
    pub reserved: [u16; 2],
}

/// Color bitfield
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbBitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

/// Variable screen info (corresponds to kernel fbdev.rs)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbVarScreeninfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub red: FbBitfield,
    pub green: FbBitfield,
    pub blue: FbBitfield,
    pub transp: FbBitfield,
    pub nonstd: u32,
    pub activate: u32,
    pub height: u32,
    pub width: u32,
    pub accel_flags: u32,
    pub pixclock: u32,
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    pub sync: u32,
    pub vmode: u32,
    pub rotate: u32,
    pub colorspace: u32,
    pub reserved: [u32; 4],
}

impl Default for FbVarScreeninfo {
    fn default() -> Self {
        Self {
            xres: 0, yres: 0,
            xres_virtual: 0, yres_virtual: 0,
            xoffset: 0, yoffset: 0,
            bits_per_pixel: 32,
            grayscale: 0,
            red: FbBitfield::default(),
            green: FbBitfield::default(),
            blue: FbBitfield::default(),
            transp: FbBitfield::default(),
            nonstd: 0, activate: 0,
            height: 0, width: 0,
            accel_flags: 0, pixclock: 0,
            left_margin: 0, right_margin: 0,
            upper_margin: 0, lower_margin: 0,
            hsync_len: 0, vsync_len: 0,
            sync: 0, vmode: 0, rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        }
    }
}

/// System call wrapper function - RISC-V version
#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn syscall3(num: usize, arg0: usize, arg1: usize, arg2: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        inlateout("a0") arg0 => ret,
        in("a1") arg1,
        in("a2") arg2,
        in("a7") num,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn syscall6(num: usize, arg0: usize, arg1: usize, arg2: usize,
                   arg3: usize, arg4: usize, arg5: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "ecall",
        inlateout("a0") arg0 => ret,
        in("a1") arg1,
        in("a2") arg2,
        in("a3") arg3,
        in("a4") arg4,
        in("a5") arg5,
        in("a7") num,
        options(nostack)
    );
    ret
}

/// System call wrapper function - non-RISC-V platforms (for development/testing)
#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
unsafe fn syscall3(_num: usize, _arg0: usize, _arg1: usize, _arg2: usize) -> isize {
    // Return error on non-RISC-V platforms
    -1
}

#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
unsafe fn syscall6(_num: usize, _arg0: usize, _arg1: usize, _arg2: usize,
                   _arg3: usize, _arg4: usize, _arg5: usize) -> isize {
    // Return error on non-RISC-V platforms
    -1
}

/// Color constants (xRGB format)
pub mod color {
    pub const BLACK: u32 = 0xFF000000;
    pub const WHITE: u32 = 0xFFFFFFFF;
    pub const RED: u32 = 0xFFFF0000;
    pub const GREEN: u32 = 0xFF00FF00;
    pub const BLUE: u32 = 0xFF0000FF;
    pub const YELLOW: u32 = 0xFFFFFF00;
    pub const CYAN: u32 = 0xFF00FFFF;
    pub const MAGENTA: u32 = 0xFFFF00FF;
    pub const GRAY: u32 = 0xFF808080;
    pub const DARK_GRAY: u32 = 0xFF404040;
    pub const LIGHT_GRAY: u32 = 0xFFC0C0C0;
    pub const TRANSPARENT: u32 = 0x00000000;
}

/// Framebuffer drawing trait
pub trait Framebuffer {
    fn put_pixel(&self, x: u32, y: u32, color: u32);
    fn width(&self) -> u32;
    fn height(&self) -> u32;

    fn fill_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32) {
        let x_end = (x + width).min(self.width());
        let y_end = (y + height).min(self.height());
        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, color);
            }
        }
    }

    fn blit_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32, thickness: u32) {
        self.fill_rect(x, y, width, thickness, color);
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        self.fill_rect(x, y, thickness, height, color);
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.width(), self.height(), color);
    }

    fn draw_line_h(&self, x: u32, y: u32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    fn draw_line_v(&self, x: u32, y: u32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    fn draw_line(&self, x0: u32, y0: u32, x1: u32, y1: u32, color: u32) {
        let mut x0 = x0 as i32;
        let mut y0 = y0 as i32;
        let x1 = x1 as i32;
        let y1 = y1 as i32;

        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };

        let mut err = dx + dy;

        loop {
            self.put_pixel(x0 as u32, y0 as u32, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }
}

/// Framebuffer info
#[derive(Clone, Copy, Debug)]
pub struct FramebufferInfo {
    /// Framebuffer address
    pub addr: usize,
    /// Framebuffer size (bytes)
    pub size: u32,
    /// Width (pixels)
    pub width: u32,
    /// Height (pixels)
    pub height: u32,
    /// Bytes per line
    pub stride: u32,
}

/// Framebuffer device
pub struct FramebufferDevice {
    /// Framebuffer info
    info: FramebufferInfo,
    /// Framebuffer start pointer
    ptr: *mut u8,
    /// File descriptor (for ioctl)
    fd: i32,
}

unsafe impl Send for FramebufferDevice {}
unsafe impl Sync for FramebufferDevice {}

impl FramebufferDevice {
    /// Open framebuffer device
    ///
    /// Uses ioctl to get screen info, then mmaps to user space
    ///
    /// # Returns
    /// Returns Some(FramebufferDevice) on success, None on failure
    pub fn open() -> Option<Self> {
        unsafe {
            // Use special fd 1000 to indicate framebuffer device
            let fd = FBDEV_FD;

            // Get fixed screen info
            let mut fix_info: FbFixScreeninfo = core::mem::zeroed();
            let ret = syscall3(
                syscall::SYS_IOCTL,
                fd as usize,
                syscall::FBIOGET_FSCREENINFO as usize,
                &mut fix_info as *mut _ as usize,
            );
            if ret < 0 {
                return None;
            }

            // Get variable screen info
            let mut var_info: FbVarScreeninfo = core::mem::zeroed();
            let ret = syscall3(
                syscall::SYS_IOCTL,
                fd as usize,
                syscall::FBIOGET_VSCREENINFO as usize,
                &mut var_info as *mut _ as usize,
            );
            if ret < 0 {
                return None;
            }

            // mmap the framebuffer
            let fb_size = fix_info.smem_len as usize;
            let fb_ptr = syscall6(
                syscall::SYS_MMAP,
                0,
                fb_size,
                (prot::PROT_READ | prot::PROT_WRITE) as usize,
                map::MAP_SHARED as usize,
                fd as usize,
                0,
            );

            // MAP_FAILED = -1
            if fb_ptr == -1_isize {
                return None;
            }

            let device = Self {
                info: FramebufferInfo {
                    addr: fb_ptr as usize,
                    size: fix_info.smem_len,
                    width: var_info.xres,
                    height: var_info.yres,
                    stride: fix_info.line_length / 4,
                },
                ptr: fb_ptr as usize as *mut u8,
                fd,
            };

            Some(device)
        }
    }

    /// Create new Framebuffer
    ///
    /// # Safety
    /// `addr` must be a valid address
    pub unsafe fn new(addr: usize, info: FramebufferInfo) -> Self {
        let ptr = addr as *mut u8;
        Self { info, ptr, fd: FBDEV_FD }
    }

    /// Create from raw pointer
    pub unsafe fn from_raw(addr: usize, width: u32, height: u32) -> Self {
        let stride = width * 4;
        let size = stride * height;
        Self {
            info: FramebufferInfo {
                addr,
                size,
                width,
                height,
                stride,
            },
            ptr: addr as *mut u8,
            fd: FBDEV_FD,
        }
    }

    /// Get width
    #[inline]
    pub fn width(&self) -> u32 {
        self.info.width
    }

    /// Get height
    #[inline]
    pub fn height(&self) -> u32 {
        self.info.height
    }

    /// Get bytes per line
    #[inline]
    pub fn stride(&self) -> u32 {
        self.info.stride
    }

    /// Get framebuffer pointer
    #[inline]
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// Bulk copy data to framebuffer (efficient version)
    ///
    /// Copies data from source buffer to framebuffer, assumes stride == width
    pub fn copy_from_buffer(&self, src: &[u32]) {
        let total_pixels = (self.width() * self.height()) as usize;
        let copy_len = src.len().min(total_pixels);

        unsafe {
            let dst = self.ptr as *mut u32;
            core::ptr::copy_nonoverlapping(src.as_ptr(), dst, copy_len);
        }
    }

    /// Get framebuffer info
    #[inline]
    pub fn info(&self) -> &FramebufferInfo {
        &self.info
    }

    /// Draw single pixel
    #[inline]
    pub fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if x >= self.width() || y >= self.height() {
            return;
        }

        unsafe {
            // stride is pixels per line, each pixel is 4 bytes
            let offset = ((y * self.stride() + x) * 4) as usize;
            let pixel_ptr = self.ptr.add(offset) as *mut u32;
            write_volatile(pixel_ptr, color);
        }
    }

    /// Get pixel color
    #[inline]
    pub fn get_pixel(&self, x: u32, y: u32) -> u32 {
        if x >= self.width() || y >= self.height() {
            return 0;
        }

        unsafe {
            // stride is pixels per line, each pixel is 4 bytes
            let offset = ((y * self.stride() + x) * 4) as usize;
            let pixel_ptr = self.ptr.add(offset) as *const u32;
            read_volatile(pixel_ptr)
        }
    }

    /// Flush framebuffer to display device
    ///
    /// VirtIO-GPU requires explicit flush to display updated content
    /// This method should be called after each drawing operation
    pub fn flush(&self) -> bool {
        unsafe {
            let ret = syscall3(
                syscall::SYS_IOCTL,
                self.fd as usize,
                syscall::FBIO_FLUSH as usize,
                0,  // arg parameter unused
            );
            ret >= 0
        }
    }

    /// Fill rectangle
    pub fn fill_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32) {
        let x_end = (x + width).min(self.width());
        let y_end = (y + height).min(self.height());

        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, color);
            }
        }
    }

    /// Draw rectangle border
    pub fn blit_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32, thickness: u32) {
        // Top edge
        self.fill_rect(x, y, width, thickness, color);
        // Bottom edge
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        // Left edge
        self.fill_rect(x, y, thickness, height, color);
        // Right edge
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    /// Clear screen
    pub fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.width(), self.height(), color);
    }

    /// Draw horizontal line
    pub fn draw_line_h(&self, x: u32, y: u32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    /// Draw vertical line
    pub fn draw_line_v(&self, x: u32, y: u32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    /// Draw line segment (Bresenham's algorithm)
    pub fn draw_line(&self, x0: u32, y0: u32, x1: u32, y1: u32, color: u32) {
        let mut x0 = x0 as i32;
        let mut y0 = y0 as i32;
        let x1 = x1 as i32;
        let y1 = y1 as i32;

        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };

        let mut err = dx + dy;

        loop {
            self.put_pixel(x0 as u32, y0 as u32, color);

            if x0 == x1 && y0 == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draw circle
    pub fn draw_circle(&self, cx: u32, cy: u32, radius: u32, color: u32, fill: bool) {
        let cx = cx as i32;
        let cy = cy as i32;
        let radius = radius as i32;

        if fill {
            for y in -radius..=radius {
                for x in -radius..=radius {
                    if x * x + y * y <= radius * radius {
                        self.put_pixel((cx + x) as u32, (cy + y) as u32, color);
                    }
                }
            }
        } else {
            let mut x = radius;
            let mut y = 0i32;
            let mut err = 0i32;

            while x >= y {
                self.put_pixel((cx + x) as u32, (cy + y) as u32, color);
                self.put_pixel((cx + y) as u32, (cy + x) as u32, color);
                self.put_pixel((cx - y) as u32, (cy + x) as u32, color);
                self.put_pixel((cx - x) as u32, (cy + y) as u32, color);
                self.put_pixel((cx - x) as u32, (cy - y) as u32, color);
                self.put_pixel((cx - y) as u32, (cy - x) as u32, color);
                self.put_pixel((cx + y) as u32, (cy - x) as u32, color);
                self.put_pixel((cx + x) as u32, (cy - y) as u32, color);

                y += 1;
                err += 1 + 2 * y;
                if 2 * (err - x) + 1 > 0 {
                    x -= 1;
                    err += 1 - 2 * x;
                }
            }
        }
    }

    /// Copy rectangle region
    pub fn copy_rect(&self, src_x: u32, src_y: u32, dst_x: u32, dst_y: u32, width: u32, height: u32) {
        for py in 0..height {
            for px in 0..width {
                let sx = src_x + px;
                let sy = src_y + py;
                let dx = dst_x + px;
                let dy = dst_y + py;

                if sx < self.width() && sy < self.height() && dx < self.width() && dy < self.height() {
                    let color = self.get_pixel(sx, sy);
                    self.put_pixel(dx, dy, color);
                }
            }
        }
    }
}

/// Implement Framebuffer trait for FramebufferDevice
impl Framebuffer for FramebufferDevice {
    fn put_pixel(&self, x: u32, y: u32, color: u32) {
        self.put_pixel(x, y, color);
    }

    fn width(&self) -> u32 {
        self.width()
    }

    fn height(&self) -> u32 {
        self.height()
    }
}
