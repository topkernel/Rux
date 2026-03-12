//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PS/2 keyboard and mouse driver
//!
//! Note: PS/2 ports (0x60/0x64) are not available on RISC-V virt platform
//! This driver is kept as a framework only, actual input should use VirtIO Input

use super::event::*;

// ============================================================================
// PS/2 port definitions (x86 only)
// ============================================================================

/// PS/2 data port
const PS2_DATA_PORT: u16 = 0x60;
/// PS/2 command/status port
const PS2_CMD_PORT: u16 = 0x64;

// ============================================================================
// PS/2 keyboard scancodes (Set 1)
// ============================================================================

pub mod scancode {
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
    pub const KEY_Z: u16 = 0x2C;

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

    pub const KEY_ENTER: u16 = 0x1C;
    pub const KEY_SPACE: u16 = 0x39;
    pub const KEY_BACKSPACE: u16 = 0x0E;
    pub const KEY_TAB: u16 = 0x0F;
    pub const KEY_ESCAPE: u16 = 0x01;

    pub const KEY_LSHIFT: u16 = 0x2A;
    pub const KEY_RSHIFT: u16 = 0x36;
    pub const KEY_LCTRL: u16 = 0x1D;
    pub const KEY_RCTRL: u16 = 0x11D;
    pub const KEY_LALT: u16 = 0x38;
    pub const KEY_RALT: u16 = 0x138;

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

    /// Release flag
    pub const BREAK_CODE: u16 = 0x80;
}

// ============================================================================
// PS/2 keyboard driver
// ============================================================================

/// PS/2 keyboard state
pub struct PS2Keyboard {
    shift_pressed: bool,
    ctrl_pressed: bool,
    alt_pressed: bool,
}

impl PS2Keyboard {
    pub const fn new() -> Self {
        Self {
            shift_pressed: false,
            ctrl_pressed: false,
            alt_pressed: false,
        }
    }

    /// Read scancode (not available on RISC-V)
    pub fn read_scancode(&mut self) -> Option<InputEvent> {
        // PS/2 ports not available on RISC-V virt platform
        None
    }

    /// Check if data is available (always returns false on RISC-V)
    pub fn has_data(&self) -> bool {
        false
    }

    /// Convert scancode to ASCII
    pub fn scancode_to_ascii(&self, scancode: u16) -> Option<u8> {
        let shifted = self.shift_pressed;

        let ascii = match scancode {
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

            scancode::KEY_SPACE => b' ',
            scancode::KEY_ENTER => b'\n',
            scancode::KEY_BACKSPACE => 0x08,
            scancode::KEY_TAB => b'\t',
            scancode::KEY_ESCAPE => 0x1B,
            _ => return None,
        };

        Some(ascii)
    }
}

/// Global PS/2 keyboard instance
pub static mut PS2_KEYBOARD: PS2Keyboard = PS2Keyboard::new();

// ============================================================================
// PS/2 mouse driver
// ============================================================================

/// Mouse packet flags
pub mod mouse_flags {
    pub const LEFT_BUTTON: u8 = 0x01;
    pub const RIGHT_BUTTON: u8 = 0x02;
    pub const MIDDLE_BUTTON: u8 = 0x04;
    pub const ALWAYS_SET: u8 = 0x08;
    pub const X_SIGN: u8 = 0x10;
    pub const Y_SIGN: u8 = 0x20;
    pub const X_OVERFLOW: u8 = 0x40;
    pub const Y_OVERFLOW: u8 = 0x80;
}

/// PS/2 mouse state
pub struct PS2Mouse {
    packet_index: u8,
    packet: [u8; 3],
    x: i32,
    y: i32,
    left_pressed: bool,
    right_pressed: bool,
    middle_pressed: bool,
}

impl PS2Mouse {
    pub const fn new() -> Self {
        Self {
            packet_index: 0,
            packet: [0; 3],
            x: 0,
            y: 0,
            left_pressed: false,
            right_pressed: false,
            middle_pressed: false,
        }
    }

    /// Read mouse event (not available on RISC-V)
    pub fn read_event(&mut self) -> Option<InputEvent> {
        // PS/2 ports not available on RISC-V virt platform
        None
    }

    /// Check if data is available (always returns false on RISC-V)
    pub fn has_data(&self) -> bool {
        false
    }

    /// Get X position
    pub fn x(&self) -> i32 {
        self.x
    }

    /// Get Y position
    pub fn y(&self) -> i32 {
        self.y
    }
}

/// Global PS/2 mouse instance
pub static mut PS2_MOUSE: PS2Mouse = PS2Mouse::new();

// ============================================================================
// Initialization functions
// ============================================================================

/// Initialize PS/2 keyboard driver
pub fn init_keyboard() {
    // PS/2 keyboard not available on RISC-V
}

/// Initialize PS/2 mouse driver
pub fn init_mouse() {
    // PS/2 mouse not available on RISC-V
}
