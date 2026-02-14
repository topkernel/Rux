//! Framebuffer 基础绘图接口
//!
//! 提供基础的像素级绘图操作

use core::ptr::write_volatile;
use core::ptr::read_volatile;

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
    pub const LIGHT_GRAY: u32 = 0xFFC0C0C0;
    pub const TRANSPARENT: u32 = 0x00000000;
}

/// Framebuffer 绘图 trait
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

/// Framebuffer 信息
#[derive(Clone, Copy, Debug)]
pub struct FramebufferInfo {
    /// Framebuffer 地址
    pub addr: usize,
    /// Framebuffer 大小（字节）
    pub size: u32,
    /// 宽度（像素）
    pub width: u32,
    /// 高度（像素）
    pub height: u32,
    /// 每行字节数
    pub stride: u32,
}

/// Framebuffer 设备
pub struct FramebufferDevice {
    /// Framebuffer 信息
    info: FramebufferInfo,
    /// Framebuffer 起始指针
    ptr: *mut u8,
}

unsafe impl Send for FramebufferDevice {}
unsafe impl Sync for FramebufferDevice {}

impl FramebufferDevice {
    /// 创建新的 Framebuffer
    ///
    /// # Safety
    /// `addr` 必须是有效的地址
    pub unsafe fn new(addr: usize, info: FramebufferInfo) -> Self {
        let ptr = addr as *mut u8;
        Self { info, ptr }
    }

    /// 从原始指针创建
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
        }
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

    /// 获取帧缓冲区指针
    #[inline]
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// 获取帧缓冲区信息
    #[inline]
    pub fn info(&self) -> &FramebufferInfo {
        &self.info
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
            read_volatile(pixel_ptr)
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

    /// 复制矩形区域
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

/// 为 FramebufferDevice 实现 Framebuffer trait
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
