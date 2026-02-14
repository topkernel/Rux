//! 窗口管理器

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::string::String;
use crate::framebuffer::{Framebuffer, color};
use crate::font::FontRenderer;

/// 窗口 ID
pub type WindowId = u32;

/// 窗口状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
}

/// 标题栏高度
pub const TITLE_BAR_HEIGHT: u32 = 20;

/// 窗口
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub state: WindowState,
    pub visible: bool,
}

impl Window {
    pub fn new(id: WindowId, title: &str, x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            id,
            title: String::from(title),
            x,
            y,
            width,
            height,
            z_order: 0,
            state: WindowState::Normal,
            visible: true,
        }
    }

    pub fn contains(&self, px: u32, py: u32) -> bool {
        if !self.visible {
            return false;
        }
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }

    pub fn is_in_title_bar(&self, px: u32, py: u32) -> bool {
        if !self.visible {
            return false;
        }
        px >= self.x && px < self.x + self.width - 20 && py >= self.y && py < self.y + TITLE_BAR_HEIGHT
    }

    pub fn is_in_close_button(&self, px: u32, py: u32) -> bool {
        if !self.visible {
            return false;
        }
        let close_x = self.x + self.width - 18;
        let close_y = self.y + 4;
        px >= close_x && px < close_x + 12 && py >= close_y && py < close_y + 12
    }

    pub fn draw<F: Framebuffer>(&self, fb: &F, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        // 阴影
        fb.fill_rect(self.x + 4, self.y + 4, self.width, self.height, color::DARK_GRAY);
        // 背景
        fb.fill_rect(self.x, self.y, self.width, self.height, color::WHITE);
        // 边框
        fb.blit_rect(self.x, self.y, self.width, self.height, color::BLACK, 2);
        // 标题栏
        fb.fill_rect(self.x, self.y, self.width, TITLE_BAR_HEIGHT, color::BLUE);

        // 标题文本
        if self.width > 40 {
            let title_x = self.x + 6;
            let title_y = self.y + 6;
            let max_chars = ((self.width - 30) / 8) as usize;
            for (i, ch) in self.title.bytes().enumerate() {
                if i >= max_chars {
                    break;
                }
                font.draw_char(fb, title_x + i as u32 * 8, title_y, ch, color::WHITE);
            }
        }

        // 关闭按钮
        let close_x = self.x + self.width - 18;
        let close_y = self.y + 4;
        fb.fill_rect(close_x, close_y, 12, 12, color::RED);
        fb.draw_line(close_x + 2, close_y + 2, close_x + 10, close_y + 10, color::WHITE);
        fb.draw_line(close_x + 10, close_y + 2, close_x + 2, close_y + 10, color::WHITE);
    }
}

/// 窗口管理器
pub struct WindowManager {
    windows: BTreeMap<WindowId, Window>,
    next_id: WindowId,
    next_z_order: u32,
    dragging_window: Option<WindowId>,
    drag_offset_x: i32,
    drag_offset_y: i32,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            next_id: 1,
            next_z_order: 1,
            dragging_window: None,
            drag_offset_x: 0,
            drag_offset_y: 0,
        }
    }

    pub fn create_window(&mut self, title: &str, x: u32, y: u32, width: u32, height: u32) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;

        let mut window = Window::new(id, title, x, y, width, height);
        window.z_order = self.next_z_order;
        self.next_z_order += 1;

        self.windows.insert(id, window);
        id
    }

    pub fn remove_window(&mut self, id: WindowId) -> bool {
        self.windows.remove(&id).is_some()
    }

    pub fn get_window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn get_window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    pub fn windows(&self) -> Vec<&Window> {
        self.windows.values().collect()
    }

    pub fn bring_to_front(&mut self, id: WindowId) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.z_order = self.next_z_order;
            self.next_z_order += 1;
        }
    }

    fn get_top_window_at(&self, x: u32, y: u32) -> Option<WindowId> {
        let mut windows: Vec<&Window> = self.windows.values()
            .filter(|w| w.visible && w.contains(x, y))
            .collect();
        windows.sort_by_key(|w| w.z_order);
        windows.last().map(|w| w.id)
    }

    pub fn handle_mouse_down(&mut self, x: u32, y: u32) -> Option<WindowId> {
        if let Some(window_id) = self.get_top_window_at(x, y) {
            self.bring_to_front(window_id);

            if let Some(window) = self.windows.get(&window_id) {
                if window.is_in_close_button(x, y) {
                    return Some(window_id);
                }

                if window.is_in_title_bar(x, y) {
                    self.dragging_window = Some(window_id);
                    self.drag_offset_x = x as i32 - window.x as i32;
                    self.drag_offset_y = y as i32 - window.y as i32;
                }
            }
        }
        None
    }

    pub fn handle_mouse_move(&mut self, x: u32, y: u32) {
        if let Some(window_id) = self.dragging_window {
            let new_x = (x as i32 - self.drag_offset_x).max(0) as u32;
            let new_y = (y as i32 - self.drag_offset_y).max(0) as u32;

            if let Some(window) = self.windows.get_mut(&window_id) {
                window.x = new_x;
                window.y = new_y;
            }
        }
    }

    pub fn handle_mouse_up(&mut self) {
        self.dragging_window = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging_window.is_some()
    }

    pub fn draw_all<F: Framebuffer>(&self, fb: &F, font: &FontRenderer) {
        let mut windows: Vec<&Window> = self.windows.values().collect();
        windows.sort_by_key(|w| w.z_order);

        for window in windows {
            window.draw(fb, font);
        }
    }
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}
