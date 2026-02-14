//! 鼠标光标渲染
//!
//! 提供鼠标光标的显示和位置跟踪功能

extern crate alloc;
use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicI32, AtomicBool, Ordering};

/// 默认箭头光标 (16x16)
///
/// X = 前景色（黑色）
/// . = 背景色（白色）
/// 空格 = 透明
const ARROW_CURSOR: [u16; 16] = [
    0b0000000000000001, // _
    0b0000000000000011, // __
    0b0000000000000111, // ___
    0b0000000000001111, // ____
    0b0000000000011111, // _____
    0b0000000000111111, // ______
    0b0000000001111111, // _______
    0b0000000011111111, // ________
    0b0000000111111111, // _________
    0b0000001111111111, // __________
    0b0000011111111111, // ___________
    0b0000000000000111, // ___        (尾部)
    0b0000000000000111, // ___
    0b0000000000000011, // __
    0b0000000000000011, // __
    0b0000000000000001, // _
];

/// 光标掩码（用于背景）
const ARROW_MASK: [u16; 16] = [
    0b0000000000000011,
    0b0000000000000111,
    0b0000000000001111,
    0b0000000000011111,
    0b0000000000111111,
    0b0000000001111111,
    0b0000000011111111,
    0b0000000111111111,
    0b0000001111111111,
    0b0000011111111111,
    0b0000111111111111,
    0b0000000000001111,
    0b0000000000001111,
    0b0000000000000111,
    0b0000000000000111,
    0b0000000000000011,
];

/// 光标颜色常量
pub mod cursor_color {
    pub const BLACK: u32 = 0xFF000000;
    pub const WHITE: u32 = 0xFFFFFFFF;
    pub const TRANSPARENT: u32 = 0x00000000;
}

/// 鼠标光标状态
pub struct MouseCursor {
    /// X 坐标
    x: AtomicI32,
    /// Y 坐标
    y: AtomicI32,
    /// 屏幕宽度
    screen_width: u32,
    /// 屏幕高度
    screen_height: u32,
    /// 光标宽度
    cursor_width: u32,
    /// 光标高度
    cursor_height: u32,
    /// 是否可见
    visible: AtomicBool,
    /// 是否已初始化
    initialized: AtomicBool,
}

impl MouseCursor {
    /// 创建新的鼠标光标
    pub const fn new() -> Self {
        Self {
            x: AtomicI32::new(0),
            y: AtomicI32::new(0),
            screen_width: 0,
            screen_height: 0,
            cursor_width: 16,
            cursor_height: 16,
            visible: AtomicBool::new(true),
            initialized: AtomicBool::new(false),
        }
    }

    /// 初始化光标
    pub fn init(&mut self, screen_width: u32, screen_height: u32) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        self.screen_width = screen_width;
        self.screen_height = screen_height;

        // 将光标初始位置设置在屏幕中央
        self.x.store((screen_width / 2) as i32, Ordering::Release);
        self.y.store((screen_height / 2) as i32, Ordering::Release);

        self.initialized.store(true, Ordering::Release);
    }

    /// 移动光标
    pub fn move_by(&self, dx: i16, dy: i16) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }

        let new_x = self.x.load(Ordering::Acquire) + dx as i32;
        let new_y = self.y.load(Ordering::Acquire) + dy as i32;

        // 限制在屏幕范围内
        let clamped_x = new_x.clamp(0, (self.screen_width - 1) as i32);
        let clamped_y = new_y.clamp(0, (self.screen_height - 1) as i32);

        self.x.store(clamped_x, Ordering::Release);
        self.y.store(clamped_y, Ordering::Release);
    }

    /// 设置光标位置
    pub fn set_position(&self, x: i32, y: i32) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }

        let clamped_x = x.clamp(0, (self.screen_width - 1) as i32);
        let clamped_y = y.clamp(0, (self.screen_height - 1) as i32);

        self.x.store(clamped_x, Ordering::Release);
        self.y.store(clamped_y, Ordering::Release);
    }

    /// 获取光标位置
    pub fn get_position(&self) -> (i32, i32) {
        (
            self.x.load(Ordering::Acquire),
            self.y.load(Ordering::Acquire),
        )
    }

    /// 设置可见性
    pub fn set_visible(&self, visible: bool) {
        self.visible.store(visible, Ordering::Release);
    }

    /// 获取可见性
    pub fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }

    /// 在缓冲区上绘制光标
    ///
    /// # 参数
    /// - `buffer`: 像素缓冲区（u32 数组）
    /// - `buffer_width`: 缓冲区宽度
    /// - `buffer_height`: 缓冲区高度
    pub fn draw(&self, buffer: &mut [u32], buffer_width: u32, buffer_height: u32) {
        if !self.initialized.load(Ordering::Acquire) || !self.visible.load(Ordering::Acquire) {
            return;
        }

        let cursor_x = self.x.load(Ordering::Acquire) as u32;
        let cursor_y = self.y.load(Ordering::Acquire) as u32;

        // 绘制光标
        for py in 0..self.cursor_height {
            for px in 0..self.cursor_width {
                let screen_x = cursor_x + px;
                let screen_y = cursor_y + py;

                // 检查边界
                if screen_x >= buffer_width || screen_y >= buffer_height {
                    continue;
                }

                let offset = (screen_y * buffer_width + screen_x) as usize;
                if offset >= buffer.len() {
                    continue;
                }

                // 检查掩码和光标位
                let mask_bit = (ARROW_MASK[py as usize] >> (15 - px)) & 1;
                let cursor_bit = (ARROW_CURSOR[py as usize] >> (15 - px)) & 1;

                if mask_bit != 0 {
                    // 绘制背景（白色）
                    if cursor_bit != 0 {
                        // 绘制前景（黑色）
                        buffer[offset] = cursor_color::BLACK;
                    } else {
                        // 绘制背景（白色）
                        buffer[offset] = cursor_color::WHITE;
                    }
                }
                // 如果 mask_bit == 0，则透明，不修改像素
            }
        }
    }

    /// 在双缓冲上绘制光标（使用 DoubleBuffer 的接口）
    pub fn draw_on_framebuffer<F>(&self, fb: &F)
    where
        F: crate::graphics::font::Framebuffer,
    {
        if !self.initialized.load(Ordering::Acquire) || !self.visible.load(Ordering::Acquire) {
            return;
        }

        let cursor_x = self.x.load(Ordering::Acquire) as u32;
        let cursor_y = self.y.load(Ordering::Acquire) as u32;

        // 绘制光标
        for py in 0..self.cursor_height {
            for px in 0..self.cursor_width {
                let screen_x = cursor_x + px;
                let screen_y = cursor_y + py;

                // 检查掩码和光标位
                let mask_bit = (ARROW_MASK[py as usize] >> (15 - px)) & 1;
                let cursor_bit = (ARROW_CURSOR[py as usize] >> (15 - px)) & 1;

                if mask_bit != 0 {
                    if cursor_bit != 0 {
                        fb.put_pixel(screen_x, screen_y, cursor_color::BLACK);
                    } else {
                        fb.put_pixel(screen_x, screen_y, cursor_color::WHITE);
                    }
                }
            }
        }
    }
}

/// 全局鼠标光标实例
static MOUSE_CURSOR: Mutex<MouseCursor> = Mutex::new(MouseCursor::new());

/// 初始化鼠标光标
pub fn init(screen_width: u32, screen_height: u32) {
    let mut cursor = MOUSE_CURSOR.lock();
    cursor.init(screen_width, screen_height);
}

/// 移动光标
pub fn move_cursor(dx: i16, dy: i16) {
    let cursor = MOUSE_CURSOR.lock();
    cursor.move_by(dx, dy);
}

/// 设置光标位置
pub fn set_cursor_position(x: i32, y: i32) {
    let cursor = MOUSE_CURSOR.lock();
    cursor.set_position(x, y);
}

/// 获取光标位置
pub fn get_cursor_position() -> (i32, i32) {
    let cursor = MOUSE_CURSOR.lock();
    cursor.get_position()
}

/// 设置光标可见性
pub fn set_cursor_visible(visible: bool) {
    let cursor = MOUSE_CURSOR.lock();
    cursor.set_visible(visible);
}

/// 绘制光标到缓冲区
pub fn draw_cursor(buffer: &mut [u32], buffer_width: u32, buffer_height: u32) {
    let cursor = MOUSE_CURSOR.lock();
    cursor.draw(buffer, buffer_width, buffer_height);
}

/// 绘制光标到 framebuffer
pub fn draw_cursor_on_framebuffer<F>(fb: &F)
where
    F: crate::graphics::font::Framebuffer,
{
    let cursor = MOUSE_CURSOR.lock();
    cursor.draw_on_framebuffer(fb);
}
