//! 双缓冲系统
//!
//! 提供无闪烁的图形渲染：所有绘图操作在后端缓冲区进行，
//! 完成后一次性复制到前端 framebuffer。

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use crate::drivers::gpu::framebuffer::FrameBuffer;
use crate::graphics::font::Framebuffer;

/// 双缓冲管理器
pub struct DoubleBuffer {
    /// 后端缓冲区（RGBA 格式，每个像素 4 字节）
    back_buffer: Vec<u32>,
    /// 屏幕宽度
    width: u32,
    /// 屏幕高度
    height: u32,
    /// 每行像素数（stride）
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
    ///
    /// # 参数
    /// - `width`: 屏幕宽度
    /// - `height`: 屏幕高度
    /// - `stride`: 每行字节数（通常是 width * 4）
    pub fn init(&mut self, width: u32, height: u32, stride: u32) {
        if self.initialized {
            return;
        }

        self.width = width;
        self.height = height;
        self.stride = stride;

        // 分配后端缓冲区
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

    /// 获取 stride
    #[inline]
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// 在后端缓冲区绘制单个像素
    #[inline]
    pub fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if !self.initialized || x >= self.width || y >= self.height {
            return;
        }

        let offset = (y * self.stride + x) as usize;
        if offset < self.back_buffer.len() {
            // SAFETY: 我们已检查边界，且 DoubleBuffer 是单线程使用
            unsafe {
                let ptr = self.back_buffer.as_ptr() as *mut u32;
                core::ptr::write_volatile(ptr.add(offset), color);
            }
        }
    }

    /// 获取后端缓冲区的像素颜色
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

    /// 在后端缓冲区填充矩形
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
        if !self.initialized {
            return;
        }

        // 上边
        self.fill_rect(x, y, width, thickness, color);
        // 下边
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        // 左边
        self.fill_rect(x, y, thickness, height, color);
        // 右边
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    /// 清空后端缓冲区
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

    /// 从另一个区域复制像素（位块传输）
    pub fn copy_rect(&self, src_x: u32, src_y: u32, dst_x: u32, dst_y: u32, width: u32, height: u32) {
        if !self.initialized {
            return;
        }

        // 简单实现：逐像素复制
        // 注意：这里需要处理重叠区域的情况
        for py in 0..height {
            for px in 0..width {
                let sx = src_x + px;
                let sy = src_y + py;
                let dx = dst_x + px;
                let dy = dst_y + py;

                if sx < self.width && sy < self.height && dx < self.width && dy < self.height {
                    let color = self.get_pixel(sx, sy);
                    self.put_pixel(dx, dy, color);
                }
            }
        }
    }

    /// 将后端缓冲区复制到前端 framebuffer（页面翻转）
    pub fn swap_buffers(&self, fb: &FrameBuffer) {
        if !self.initialized {
            return;
        }

        // 逐像素复制到前端缓冲区
        // TODO: 优化为批量复制（如果 framebuffer 支持的话）
        for y in 0..self.height {
            for x in 0..self.width {
                let color = self.get_pixel(x, y);
                fb.put_pixel(x, y, color);
            }
        }
    }

    /// 将指定区域复制到前端 framebuffer（局部更新）
    pub fn swap_region(&self, fb: &FrameBuffer, x: u32, y: u32, width: u32, height: u32) {
        if !self.initialized {
            return;
        }

        let x_end = (x + width).min(self.width);
        let y_end = (y + height).min(self.height);

        for py in y..y_end {
            for px in x..x_end {
                let color = self.get_pixel(px, py);
                fb.put_pixel(px, py, color);
            }
        }
    }

    /// 获取后端缓冲区指针（用于高级操作）
    pub fn as_ptr(&self) -> *const u32 {
        self.back_buffer.as_ptr()
    }

    /// 获取后端缓冲区可变指针（用于高级操作）
    pub fn as_mut_ptr(&mut self) -> *mut u32 {
        self.back_buffer.as_mut_ptr()
    }

    /// 获取缓冲区大小（像素数）
    pub fn buffer_size(&self) -> usize {
        self.back_buffer.len()
    }
}

/// 为 DoubleBuffer 实现 Framebuffer trait
impl Framebuffer for DoubleBuffer {
    fn put_pixel(&self, x: u32, y: u32, color: u32) {
        self.put_pixel(x, y, color);
    }
}

/// 全局双缓冲实例
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

static DOUBLE_BUFFER: Mutex<Option<DoubleBuffer>> = Mutex::new(None);
static DB_INIT: AtomicBool = AtomicBool::new(false);

/// 初始化全局双缓冲系统
pub fn init(width: u32, height: u32, stride: u32) {
    if DB_INIT.load(Ordering::Acquire) {
        return;
    }

    let mut db = DOUBLE_BUFFER.lock();
    let mut buffer = DoubleBuffer::new();
    buffer.init(width, height, stride);
    *db = Some(buffer);

    DB_INIT.store(true, Ordering::Release);
}

/// 检查双缓冲是否已初始化
pub fn is_initialized() -> bool {
    DB_INIT.load(Ordering::Acquire)
}

/// 获取双缓冲（用于绘图操作）
pub fn get_buffer() -> Option<&'static DoubleBuffer> {
    if !DB_INIT.load(Ordering::Acquire) {
        return None;
    }

    // SAFETY: 这是一个静态 Mutex 保护的值，我们只返回不可变引用
    // 绘图操作通过 DoubleBuffer 的内部方法完成
    let db = DOUBLE_BUFFER.lock();
    // 这里有一个问题：我们不能从 lock guard 返回引用
    // 所以我们使用另一种方式
    None
}

/// 在双缓冲上执行绘图操作
pub fn with_buffer<F>(f: F)
where
    F: FnOnce(&DoubleBuffer),
{
    if !DB_INIT.load(Ordering::Acquire) {
        return;
    }

    let db = DOUBLE_BUFFER.lock();
    if let Some(buffer) = db.as_ref() {
        f(buffer);
    }
}

/// 将后端缓冲区复制到前端 framebuffer
pub fn swap_buffers(fb: &FrameBuffer) {
    if !DB_INIT.load(Ordering::Acquire) {
        return;
    }

    let db = DOUBLE_BUFFER.lock();
    if let Some(buffer) = db.as_ref() {
        buffer.swap_buffers(fb);
    }
}

/// 清空双缓冲
pub fn clear(color: u32) {
    if !DB_INIT.load(Ordering::Acquire) {
        return;
    }

    let db = DOUBLE_BUFFER.lock();
    if let Some(buffer) = db.as_ref() {
        buffer.clear(color);
    }
}
