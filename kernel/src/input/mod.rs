//! 输入事件系统
//!
//! 提供统一的输入事件接口

use crate::println;
use crate::drivers::keyboard::ps2::{KeyEvent, KEYBOARD};
use crate::drivers::mouse::ps2::{MouseEvent, MOUSE};
use alloc::collections::vec_deque::VecDeque;
use core::sync::atomic::{AtomicBool, Ordering};

pub const EV_KEY: u16 = 0x01;  // 按键事件
pub const EV_REL: u16 = 0x02;  // 相对坐标事件
pub const EV_ABS: u16 = 0x03;  // 绝对坐标事件

pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawInputEvent {
    /// 时间戳 (秒)
    pub tv_sec: u64,
    /// 时间戳 (微秒)
    pub tv_usec: u64,
    /// 事件类型
    pub type_: u16,
    /// 事件代码
    pub code: u16,
    /// 事件值
    pub value: i32,
}

/// 输入事件类型
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    /// 键盘事件
    Keyboard(KeyEvent),
    /// 鼠标移动
    MouseMove { dx: i16, dy: i16 },
    /// 鼠标按键
    MouseButton { left: bool, right: bool, middle: bool },
}

/// 输入事件队列（最大容量 128）
static EVENT_QUEUE: spin::Mutex<VecDeque<InputEvent>> = spin::Mutex::new(VecDeque::new());

/// 输入系统初始化标志
static INPUT_INIT: AtomicBool = AtomicBool::new(false);

/// 初始化输入系统
pub fn init() {
    use crate::drivers::keyboard;
    use crate::drivers::mouse;

    // 初始化键盘驱动
    keyboard::ps2::init();

    // 初始化鼠标驱动
    mouse::ps2::init();

    INPUT_INIT.store(true, Ordering::Release);
}

/// 拉取输入事件（非阻塞）
pub fn poll_event() -> Option<InputEvent> {
    if !INPUT_INIT.load(Ordering::Acquire) {
        return None;
    }

    // 首先检查键盘事件
    if let Some(event) = fetch_keyboard_event() {
        return Some(InputEvent::Keyboard(event));
    }

    // 然后检查鼠标事件
    if let Some(event) = fetch_mouse_event() {
        return Some(event);
    }

    None
}

/// 从键盘拉取事件
fn fetch_keyboard_event() -> Option<KeyEvent> {
    use crate::drivers::keyboard::ps2;

    unsafe {
        if ps2::KEYBOARD.has_data() {
            ps2::KEYBOARD.read_scancode()
        } else {
            None
        }
    }
}

/// 从鼠标拉取事件
fn fetch_mouse_event() -> Option<InputEvent> {
    use crate::drivers::mouse::ps2;

    unsafe {
        if ps2::MOUSE.has_data() {
            if let Some(event) = ps2::MOUSE.read_byte() {
                Some(match event {
                    ps2::MouseEvent::Move { dx, dy } => {
                        InputEvent::MouseMove { dx, dy }
                    }
                    ps2::MouseEvent::Button { left, right, middle } => {
                        InputEvent::MouseButton { left, right, middle }
                    }
                })
            } else {
                None
            }
        } else {
            None
        }
    }
}

pub fn get_raw_input_event() -> Option<RawInputEvent> {
    if let Some(event) = poll_event() {
        let raw_event = match event {
            InputEvent::Keyboard(key_event) => {
                // 键盘事件
                match key_event {
                    KeyEvent::Press(code) => RawInputEvent {
                        tv_sec: 0,
                        tv_usec: 0,
                        type_: EV_KEY,
                        code: code as u16,
                        value: 1,  // 按下
                    },
                    KeyEvent::Release(code) => RawInputEvent {
                        tv_sec: 0,
                        tv_usec: 0,
                        type_: EV_KEY,
                        code: code as u16,
                        value: 0,  // 释放
                    },
                }
            }
            InputEvent::MouseMove { dx, dy } => {
                // 鼠标移动事件 - 需要返回两个事件 (X 和 Y)
                // 简化处理：只返回 X 移动，Y 移动在下一次调用返回
                RawInputEvent {
                    tv_sec: 0,
                    tv_usec: 0,
                    type_: EV_REL,
                    code: REL_X,
                    value: dx as i32,
                }
            }
            InputEvent::MouseButton { left, right, middle } => {
                // 鼠标按键事件
                if left {
                    RawInputEvent {
                        tv_sec: 0,
                        tv_usec: 0,
                        type_: EV_KEY,
                        code: BTN_LEFT,
                        value: 1,
                    }
                } else if right {
                    RawInputEvent {
                        tv_sec: 0,
                        tv_usec: 0,
                        type_: EV_KEY,
                        code: BTN_RIGHT,
                        value: 1,
                    }
                } else if middle {
                    RawInputEvent {
                        tv_sec: 0,
                        tv_usec: 0,
                        type_: EV_KEY,
                        code: BTN_MIDDLE,
                        value: 1,
                    }
                } else {
                    // 按键释放 - 假设是左键
                    RawInputEvent {
                        tv_sec: 0,
                        tv_usec: 0,
                        type_: EV_KEY,
                        code: BTN_LEFT,
                        value: 0,
                    }
                }
            }
        };
        Some(raw_event)
    } else {
        None
    }
}
