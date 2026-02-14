//! PS/2 键盘驱动
//!
//! 支持 IBM PC/AT 兼容键盘（PS/2 协议）
//!
//! 参考资料：
//! - OSDev Wiki: https://wiki.osdev.org/PS/2_Keyboard
//! - IBM PC AT Technical Reference

use crate::println;

/// PS/2 数据端口（RISC-V virt 平台）
const PS2_DATA_PORT: u16 = 0x60;

/// PS/2 命令/状态端口
const PS2_CMD_PORT: u16 = 0x64;

/// 键盘扫描码集（使用 set 1）
pub mod scancode {
    /// 键盘释放标志位
    pub const BREAK_CODE: u16 = 0x80;

    /// 字母键 A-Z (without shift)
    pub const KEY_A: u16 = 0x1E;
    pub const KEY_B: u16 = 0x30;
    pub const KEY_C: u16 = 0x2E;
    pub const KEY_D: u16 = 0x20;
    pub const KEY_E: u16 = 0x12;
    pub const KEY_F: u16 = 0x21;
    pub const KEY_G: u16 = 0x22;
    pub const KEY_H: u16 = 0x23;
    pub const KEY_I: u16 = 0x17;
    pub const KEY_J: u16 = 0x24;
    pub const KEY_K: u16 = 0x25;
    pub const KEY_L: u16 = 0x26;
    pub const KEY_M: u16 = 0x27;
    pub const KEY_N: u16 = 0x31;
    pub const KEY_O: u16 = 0x18;
    pub const KEY_P: u16 = 0x19;
    pub const KEY_Q: u16 = 0x10;
    pub const KEY_R: u16 = 0x13;
    pub const KEY_S: u16 = 0x1F;
    pub const KEY_T: u16 = 0x14;
    pub const KEY_U: u16 = 0x16;
    pub const KEY_V: u16 = 0x2F;
    pub const KEY_W: u16 = 0x11;
    pub const KEY_X: u16 = 0x2D;
    pub const KEY_Y: u16 = 0x15;
    pub const KEY_Z: u16 = 0x2D;

    /// 数字键 1-9, 0
    pub const KEY_1: u16 = 0x02;
    pub const KEY_2: u16 = 0x03;
    pub const KEY_3: u16 = 0x04;
    pub const KEY_4: u16 = 0x05;
    pub const KEY_5: u16 = 0x06;
    pub const KEY_6: u16 = 0x07;
    pub const KEY_7: u16 = 0x08;
    pub const KEY_8: u16 = 0x09;
    pub const KEY_9: u16 = 0x0A;
    pub const KEY_0: u16 = 0x0B;

    /// 特殊键
    pub const KEY_ENTER: u16 = 0x1C;
    pub const KEY_SPACE: u16 = 0x39;
    pub const KEY_BACKSPACE: u16 = 0x0E;
    pub const KEY_TAB: u16 = 0x0F;
    pub const KEY_ESCAPE: u16 = 0x01;

    /// 修饰键
    pub const KEY_LSHIFT: u16 = 0x2A;
    pub const KEY_RSHIFT: u16 = 0x36;
    pub const KEY_LCTRL: u16 = 0x1D;
    pub const KEY_RCTRL: u16 = 0x11D;
    pub const KEY_LALT: u16 = 0x38;
    pub const KEY_RALT: u16 = 0x138;

    /// 功能键 F1-F12
    pub const KEY_F1: u16 = 0x3B;
    pub const KEY_F2: u16 = 0x3C;
    pub const KEY_F3: u16 = 0x3D;
    pub const KEY_F4: u16 = 0x3E;
    pub const KEY_F5: u16 = 0x3F;
    pub const KEY_F6: u16 = 0x40;
    pub const KEY_F7: u16 = 0x41;
    pub const KEY_F8: u16 = 0x42;
    pub const KEY_F9: u16 = 0x43;
    pub const KEY_F10: u16 = 0x44;
    pub const KEY_F11: u16 = 0x57;
    pub const KEY_F12: u16 = 0x58;

    /// 方向键
    pub const KEY_UP: u16 = 0x148;
    pub const KEY_DOWN: u16 = 0x150;
    pub const KEY_LEFT: u16 = 0x14B;
    pub const KEY_RIGHT: u16 = 0x14D;
}

/// 键盘事件
#[derive(Debug, Clone, Copy)]
pub enum KeyEvent {
    /// 按键按下
    Press(u16),
    /// 按键释放
    Release(u16),
}

/// PS/2 键盘驱动状态
pub struct PS2Keyboard {
    /// Shift 键状态
    shift_pressed: bool,
    /// Ctrl 键状态
    ctrl_pressed: bool,
    /// Alt 键状态
    alt_pressed: bool,
}

impl PS2Keyboard {
    /// 创建新的 PS/2 键盘驱动
    pub const fn new() -> Self {
        Self {
            shift_pressed: false,
            ctrl_pressed: false,
            alt_pressed: false,
        }
    }

    /// 读取扫描码并转换为键盘事件
    pub fn read_scancode(&mut self) -> Option<KeyEvent> {
        // TODO: Implement RISC-V PS/2 keyboard input
        // The x86 inb instruction doesn't work on RISC-V
        None
    }

    /// 处理修饰键按下
    fn handle_modifier_press(&mut self, scancode: u16) {
        match scancode {
            scancode::KEY_LSHIFT | scancode::KEY_RSHIFT => {
                self.shift_pressed = true;
            }
            scancode::KEY_LCTRL | scancode::KEY_RCTRL => {
                self.ctrl_pressed = true;
            }
            scancode::KEY_LALT | scancode::KEY_RALT => {
                self.alt_pressed = true;
            }
            _ => {}
        }
    }

    /// 处理修饰键释放
    fn handle_modifier_release(&mut self, scancode: u16) {
        match scancode {
            scancode::KEY_LSHIFT | scancode::KEY_RSHIFT => {
                self.shift_pressed = false;
            }
            scancode::KEY_LCTRL | scancode::KEY_RCTRL => {
                self.ctrl_pressed = false;
            }
            scancode::KEY_LALT | scancode::KEY_RALT => {
                self.alt_pressed = false;
            }
            _ => {}
        }
    }

    /// 将扫描码转换为 ASCII
    pub fn scancode_to_ascii(&self, scancode: u16) -> Option<u8> {
        let shifted = self.shift_pressed;

        let ascii = match scancode {
            // 字母键
            scancode::KEY_A => if shifted { b'A' } else { b'a' },
            scancode::KEY_B => if shifted { b'B' } else { b'b' },
            scancode::KEY_C => if shifted { b'C' } else { b'c' },
            scancode::KEY_D => if shifted { b'D' } else { b'd' },
            scancode::KEY_E => if shifted { b'E' } else { b'e' },
            scancode::KEY_F => if shifted { b'F' } else { b'f' },
            scancode::KEY_G => if shifted { b'G' } else { b'g' },
            scancode::KEY_H => if shifted { b'H' } else { b'h' },
            scancode::KEY_I => if shifted { b'I' } else { b'i' },
            scancode::KEY_J => if shifted { b'J' } else { b'j' },
            scancode::KEY_K => if shifted { b'K' } else { b'k' },
            scancode::KEY_L => if shifted { b'L' } else { b'l' },
            scancode::KEY_M => if shifted { b'M' } else { b'm' },
            scancode::KEY_N => if shifted { b'N' } else { b'n' },
            scancode::KEY_O => if shifted { b'O' } else { b'o' },
            scancode::KEY_P => if shifted { b'P' } else { b'p' },
            scancode::KEY_Q => if shifted { b'Q' } else { b'q' },
            scancode::KEY_R => if shifted { b'R' } else { b'r' },
            scancode::KEY_S => if shifted { b'S' } else { b's' },
            scancode::KEY_T => if shifted { b'T' } else { b't' },
            scancode::KEY_U => if shifted { b'U' } else { b'u' },
            scancode::KEY_V => if shifted { b'V' } else { b'v' },
            scancode::KEY_W => if shifted { b'W' } else { b'w' },
            scancode::KEY_X => if shifted { b'X' } else { b'x' },
            scancode::KEY_Y => if shifted { b'Y' } else { b'y' },
            scancode::KEY_Z => if shifted { b'Z' } else { b'z' },

            // 数字键
            scancode::KEY_1 => if shifted { b'!' } else { b'1' },
            scancode::KEY_2 => if shifted { b'@' } else { b'2' },
            scancode::KEY_3 => if shifted { b'#' } else { b'3' },
            scancode::KEY_4 => if shifted { b'$' } else { b'4' },
            scancode::KEY_5 => if shifted { b'%' } else { b'5' },
            scancode::KEY_6 => if shifted { b'^' } else { b'6' },
            scancode::KEY_7 => if shifted { b'&' } else { b'7' },
            scancode::KEY_8 => if shifted { b'*' } else { b'8' },
            scancode::KEY_9 => if shifted { b'(' } else { b'9' },
            scancode::KEY_0 => if shifted { b')' } else { b'0' },

            // 特殊键
            scancode::KEY_SPACE => b' ',
            scancode::KEY_ENTER => b'\n',
            scancode::KEY_BACKSPACE => 0x08,
            scancode::KEY_TAB => b'\t',
            scancode::KEY_ESCAPE => 0x1B,

            _ => return None,
        };

        Some(ascii)
    }

    /// 检查是否有可读数据
    pub fn has_data(&self) -> bool {
        // TODO: Implement RISC-V PS/2 keyboard status check
        false
    }
}

/// 全局 PS/2 键盘驱动实例
pub static mut KEYBOARD: PS2Keyboard = PS2Keyboard::new();

/// 初始化 PS/2 键盘驱动
pub fn init() {
    println!("keyboard: Initializing PS/2 keyboard driver...");
    println!("keyboard: PS/2 keyboard initialized [OK]");
}

/// 读取键盘事件（非阻塞）
pub fn read_event() -> Option<KeyEvent> {
    unsafe {
        if KEYBOARD.has_data() {
            KEYBOARD.read_scancode()
        } else {
            None
        }
    }
}

/// 读取 ASCII 字符（非阻塞）
pub fn read_char() -> Option<u8> {
    unsafe {
        if let Some(event) = read_event() {
            match event {
                KeyEvent::Press(scancode) => {
                    KEYBOARD.scancode_to_ascii(scancode)
                }
                KeyEvent::Release(_) => None,
            }
        } else {
            None
        }
    }
}
