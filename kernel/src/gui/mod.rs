//! 窗口管理器
//!
//! 提供基础的窗口管理功能

extern crate alloc;
use crate::println;
use crate::drivers::gpu::framebuffer::{FrameBuffer, color};
use crate::graphics::font::FontRenderer;
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;

pub mod cursor;
pub mod widgets;

pub use cursor::{
    init as init_cursor,
    move_cursor,
    set_cursor_position,
    get_cursor_position,
    set_cursor_visible,
    draw_cursor,
    draw_cursor_on_framebuffer,
    MouseCursor,
};

pub use widgets::{WidgetId, WidgetState, WidgetEvent, Button, Label, TextBox, SimplePanel};

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
    /// 标题栏高度
    pub title_bar_height: u32,
}

/// 标题栏高度常量
pub const TITLE_BAR_HEIGHT: u32 = 20;

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
            title_bar_height: TITLE_BAR_HEIGHT,
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

    /// 检查点是否在标题栏内（用于拖拽）
    pub fn is_in_title_bar(&self, x: u32, y: u32) -> bool {
        if !self.visible {
            return false;
        }

        x >= self.x && x < self.x + self.width - 20 && // 排除关闭按钮区域
        y >= self.y && y < self.y + self.title_bar_height
    }

    /// 检查点是否在关闭按钮内
    pub fn is_in_close_button(&self, x: u32, y: u32) -> bool {
        if !self.visible {
            return false;
        }

        let close_x = self.x + self.width - 18;
        let close_y = self.y + 4;
        x >= close_x && x < close_x + 12 &&
        y >= close_y && y < close_y + 12
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
        fb.fill_rect(self.x, self.y, self.width, self.title_bar_height, BLUE);

        // 绘制标题文本（使用字体渲染）
        if self.width > 40 {
            let title_y = self.y + 6; // 垂直居中
            let title_x = self.x + 6;
            let max_chars = ((self.width - 30) / 8) as usize; // 预留关闭按钮空间

            for (i, byte) in self.title.iter().enumerate() {
                if i >= max_chars {
                    break;
                }
                let char_x = title_x + i as u32 * 8;
                font.draw_char(fb, char_x, title_y, *byte, WHITE);
            }
        }

        // 绘制关闭按钮 X
        let close_x = self.x + self.width - 18;
        let close_y = self.y + 4;
        fb.fill_rect(close_x, close_y, 12, 12, RED);
        fb.draw_line(close_x + 2, close_y + 2, close_x + 10, close_y + 10, WHITE);
        fb.draw_line(close_x + 10, close_y + 2, close_x + 2, close_y + 10, WHITE);
    }
}

/// 窗口管理器
pub struct WindowManager {
    /// 窗口列表
    windows: BTreeMap<WindowId, Window>,
    /// 下一个窗口 ID
    next_window_id: WindowId,
    /// Z-order 计数器
    next_z_order: u32,
    /// 桌面背景颜色
    desktop_color: u32,
    /// 当前正在拖拽的窗口 ID
    dragging_window: Option<WindowId>,
    /// 拖拽时鼠标相对于窗口左上角的 X 偏移
    drag_offset_x: i32,
    /// 拖拽时鼠标相对于窗口左上角的 Y 偏移
    drag_offset_y: i32,
}

impl WindowManager {
    /// 创建新窗口管理器
    pub fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            next_window_id: 1,
            next_z_order: 1,
            desktop_color: color::BLUE,
            dragging_window: None,
            drag_offset_x: 0,
            drag_offset_y: 0,
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
        let mut windows: Vec<&Window> = self.windows.values().collect();
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
        for (_id, window) in self.windows.iter() {
            if window.contains(x, y) {
                return true;
            }
        }
        false
    }

    /// 获取窗口列表
    pub fn windows(&self) -> Vec<&Window> {
        self.windows.values().collect()
    }

    /// 设置桌面颜色
    pub fn set_desktop_color(&mut self, color: u32) {
        self.desktop_color = color;
    }

    /// 获取最上层的可见窗口（用于点击检测）
    fn get_top_window_at(&self, x: u32, y: u32) -> Option<WindowId> {
        let mut windows: Vec<&Window> = self.windows.values()
            .filter(|w| w.visible && w.contains(x, y))
            .collect();
        windows.sort_by_key(|w| w.z_order);

        windows.last().map(|w| w.id)
    }

    /// 处理鼠标按下事件
    pub fn handle_mouse_down(&mut self, x: u32, y: u32) -> Option<WindowId> {
        // 查找点击的窗口
        if let Some(window_id) = self.get_top_window_at(x, y) {
            // 将窗口置顶
            self.bring_to_front(window_id);

            // 检查是否点击了关闭按钮
            if let Some(window) = self.windows.get(&window_id) {
                if window.is_in_close_button(x, y) {
                    return Some(window_id); // 返回窗口 ID 表示点击了关闭
                }

                // 检查是否点击了标题栏（开始拖拽）
                if window.is_in_title_bar(x, y) {
                    self.dragging_window = Some(window_id);
                    self.drag_offset_x = x as i32 - window.x as i32;
                    self.drag_offset_y = y as i32 - window.y as i32;
                }
            }
        }
        None
    }

    /// 处理鼠标移动事件
    pub fn handle_mouse_move(&mut self, x: u32, y: u32) {
        if let Some(window_id) = self.dragging_window {
            // 计算新位置
            let new_x = (x as i32 - self.drag_offset_x).max(0) as u32;
            let new_y = (y as i32 - self.drag_offset_y).max(0) as u32;

            // 更新窗口位置
            if let Some(window) = self.windows.get_mut(&window_id) {
                window.x = new_x;
                window.y = new_y;
            }
        }
    }

    /// 处理鼠标释放事件
    pub fn handle_mouse_up(&mut self, _x: u32, _y: u32) {
        // 停止拖拽
        self.dragging_window = None;
    }

    /// 将窗口置顶
    pub fn bring_to_front(&mut self, id: WindowId) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.z_order = self.next_z_order;
            self.next_z_order += 1;
        }
    }

    /// 检查是否正在拖拽
    pub fn is_dragging(&self) -> bool {
        self.dragging_window.is_some()
    }

    /// 获取正在拖拽的窗口 ID
    pub fn get_dragging_window(&self) -> Option<WindowId> {
        self.dragging_window
    }
}

/// 全局窗口管理器
static mut WINDOW_MANAGER: Option<WindowManager> = None;

/// 初始化窗口管理器
pub fn init() {
    println!("wm: Initializing window manager...");
    unsafe {
        WINDOW_MANAGER = Some(WindowManager::new());
    }
    println!("wm: Window manager initialized [OK]");
}

/// 获取窗口管理器
pub fn get_manager() -> &'static mut WindowManager {
    unsafe {
        WINDOW_MANAGER.as_mut().expect("Window manager not initialized")
    }
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
    for (id, window) in wm.windows.iter() {
        if window.contains(x, y) {
            return Some(*id);
        }
    }
    None
}
