//! Framebuffer 基础绘图接口
//!
//! 提供基础的像素级绘图操作

use core::ptr::write_volatile;

/// Framebuffer 信息
pub struct FrameBufferInfo {
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
    /// 格式（xRGB = 1）
    pub format: u32,
}

/// 颜色常量 (xRGB 格式)
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

/// Framebuffer 结构
pub struct FrameBuffer {
    /// Framebuffer 信息
    info: FrameBufferInfo,
    /// Framebuffer 起始指针
    ptr: *mut u8,
}

unsafe impl Send for FrameBuffer {}
unsafe impl Sync for FrameBuffer {}

impl FrameBuffer {
    /// 创建新的 Framebuffer
    ///
    /// # Safety
    /// `addr` 必须是有效的物理地址，且 `info` 包含正确的信息
    pub unsafe fn new(addr: u64, info: FrameBufferInfo) -> Self {
        // 将物理地址映射为虚拟地址
        // 暂时假设恒等映射（物理地址 = 虚拟地址）
        let ptr = addr as *mut u8;

        Self { info, ptr }
    }

    /// 获取宽度
    #[inline]
    pub fn width(&self) -> u32 {
        self.info.width
    }

    /// 获取高度
    #[inline]
    pub fn height(&self) -> u32 {
        self.info.height
    }

    /// 获取每行字节数
    #[inline]
    pub fn stride(&self) -> u32 {
        self.info.stride
    }

    /// 绘制单个像素
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

    /// 获取像素颜色
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

    /// 填充矩形
    pub fn fill_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32) {
        let x_end = (x + width).min(self.width());
        let y_end = (y + height).min(self.height());

        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, color);
            }
        }
    }

    /// 绘制矩形边框
    pub fn blit_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32, thickness: u32) {
        // 上边
        self.fill_rect(x, y, width, thickness, color);
        // 下边
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        // 左边
        self.fill_rect(x, y, thickness, height, color);
        // 右边
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    /// 清空屏幕
    pub fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.width(), self.height(), color);
    }

    /// 绘制水平线
    pub fn draw_line_h(&self, x: u32, y: u32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    /// 绘制垂直线
    pub fn draw_line_v(&self, x: u32, y: u32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    /// 绘制线段 (Bresenham 算法)
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

    /// 绘制圆
    pub fn draw_circle(&self, cx: u32, cy: u32, radius: u32, color: u32, fill: bool) {
        let cx = cx as i32;
        let cy = cy as i32;
        let radius = radius as i32;

        if fill {
            // 填充圆
            for y in -radius..=radius {
                for x in -radius..=radius {
                    if x * x + y * y <= radius * radius {
                        self.put_pixel((cx + x) as u32, (cy + y) as u32, color);
                    }
                }
            }
        } else {
            // 空心圆 (Midpoint 算法)
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

    /// 绘制位图
    pub fn draw_bitmap(&self, x: u32, y: u32, width: u32, height: u32, data: &[u8], color: u32) {
        for py in 0..height {
            for px in 0..width {
                let byte_index = (py * width + px) / 8;
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

    /// 获取 framebuffer 起始地址
    #[inline]
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// 获取 framebuffer 信息
    #[inline]
    pub fn info(&self) -> &FrameBufferInfo {
        &self.info
    }
}
