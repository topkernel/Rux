//! 输入事件系统
//!
//! 提供统一的输入事件接口

use crate::println;
use crate::drivers::keyboard::ps2::{KeyEvent, KEYBOARD};
use crate::drivers::mouse::ps2::{MouseEvent, MOUSE};
use alloc::collections::vec_deque::VecDeque;
use core::sync::atomic::{AtomicBool, Ordering};

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
static EVENT_QUEUE: spin::Mutex<VecDeque<InputEvent, 128>> = spin::Mutex::new(VecDeque::new());

/// 输入系统初始化标志
static INPUT_INIT: AtomicBool = AtomicBool::new(false);

/// 初始化输入系统
pub fn init() {
    use crate::drivers::keyboard;
    use crate::drivers::mouse;

    println!("input: Initializing input subsystem...");

    // 初始化键盘驱动
    keyboard::ps2::init();

    // 初始化鼠标驱动
    mouse::ps2::init();

    INPUT_INIT.store(true, Ordering::Release);

    println!("input: Input subsystem initialized [OK]");
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
