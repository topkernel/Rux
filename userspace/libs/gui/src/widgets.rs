//! UI 控件

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::framebuffer::{Framebuffer, color};
use crate::font::FontRenderer;

/// 控件 ID
pub type WidgetId = u32;

/// 控件事件
#[derive(Debug, Clone, Copy)]
pub enum WidgetEvent {
    Click { x: u32, y: u32 },
    MouseDown { x: u32, y: u32 },
    MouseUp { x: u32, y: u32 },
    MouseMove { x: u32, y: u32 },
    KeyPress { key: u8 },
    Focus,
    Blur,
}

/// 控件状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetState {
    Normal,
    Hover,
    Pressed,
    Disabled,
    Focused,
}

/// 按钮
pub struct Button {
    pub id: WidgetId,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub text: String,
    pub state: WidgetState,
    pub visible: bool,
    pub enabled: bool,
    pub clicked: bool,
}

impl Button {
    pub fn new(id: WidgetId, x: u32, y: u32, width: u32, height: u32, text: &str) -> Self {
        Self {
            id, x, y, width, height,
            text: String::from(text),
            state: WidgetState::Normal,
            visible: true,
            enabled: true,
            clicked: false,
        }
    }

    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }

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

    pub fn draw<F: Framebuffer>(&self, fb: &F, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        let bg = match self.state {
            WidgetState::Normal => color::GRAY,
            WidgetState::Hover => 0xFFA0A0A0,
            WidgetState::Pressed => 0xFF606060,
            WidgetState::Disabled => 0xFF404040,
            WidgetState::Focused => 0xFFA0A0A0,
        };

        fb.fill_rect(self.x, self.y, self.width, self.height, bg);
        fb.blit_rect(self.x, self.y, self.width, self.height, color::BLACK, 1);

        let text_width = font.measure_text(&self.text);
        let text_x = self.x + (self.width.saturating_sub(text_width)) / 2;
        let text_y = self.y + (self.height.saturating_sub(font.height())) / 2;

        font.draw_string(fb, text_x, text_y, &self.text, color::WHITE);
    }

    pub fn was_clicked(&mut self) -> bool {
        let c = self.clicked;
        self.clicked = false;
        c
    }
}

/// 标签
pub struct Label {
    pub id: WidgetId,
    pub x: u32,
    pub y: u32,
    pub text: String,
    pub visible: bool,
    pub text_color: u32,
}

impl Label {
    pub fn new(id: WidgetId, x: u32, y: u32, text: &str) -> Self {
        Self {
            id, x, y,
            text: String::from(text),
            visible: true,
            text_color: color::WHITE,
        }
    }

    pub fn draw<F: Framebuffer>(&self, fb: &F, font: &FontRenderer) {
        if !self.visible {
            return;
        }
        font.draw_string(fb, self.x, self.y, &self.text, self.text_color);
    }
}

/// 文本框
pub struct TextBox {
    pub id: WidgetId,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub text: String,
    pub state: WidgetState,
    pub visible: bool,
    pub cursor_pos: usize,
}

impl TextBox {
    pub fn new(id: WidgetId, x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            id, x, y, width, height,
            text: String::new(),
            state: WidgetState::Normal,
            visible: true,
            cursor_pos: 0,
        }
    }

    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }

    pub fn handle_event(&mut self, event: WidgetEvent) -> bool {
        match event {
            WidgetEvent::MouseDown { .. } => {
                self.state = WidgetState::Focused;
                true
            }
            WidgetEvent::KeyPress { key } if self.state == WidgetState::Focused => {
                if key == b'\x08' {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                        self.text.remove(self.cursor_pos);
                    }
                } else if key >= 0x20 && key <= 0x7E {
                    self.text.insert(self.cursor_pos, key as char);
                    self.cursor_pos += 1;
                }
                true
            }
            _ => false,
        }
    }

    pub fn draw<F: Framebuffer>(&self, fb: &F, font: &FontRenderer) {
        if !self.visible {
            return;
        }

        fb.fill_rect(self.x, self.y, self.width, self.height, color::WHITE);
        let border = if self.state == WidgetState::Focused { color::BLUE } else { color::BLACK };
        fb.blit_rect(self.x, self.y, self.width, self.height, border, 1);

        let text_x = self.x + 4;
        let text_y = self.y + (self.height.saturating_sub(font.height())) / 2;

        font.draw_string(fb, text_x, text_y, &self.text, color::BLACK);

        if self.state == WidgetState::Focused {
            let cursor_x = text_x + (self.cursor_pos as u32 * 8);
            fb.draw_line_v(cursor_x, text_y, font.height(), color::BLACK);
        }
    }
}

/// 简单面板
pub struct SimplePanel {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
    pub buttons: Vec<Button>,
    pub labels: Vec<Label>,
    pub textboxes: Vec<TextBox>,
    next_id: WidgetId,
}

impl SimplePanel {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x, y, width, height,
            visible: true,
            buttons: Vec::new(),
            labels: Vec::new(),
            textboxes: Vec::new(),
            next_id: 1,
        }
    }

    pub fn add_button(&mut self, bx: u32, by: u32, bw: u32, bh: u32, text: &str) -> WidgetId {
        let id = self.next_id;
        self.next_id += 1;
        self.buttons.push(Button::new(id, self.x + bx, self.y + by, bw, bh, text));
        id
    }

    pub fn add_label(&mut self, lx: u32, ly: u32, text: &str) -> WidgetId {
        let id = self.next_id;
        self.next_id += 1;
        self.labels.push(Label::new(id, self.x + lx, self.y + ly, text));
        id
    }

    pub fn add_textbox(&mut self, tx: u32, ty: u32, tw: u32, th: u32) -> WidgetId {
        let id = self.next_id;
        self.next_id += 1;
        self.textboxes.push(TextBox::new(id, self.x + tx, self.y + ty, tw, th));
        id
    }

    pub fn draw<F: Framebuffer>(&self, fb: &F, font: &FontRenderer) {
        if !self.visible {
            return;
        }

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

    pub fn handle_mouse(&mut self, event: WidgetEvent) {
        for button in &mut self.buttons {
            button.handle_event(event);
        }
        for textbox in &mut self.textboxes {
            textbox.handle_event(event);
        }
    }
}
