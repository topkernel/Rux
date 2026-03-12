//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Input event definitions
//!
//! evdev compatible input event interface

// ============================================================================
// Event type constants (input.h)
// ============================================================================

/// Key event
pub const EV_KEY: u16 = 0x01;
/// Relative coordinate event (mouse movement)
pub const EV_REL: u16 = 0x02;
/// Absolute coordinate event (touchscreen, tablet)
pub const EV_ABS: u16 = 0x03;
/// Sync event
pub const EV_SYN: u16 = 0x00;
/// Miscellaneous event
pub const EV_MSC: u16 = 0x04;

// ============================================================================
// Relative coordinate axis codes (EV_REL)
// ============================================================================

/// X-axis relative movement
pub const REL_X: u16 = 0x00;
/// Y-axis relative movement
pub const REL_Y: u16 = 0x01;
/// Wheel
pub const REL_WHEEL: u16 = 0x08;

// ============================================================================
// Absolute coordinate axis codes (EV_ABS)
// ============================================================================

/// X-axis absolute position
pub const ABS_X: u16 = 0x00;
/// Y-axis absolute position
pub const ABS_Y: u16 = 0x01;
/// Multi-touch slot
pub const ABS_MT_SLOT: u16 = 0x2f;
/// Multi-touch X
pub const ABS_MT_POSITION_X: u16 = 0x35;
/// Multi-touch Y
pub const ABS_MT_POSITION_Y: u16 = 0x36;
/// Multi-touch tracking ID
pub const ABS_MT_TRACKING_ID: u16 = 0x39;

// ============================================================================
// Button codes (EV_KEY) - mouse
// ============================================================================

/// Mouse left button
pub const BTN_LEFT: u16 = 0x110;
/// Mouse right button
pub const BTN_RIGHT: u16 = 0x111;
/// Mouse middle button
pub const BTN_MIDDLE: u16 = 0x112;
/// Mouse side button
pub const BTN_SIDE: u16 = 0x113;
/// Mouse extra button
pub const BTN_EXTRA: u16 = 0x114;

// ============================================================================
// Button codes (EV_KEY) - keyboard
// ============================================================================

/// Keyboard key base (KEY_0 = 11)
pub const KEY_ESC: u16 = 0x01;
pub const KEY_1: u16 = 0x02;
pub const KEY_2: u16 = 0x03;
pub const KEY_3: u16 = 0x04;
pub const KEY_4: u16 = 0x05;
pub const KEY_5: u16 = 0x06;
pub const KEY_6: u16 = 0x07;
pub const KEY_7: u16 = 0x08;
pub const KEY_8: u16 = 0x09;
pub const KEY_9: u16 = 0x0a;
pub const KEY_0: u16 = 0x0b;
pub const KEY_MINUS: u16 = 0x0c;
pub const KEY_EQUAL: u16 = 0x0d;
pub const KEY_BACKSPACE: u16 = 0x0e;
pub const KEY_TAB: u16 = 0x0f;
pub const KEY_Q: u16 = 0x10;
pub const KEY_W: u16 = 0x11;
pub const KEY_E: u16 = 0x12;
pub const KEY_R: u16 = 0x13;
pub const KEY_T: u16 = 0x14;
pub const KEY_Y: u16 = 0x15;
pub const KEY_U: u16 = 0x16;
pub const KEY_I: u16 = 0x17;
pub const KEY_O: u16 = 0x18;
pub const KEY_P: u16 = 0x19;
pub const KEY_LEFTBRACE: u16 = 0x1a;
pub const KEY_RIGHTBRACE: u16 = 0x1b;
pub const KEY_ENTER: u16 = 0x1c;
pub const KEY_LEFTCTRL: u16 = 0x1d;
pub const KEY_A: u16 = 0x1e;
pub const KEY_S: u16 = 0x1f;
pub const KEY_D: u16 = 0x20;
pub const KEY_F: u16 = 0x21;
pub const KEY_G: u16 = 0x22;
pub const KEY_H: u16 = 0x23;
pub const KEY_J: u16 = 0x24;
pub const KEY_K: u16 = 0x25;
pub const KEY_L: u16 = 0x26;
pub const KEY_SEMICOLON: u16 = 0x27;
pub const KEY_APOSTROPHE: u16 = 0x28;
pub const KEY_GRAVE: u16 = 0x29;
pub const KEY_LEFTSHIFT: u16 = 0x2a;
pub const KEY_BACKSLASH: u16 = 0x2b;
pub const KEY_Z: u16 = 0x2c;
pub const KEY_X: u16 = 0x2d;
pub const KEY_C: u16 = 0x2e;
pub const KEY_V: u16 = 0x2f;
pub const KEY_B: u16 = 0x30;
pub const KEY_N: u16 = 0x31;
pub const KEY_M: u16 = 0x32;
pub const KEY_COMMA: u16 = 0x33;
pub const KEY_DOT: u16 = 0x34;
pub const KEY_SLASH: u16 = 0x35;
pub const KEY_RIGHTSHIFT: u16 = 0x36;
pub const KEY_KPASTERISK: u16 = 0x37;
pub const KEY_LEFTALT: u16 = 0x38;
pub const KEY_SPACE: u16 = 0x39;
pub const KEY_CAPSLOCK: u16 = 0x3a;

/// Function keys F1-F12
pub const KEY_F1: u16 = 0x3b;
pub const KEY_F2: u16 = 0x3c;
pub const KEY_F3: u16 = 0x3d;
pub const KEY_F4: u16 = 0x3e;
pub const KEY_F5: u16 = 0x3f;
pub const KEY_F6: u16 = 0x40;
pub const KEY_F7: u16 = 0x41;
pub const KEY_F8: u16 = 0x42;
pub const KEY_F9: u16 = 0x43;
pub const KEY_F10: u16 = 0x44;
pub const KEY_F11: u16 = 0x57;
pub const KEY_F12: u16 = 0x58;

/// Arrow keys
pub const KEY_UP: u16 = 0x67;
pub const KEY_DOWN: u16 = 0x6c;
pub const KEY_LEFT: u16 = 0x69;
pub const KEY_RIGHT: u16 = 0x6a;

/// Right modifier keys
pub const KEY_RIGHTCTRL: u16 = 0x61;
pub const KEY_RIGHTALT: u16 = 0x64;

// ============================================================================
// Key values
// ============================================================================

/// Key released
pub const KEY_RELEASE: i32 = 0;
/// Key pressed
pub const KEY_PRESS: i32 = 1;
/// Key repeat
pub const KEY_REPEAT: i32 = 2;

// ============================================================================
// Input event structure (input_event)
// ============================================================================

/// Raw input event (evdev compatible, 24 bytes)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct InputEvent {
    /// Timestamp (seconds)
    pub tv_sec: u64,
    /// Timestamp (microseconds)
    pub tv_usec: u64,
    /// Event type (EV_KEY, EV_REL, EV_ABS)
    pub type_: u16,
    /// Event code (keycode/axis)
    pub code: u16,
    /// Event value (press/release/coordinate value)
    pub value: i32,
}

impl InputEvent {
    /// Create new input event
    pub fn new(type_: u16, code: u16, value: i32) -> Self {
        Self {
            tv_sec: 0,
            tv_usec: 0,
            type_,
            code,
            value,
        }
    }

    /// Create keyboard event
    pub fn key_event(code: u16, pressed: bool) -> Self {
        Self::new(EV_KEY, code, if pressed { KEY_PRESS } else { KEY_RELEASE })
    }

    /// Create relative motion event
    pub fn rel_event(axis: u16, value: i32) -> Self {
        Self::new(EV_REL, axis, value)
    }

    /// Create absolute position event
    pub fn abs_event(axis: u16, value: i32) -> Self {
        Self::new(EV_ABS, axis, value)
    }

    /// Create sync event
    pub fn sync_event() -> Self {
        Self::new(EV_SYN, 0, 0)
    }
}

// ============================================================================
// High-level input event enum
// ============================================================================

/// Input event type (for internal use)
#[derive(Debug, Clone, Copy)]
pub enum InputEventKind {
    /// Keyboard event
    Key { code: u16, pressed: bool },
    /// Relative motion event
    RelativeMotion { dx: i32, dy: i32 },
    /// Absolute position event
    AbsolutePosition { x: i32, y: i32 },
    /// Mouse wheel
    Wheel { delta: i32 },
}
