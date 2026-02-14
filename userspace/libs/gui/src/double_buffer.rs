//! 双缓冲系统
//!
//! 提供无闪烁的图形渲染

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use crate::framebuffer::Framebuffer;

/// 双缓冲管理器
pub struct DoubleBuffer {
    /// 后端缓冲区
    back_buffer: Vec<u32>,
    /// 屏幕宽度
    width: u32,
    /// 屏幕高度
    height: u32,
    /// 每行像素数
    stride: u32,
    /// 是否已初始化
    initialized: bool,
}

impl DoubleBuffer {
    /// 创建新的双缓冲系统
    pub fn new() -> Self {
        Self {
            back_buffer: Vec::new(),
            width: 0,
            height: 0,
            stride: 0,
            initialized: false,
        }
    }

    /// 初始化双缓冲
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

    /// 检查是否已初始化
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// 获取宽度
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 获取高度
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 绘制像素
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

    /// 获取像素
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

    /// 填充矩形
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

    /// 绘制矩形边框
    pub fn blit_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32, thickness: u32) {
        self.fill_rect(x, y, width, thickness, color);
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        self.fill_rect(x, y, thickness, height, color);
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    /// 清空
    pub fn clear(&self, color: u32) {
        if !self.initialized {
            return;
        }
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// 绘制水平线
    pub fn draw_line_h(&self, x: u32, y: u32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    /// 绘制垂直线
    pub fn draw_line_v(&self, x: u32, y: u32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    /// 绘制线段
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

    /// 复制到前端 framebuffer
    pub fn swap_buffers<F: Framebuffer>(&self, fb: &F) {
        if !self.initialized {
            return;
        }

        for y in 0..self.height {
            for x in 0..self.width {
                let color = self.get_pixel(x, y);
                fb.put_pixel(x, y, color);
            }
        }
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
