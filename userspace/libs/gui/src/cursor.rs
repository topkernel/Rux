//! 鼠标光标

/// 默认箭头光标 (16x16)
const ARROW_CURSOR: [u16; 16] = [
    0b0000000000000001,
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
    0b0000000000000111,
    0b0000000000000111,
    0b0000000000000011,
    0b0000000000000011,
    0b0000000000000001,
];

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

/// 光标颜色
pub mod cursor_color {
    pub const BLACK: u32 = 0xFF000000;
    pub const WHITE: u32 = 0xFFFFFFFF;
}

/// 鼠标光标
pub struct MouseCursor {
    pub x: i32,
    pub y: i32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub visible: bool,
}

impl MouseCursor {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            x: (screen_width / 2) as i32,
            y: (screen_height / 2) as i32,
            screen_width,
            screen_height,
            visible: true,
        }
    }

    pub fn move_by(&mut self, dx: i16, dy: i16) {
        self.x = (self.x + dx as i32).clamp(0, (self.screen_width - 1) as i32);
        self.y = (self.y + dy as i32).clamp(0, (self.screen_height - 1) as i32);
    }

    pub fn set_position(&mut self, x: i32, y: i32) {
        self.x = x.clamp(0, (self.screen_width - 1) as i32);
        self.y = y.clamp(0, (self.screen_height - 1) as i32);
    }

    pub fn draw<F: crate::framebuffer::Framebuffer>(&self, fb: &F) {
        if !self.visible {
            return;
        }

        let cursor_x = self.x as u32;
        let cursor_y = self.y as u32;

        for py in 0..16u32 {
            for px in 0..16u32 {
                let screen_x = cursor_x + px;
                let screen_y = cursor_y + py;

                if screen_x >= self.screen_width || screen_y >= self.screen_height {
                    continue;
                }

                let mask_bit = (ARROW_MASK[py as usize] >> (15 - px)) & 1;
                let cursor_bit = (ARROW_CURSOR[py as usize] >> (15 - px)) & 1;

                if mask_bit != 0 {
                    let color = if cursor_bit != 0 {
                        cursor_color::BLACK
                    } else {
                        cursor_color::WHITE
                    };
                    fb.put_pixel(screen_x, screen_y, color);
                }
            }
        }
    }
}
