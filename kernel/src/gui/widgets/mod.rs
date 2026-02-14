//! UI 控件库
//!
//! 提供基础的 GUI 控件：Button, Label, TextBox

extern crate alloc;
use alloc::string::String;
use crate::drivers::gpu::framebuffer::{FrameBuffer, color};
use crate::graphics::font::FontRenderer;

/// 控件 ID 类型
pub type WidgetId = u32;

/// 控件事件
#[derive(Debug, Clone, Copy)]
pub enum WidgetEvent {
    /// 鼠标点击
    Click { x: u32, y: u32 },
    /// 鼠标按下
    MouseDown { x: u32, y: u32 },
    /// 鼠标释放
    MouseUp { x: u32, y: u32 },
    /// 鼠标移动
    MouseMove { x: u32, y: u32 },
    /// 键盘按键
    KeyPress { key: u8 },
    /// 键盘释放
    KeyRelease { key: u8 },
    /// 获得焦点
    Focus,
    /// 失去焦点
    Blur,
}

/// 控件状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetState {
    /// 正常
    Normal,
    /// 悬停
    Hover,
    /// 按下
    Pressed,
    /// 禁用
    Disabled,
    /// 聚焦
    Focused,
}

/// 按钮控件
pub struct Button {
    /// 控件 ID
    pub id: WidgetId,
    /// X 坐标
    pub x: u32,
    /// Y 坐标
    pub y: u32,
    /// 宽度
    pub width: u32,
    /// 高度
    pub height: u32,
    /// 按钮文本
    pub text: String,
    /// 控件状态
    pub state: WidgetState,
    /// 是否可见
    pub visible: bool,
    /// 是否启用
    pub enabled: bool,
    /// 背景颜色
    pub bg_color: u32,
    /// 文字颜色
    pub text_color: u32,
    /// 点击回调标志
    pub clicked: bool,
}

impl Button {
    /// 创建新按钮
    pub fn new(id: WidgetId, x: u32, y: u32, width: u32, height: u32, text: &str) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height,
            text: String::from(text),
            state: WidgetState::Normal,
            visible: true,
            enabled: true,
            bg_color: color::GRAY,
            text_color: color::WHITE,
            clicked: false,
        }
    }

    /// 检查点是否在按钮内
    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }

    /// 处理事件
    pub fn handle_event(&mut self, event: WidgetEvent) -> bool {
        if !self.enabled || !self.visible {
            return false;
        }

        match event {
            WidgetEvent::MouseDown { .. } => {
                self.state = WidgetState::Pressed;
                true
            }
            WidgetEvent::MouseUp { .. } => {
                if self.state == WidgetState::Pressed {
                    self.clicked = true;
                    self.state = WidgetState::Hover;
                }
                true
            }
            WidgetEvent::MouseMove { x, y } => {
                if self.contains(x, y) {
                    if self.state != WidgetState::Pressed {
                        self.state = WidgetState::Hover;
                    }
                } else {
                    self.state = WidgetState::Normal;
                }
                true
            }
            _ => false,
        }
    }

    /// 绘制按钮
    pub fn draw(&self, fb: &FrameBuffer, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        // 根据状态选择颜色
        let bg = match self.state {
            WidgetState::Normal => self.bg_color,
            WidgetState::Hover => 0xFFA0A0A0,  // 浅灰色
            WidgetState::Pressed => 0xFF606060, // 深灰色
            WidgetState::Disabled => 0xFF404040,
            WidgetState::Focused => 0xFFA0A0A0,
        };

        // 绘制按钮背景
        fb.fill_rect(self.x, self.y, self.width, self.height, bg);

        // 绘制边框
        let border_color = if self.state == WidgetState::Focused {
            color::WHITE
        } else {
            color::BLACK
        };
        fb.blit_rect(self.x, self.y, self.width, self.height, border_color, 1);

        // 绘制文本（居中）
        let text_width = font.measure_text(&self.text);
        let text_x = self.x + (self.width.saturating_sub(text_width)) / 2;
        let text_y = self.y + (self.height.saturating_sub(font.height())) / 2;

        font.draw_string(fb, text_x, text_y, &self.text, self.text_color);
    }

    /// 检查是否被点击（并清除标志）
    pub fn was_clicked(&mut self) -> bool {
        let clicked = self.clicked;
        self.clicked = false;
        clicked
    }
}

/// 标签控件
pub struct Label {
    /// 控件 ID
    pub id: WidgetId,
    /// X 坐标
    pub x: u32,
    /// Y 坐标
    pub y: u32,
    /// 宽度
    pub width: u32,
    /// 高度
    pub height: u32,
    /// 标签文本
    pub text: String,
    /// 控件状态
    pub state: WidgetState,
    /// 是否可见
    pub visible: bool,
    /// 是否启用
    pub enabled: bool,
    /// 文字颜色
    pub text_color: u32,
    /// 背景颜色（透明则为 None）
    pub bg_color: Option<u32>,
}

impl Label {
    /// 创建新标签
    pub fn new(id: WidgetId, x: u32, y: u32, text: &str) -> Self {
        let text_width = text.len() as u32 * 8;
        Self {
            id,
            x,
            y,
            width: text_width,
            height: 16,
            text: String::from(text),
            state: WidgetState::Normal,
            visible: true,
            enabled: true,
            text_color: color::WHITE,
            bg_color: None,
        }
    }

    /// 绘制标签
    pub fn draw(&self, fb: &FrameBuffer, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        // 绘制背景（如果有）
        if let Some(bg) = self.bg_color {
            fb.fill_rect(self.x, self.y, self.width, self.height, bg);
        }

        // 绘制文本
        font.draw_string(fb, self.x, self.y, &self.text, self.text_color);
    }
}

/// 文本框控件
pub struct TextBox {
    /// 控件 ID
    pub id: WidgetId,
    /// X 坐标
    pub x: u32,
    /// Y 坐标
    pub y: u32,
    /// 宽度
    pub width: u32,
    /// 高度
    pub height: u32,
    /// 文本内容
    pub text: String,
    /// 控件状态
    pub state: WidgetState,
    /// 是否可见
    pub visible: bool,
    /// 是否启用
    pub enabled: bool,
    /// 背景颜色
    pub bg_color: u32,
    /// 文字颜色
    pub text_color: u32,
    /// 光标位置
    pub cursor_pos: usize,
    /// 最大长度
    pub max_length: usize,
}

impl TextBox {
    /// 创建新文本框
    pub fn new(id: WidgetId, x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height,
            text: String::new(),
            state: WidgetState::Normal,
            visible: true,
            enabled: true,
            bg_color: color::WHITE,
            text_color: color::BLACK,
            cursor_pos: 0,
            max_length: 256,
        }
    }

    /// 检查点是否在文本框内
    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }

    /// 处理事件
    pub fn handle_event(&mut self, event: WidgetEvent) -> bool {
        if !self.enabled || !self.visible {
            return false;
        }

        match event {
            WidgetEvent::MouseDown { .. } => {
                self.state = WidgetState::Focused;
                true
            }
            WidgetEvent::Blur => {
                self.state = WidgetState::Normal;
                true
            }
            WidgetEvent::KeyPress { key } => {
                if self.state == WidgetState::Focused {
                    if key == b'\x08' {
                        // Backspace
                        self.backspace();
                    } else if key == b'\x7F' {
                        // Delete
                        self.delete();
                    } else if key >= 0x20 && key <= 0x7E {
                        // 可打印 ASCII
                        self.insert_char(key as char);
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// 绘制文本框
    pub fn draw(&self, fb: &FrameBuffer, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        // 绘制背景
        fb.fill_rect(self.x, self.y, self.width, self.height, self.bg_color);

        // 绘制边框
        let border_color = if self.state == WidgetState::Focused {
            color::BLUE
        } else {
            color::BLACK
        };
        fb.blit_rect(self.x, self.y, self.width, self.height, border_color, 1);

        // 计算文本显示区域
        let text_area_x = self.x + 4;
        let text_area_width = self.width.saturating_sub(8);
        let text_y = self.y + (self.height.saturating_sub(font.height())) / 2;

        // 绘制文本
        let mut char_x = text_area_x;
        for ch in self.text.bytes() {
            if char_x + 8 > text_area_x + text_area_width {
                break;
            }
            font.draw_char(fb, char_x, text_y, ch, self.text_color);
            char_x += 8;
        }

        // 绘制光标（如果聚焦）
        if self.state == WidgetState::Focused {
            let cursor_x = text_area_x + (self.cursor_pos as u32 * 8).min(text_area_width);
            fb.draw_line_v(cursor_x, text_y, font.height(), color::BLACK);
        }
    }

    /// 插入字符
    pub fn insert_char(&mut self, ch: char) {
        if self.text.len() < self.max_length && ch.is_ascii() {
            self.text.insert(self.cursor_pos, ch);
            self.cursor_pos += 1;
        }
    }

    /// 删除字符（退格）
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.text.remove(self.cursor_pos);
        }
    }

    /// 删除字符（Delete）
    pub fn delete(&mut self) {
        if self.cursor_pos < self.text.len() {
            self.text.remove(self.cursor_pos);
        }
    }

    /// 清空文本
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor_pos = 0;
    }
}

/// 简单面板（不使用 trait object）
pub struct SimplePanel {
    /// 面板 X 坐标
    pub x: u32,
    /// 面板 Y 坐标
    pub y: u32,
    /// 面板宽度
    pub width: u32,
    /// 面板高度
    pub height: u32,
    /// 背景颜色
    pub bg_color: u32,
    /// 边框颜色
    pub border_color: Option<u32>,
    /// 是否可见
    pub visible: bool,
    /// 按钮列表
    pub buttons: alloc::vec::Vec<Button>,
    /// 标签列表
    pub labels: alloc::vec::Vec<Label>,
    /// 文本框列表
    pub textboxes: alloc::vec::Vec<TextBox>,
    /// 下一个控件 ID
    next_id: WidgetId,
}

impl SimplePanel {
    /// 创建新面板
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            bg_color: 0xFF202020,
            border_color: Some(color::GRAY),
            visible: true,
            buttons: alloc::vec::Vec::new(),
            labels: alloc::vec::Vec::new(),
            textboxes: alloc::vec::Vec::new(),
            next_id: 1,
        }
    }

    /// 添加按钮
    pub fn add_button(&mut self, bx: u32, by: u32, bw: u32, bh: u32, text: &str) -> WidgetId {
        let id = self.next_id;
        self.next_id += 1;
        self.buttons.push(Button::new(id, self.x + bx, self.y + by, bw, bh, text));
        id
    }

    /// 添加标签
    pub fn add_label(&mut self, lx: u32, ly: u32, text: &str) -> WidgetId {
        let id = self.next_id;
        self.next_id += 1;
        self.labels.push(Label::new(id, self.x + lx, self.y + ly, text));
        id
    }

    /// 添加文本框
    pub fn add_textbox(&mut self, tx: u32, ty: u32, tw: u32, th: u32) -> WidgetId {
        let id = self.next_id;
        self.next_id += 1;
        self.textboxes.push(TextBox::new(id, self.x + tx, self.y + ty, tw, th));
        id
    }

    /// 处理鼠标事件
    pub fn handle_mouse(&mut self, event: WidgetEvent) -> bool {
        // 处理按钮
        for button in &mut self.buttons {
            if button.handle_event(event) {
                return true;
            }
        }
        // 处理文本框
        for textbox in &mut self.textboxes {
            if textbox.handle_event(event) {
                return true;
            }
        }
        false
    }

    /// 处理键盘事件
    pub fn handle_keyboard(&mut self, event: WidgetEvent) -> bool {
        for textbox in &mut self.textboxes {
            if textbox.state == WidgetState::Focused {
                return textbox.handle_event(event);
            }
        }
        false
    }

    /// 绘制面板
    pub fn draw(&self, fb: &FrameBuffer, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        // 绘制背景
        fb.fill_rect(self.x, self.y, self.width, self.height, self.bg_color);

        // 绘制边框
        if let Some(border) = self.border_color {
            fb.blit_rect(self.x, self.y, self.width, self.height, border, 1);
        }

        // 绘制所有控件
        for label in &self.labels {
            label.draw(fb, font);
        }
        for textbox in &self.textboxes {
            textbox.draw(fb, font);
        }
        for button in &self.buttons {
            button.draw(fb, font);
        }
    }

    /// 检查按钮是否被点击
    pub fn check_button_click(&mut self, id: WidgetId) -> bool {
        for button in &mut self.buttons {
            if button.id == id {
                return button.was_clicked();
            }
        }
        false
    }

    /// 获取文本框内容
    pub fn get_textbox_text(&self, id: WidgetId) -> Option<&str> {
        for textbox in &self.textboxes {
            if textbox.id == id {
                return Some(&textbox.text);
            }
        }
        None
    }
}
