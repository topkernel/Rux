//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Framebuffer basic drawing interface
//!
//! Provides basic pixel-level drawing operations

use core::ptr::write_volatile;

/// Framebuffer information
#[derive(Clone, Copy)]
pub struct FrameBufferInfo {
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
    /// Format (xRGB = 1)
    pub format: u32,
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
    pub const LIGHT_BLUE: u32 = 0xFF0000FF;
}

/// Framebuffer structure
pub struct FrameBuffer {
    /// Framebuffer information
    info: FrameBufferInfo,
    /// Framebuffer starting pointer
    ptr: *mut u8,
}

unsafe impl Send for FrameBuffer {}
unsafe impl Sync for FrameBuffer {}

impl FrameBuffer {
    /// Create a new Framebuffer
    ///
    /// # Safety
    /// `addr` must be a valid physical address, and `info` must contain correct information
    pub unsafe fn new(addr: u64, info: FrameBufferInfo) -> Self {
        // Map physical address to virtual address
        // For now, assume identity mapping (physical address = virtual address)
        let ptr = addr as *mut u8;

        Self { info, ptr }
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

    /// Get bytes per row
    #[inline]
    pub fn stride(&self) -> u32 {
        self.info.stride
    }

    /// Draw a single pixel
    #[inline]
    pub fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if x >= self.width() || y >= self.height() {
            return;
        }

        unsafe {
            let offset = (y * self.stride() + x * 4) as usize;
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
            let offset = (y * self.stride() + x * 4) as usize;
            let pixel_ptr = self.ptr.add(offset) as *const u32;
            core::ptr::read_volatile(pixel_ptr)
        }
    }

    /// Fill a rectangle
    pub fn fill_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32) {
        let x_end = (x + width).min(self.width());
        let y_end = (y + height).min(self.height());

        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, color);
            }
        }
    }

    /// Draw a rectangle border
    pub fn blit_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32, thickness: u32) {
        // Guard against underflow when thickness > width/height
        if thickness > width || thickness > height {
            return;
        }
        // Top edge
        self.fill_rect(x, y, width, thickness, color);
        // Bottom edge
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        // Left edge
        self.fill_rect(x, y, thickness, height, color);
        // Right edge
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    /// Clear the screen
    pub fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.width(), self.height(), color);
    }

    /// Draw a horizontal line
    pub fn draw_line_h(&self, x: u32, y: u32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    /// Draw a vertical line
    pub fn draw_line_v(&self, x: u32, y: u32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    /// Draw a line segment (Bresenham's algorithm)
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

    /// Draw a circle
    pub fn draw_circle(&self, cx: u32, cy: u32, radius: u32, color: u32, fill: bool) {
        let cx = cx as i32;
        let cy = cy as i32;
        let radius = radius as i32;

        if fill {
            // Filled circle
            for y in -radius..=radius {
                for x in -radius..=radius {
                    if x * x + y * y <= radius * radius {
                        self.put_pixel((cx + x) as u32, (cy + y) as u32, color);
                    }
                }
            }
        } else {
            // Hollow circle (Midpoint algorithm)
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

    /// Draw a bitmap
    pub fn draw_bitmap(&self, x: u32, y: u32, width: u32, height: u32, data: &[u8], color: u32) {
        for py in 0..height {
            for px in 0..width {
                let byte_index = ((py * width + px) / 8) as usize;
                let bit_index = 7 - ((py * width + px) % 8);

                if byte_index < data.len() {
                    let bit = (data[byte_index] >> bit_index) & 1;
                    if bit != 0 {
                        self.put_pixel(x + px, y + py, color);
                    }
                }
            }
        }
    }

    /// Get framebuffer starting address
    #[inline]
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// Get framebuffer information
    #[inline]
    pub fn info(&self) -> &FrameBufferInfo {
        &self.info
    }
}
