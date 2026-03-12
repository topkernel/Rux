//! Double buffering system
//!
//! Provides flicker-free graphics rendering

use std::vec;
use std::vec::Vec;
use crate::framebuffer::Framebuffer;

/// Double buffer manager
pub struct DoubleBuffer {
    /// Back buffer
    back_buffer: Vec<u32>,
    /// Screen width
    width: u32,
    /// Screen height
    height: u32,
    /// Pixels per line
    stride: u32,
    /// Whether initialized
    initialized: bool,
}

impl DoubleBuffer {
    /// Create new double buffering system
    pub fn new() -> Self {
        Self {
            back_buffer: Vec::new(),
            width: 0,
            height: 0,
            stride: 0,
            initialized: false,
        }
    }

    /// Initialize double buffer
    pub fn init(&mut self, width: u32, height: u32, stride: u32) {
        if self.initialized {
            return;
        }

        self.width = width;
        self.height = height;
        self.stride = stride;

        let buffer_size = (stride * height) as usize;
        self.back_buffer = vec![0u32; buffer_size];

        self.initialized = true;
    }

    /// Check if initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get width
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get height
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Draw pixel
    #[inline]
    pub fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if !self.initialized || x >= self.width || y >= self.height {
            return;
        }

        let offset = (y * self.stride + x) as usize;
        if offset < self.back_buffer.len() {
            unsafe {
                let ptr = self.back_buffer.as_ptr() as *mut u32;
                core::ptr::write_volatile(ptr.add(offset), color);
            }
        }
    }

    /// Get pixel
    #[inline]
    pub fn get_pixel(&self, x: u32, y: u32) -> u32 {
        if !self.initialized || x >= self.width || y >= self.height {
            return 0;
        }

        let offset = (y * self.stride + x) as usize;
        if offset < self.back_buffer.len() {
            self.back_buffer[offset]
        } else {
            0
        }
    }

    /// Fill rectangle
    pub fn fill_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32) {
        if !self.initialized {
            return;
        }

        let x_end = (x + width).min(self.width);
        let y_end = (y + height).min(self.height);

        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, color);
            }
        }
    }

    /// Draw rectangle border
    pub fn blit_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32, thickness: u32) {
        self.fill_rect(x, y, width, thickness, color);
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        self.fill_rect(x, y, thickness, height, color);
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    /// Clear
    pub fn clear(&self, color: u32) {
        if !self.initialized {
            return;
        }
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// Draw horizontal line
    pub fn draw_line_h(&self, x: u32, y: u32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    /// Draw vertical line
    pub fn draw_line_v(&self, x: u32, y: u32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    /// Draw line segment
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

    /// Copy to front framebuffer (efficient version)
    ///
    /// Uses bulk memory copy instead of per-pixel copy
    pub fn swap_buffers_fast(&self, fb: &crate::framebuffer::FramebufferDevice) {
        if !self.initialized {
            return;
        }

        // Directly bulk copy the entire buffer
        fb.copy_from_buffer(&self.back_buffer);
    }
}

impl Default for DoubleBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framebuffer for DoubleBuffer {
    fn put_pixel(&self, x: u32, y: u32, color: u32) {
        self.put_pixel(x, y, color);
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }
}
