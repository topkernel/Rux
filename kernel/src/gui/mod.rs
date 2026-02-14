//! 窗口管理器
//!
//! 提供基础的窗口管理功能

extern crate alloc;
use crate::println;
use crate::drivers::gpu::framebuffer::{FrameBuffer, color};
use crate::graphics::font::FontRenderer;
use alloc::collections::btree_set::BTreeSet;

/// 窗口 ID 类型
pub type WindowId = u32;

/// 窗口状态
#[derive(Debug, Clone, Copy)]
pub enum WindowState {
    /// 正常
    Normal,
    /// 最小化
    Minimized,
    /// 最大化
    Maximized,
}

/// 窗口结构
pub struct Window {
    /// 窗口 ID
    pub id: WindowId,
    /// 窗口标题
    pub title: Vec<u8>,
    /// X 坐标
    pub x: u32,
    /// Y 坐标
    pub y: u32,
    /// 宽度
    pub width: u32,
    /// 高度
    pub height: u32,
    /// Z-order（用于层级管理）
    pub z_order: u32,
    /// 窗口状态
    pub state: WindowState,
    /// 是否可见
    pub visible: bool,
}

impl Window {
    /// 创建新窗口
    pub fn new(id: WindowId, title: &str, x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            id,
            title: title.bytes().collect(),
            x,
            y,
            width,
            height,
            z_order: 0,
            state: WindowState::Normal,
            visible: true,
        }
    }

    /// 检查点是否在窗口内
    pub fn contains(&self, x: u32, y: u32) -> bool {
        if !self.visible {
            return false;
        }

        x >= self.x && x < self.x + self.width &&
        y >= self.y && y < self.y + self.height
    }

    /// 绘制窗口
    pub fn draw(&self, fb: &FrameBuffer, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        use color::*;

        // 绘制窗口阴影
        fb.fill_rect(self.x + 4, self.y + 4, self.width, self.height, DARK_GRAY);

        // 绘制窗口背景
        fb.fill_rect(self.x, self.y, self.width, self.height, WHITE);

        // 绘制窗口边框
        fb.blit_rect(self.x, self.y, self.width, self.height, BLACK, 2);

        // 绘制标题栏
        const TITLE_BAR_HEIGHT: u32 = 20;
        fb.fill_rect(self.x, self.y, self.width, TITLE_BAR_HEIGHT, BLUE);

        // 绘制标题文本（如果宽度足够）
        if self.width > 40 {
            let title_y = self.y + 2;
            let title_x = self.x + 6;
            for (i, byte) in self.title.iter().enumerate() {
                if i < 20 {
                    let char_x = title_x + i as u32 * 8;
                    if char_x + 8 < self.x + self.width {
                        fb.put_pixel(char_x, title_y, BLACK);
                        fb.put_pixel(char_x + 1, title_y, BLACK);
                    }
                }
            }
        }

        // 绘制关闭按钮 X
        let close_x = self.x + self.width - 18;
        let close_y = self.y + 4;
        fb.fill_rect(close_x, close_y, 12, 12, RED);
        fb.draw_line(close_x + 2, close_y + 2, close_x + 10, close_y + 10, WHITE, 2);
        fb.draw_line(close_x + 10, close_y + 2, close_x + 12, close_y + 10, WHITE, 2);
    }
}

/// 窗口管理器
pub struct WindowManager {
    /// 窗口列表
    windows: BTreeSet<WindowId, Window>,
    /// 下一个窗口 ID
    next_window_id: WindowId,
    /// Z-order 计数器
    next_z_order: u32,
    /// 桌面背景颜色
    desktop_color: u32,
}

impl WindowManager {
    /// 创建新窗口管理器
    pub fn new() -> Self {
        Self {
            windows: BTreeSet::new(),
            next_window_id: 1,
            next_z_order: 1,
            desktop_color: color::BLUE,
        }
    }

    /// 创建窗口
    pub fn create_window(&mut self, title: &str, x: u32, y: u32, width: u32, height: u32) -> WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;

        let mut window = Window::new(id, title, x, y, width, height);
        window.z_order = self.next_z_order;
        self.next_z_order += 1;

        self.windows.insert(id, window);
        id
    }

    /// 绘制所有窗口
    pub fn draw_all(&self, fb: &FrameBuffer, font: &FontRenderer) {
        // 绘制桌面背景
        fb.clear(self.desktop_color);

        // 按 Z-order 绘制窗口
        let mut windows: Vec<&Window> = self.windows.iter().collect();
        windows.sort_by_key(|w| w.z_order);

        for window in windows {
            window.draw(fb, font);
        }
    }

    /// 获取窗口
    pub fn get_window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    /// 获取可变窗口
    pub fn get_window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    /// 删除窗口
    pub fn remove_window(&mut self, id: WindowId) -> bool {
        self.windows.remove(&id).is_some()
    }

    /// 设置窗口可见性
    pub fn set_window_visible(&mut self, id: WindowId, visible: bool) -> bool {
        if let Some(window) = self.get_window_mut(id) {
            window.visible = visible;
            true
        } else {
            false
        }
    }

    /// 设置窗口状态
    pub fn set_window_state(&mut self, id: WindowId, state: WindowState) -> bool {
        if let Some(window) = self.get_window_mut(id) {
            window.state = state;
            true
        } else {
            false
        }
    }

    /// 移动窗口
    pub fn move_window(&mut self, id: WindowId, dx: u32, dy: u32) -> bool {
        if let Some(window) = self.get_window_mut(id) {
            window.x = (window.x as u32 + dx).max(0);
            window.y = (window.y as u32 + dy).max(0);
            true
        } else {
            false
        }
    }

    /// 检查点是否被任何窗口覆盖
    pub fn hit_test(&self, x: u32, y: u32) -> bool {
        for window in self.windows.iter() {
            if window.contains(x, y) {
                return true;
            }
        }
        false
    }

    /// 获取窗口列表
    pub fn windows(&self) -> Vec<&Window> {
        self.windows.iter().collect()
    }

    /// 设置桌面颜色
    pub fn set_desktop_color(&mut self, color: u32) {
        self.desktop_color = color;
    }
}

/// 全局窗口管理器
static mut WINDOW_MANAGER: WindowManager = WindowManager::new();

/// 初始化窗口管理器
pub fn init() {
    println!("wm: Initializing window manager...");
    println!("wm: Window manager initialized [OK]");
}

/// 获取窗口管理器
pub fn get_manager() -> &'static mut WindowManager {
    unsafe { &mut WINDOW_MANAGER }
}

/// 创建窗口
pub fn create_window(title: &str, x: u32, y: u32, width: u32, height: u32) -> WindowId {
    let wm = get_manager();
    wm.create_window(title, x, y, width, height)
}

/// 绘制所有窗口
pub fn draw_all_windows(fb: &FrameBuffer, font: &FontRenderer) {
    let wm = get_manager();
    wm.draw_all(fb, font);
}

/// 检查鼠标点击
pub fn check_click(x: u32, y: u32) -> Option<WindowId> {
    let wm = get_manager();
    if let Some(id) = wm.windows().iter().find(|w| w.contains(x, y)) {
        Some(id)
    } else {
        None
    }
}
