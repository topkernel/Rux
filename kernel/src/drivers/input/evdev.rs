//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! evdev 字符设备接口
//!
//! 提供 Linux 兼容的 /dev/input/eventX 设备

use super::event::*;
use super::{INPUT_KEYBOARD, INPUT_POINTER};
use alloc::collections::vec_deque::VecDeque;
use spin::Mutex;

// ============================================================================
// evdev ioctl 命令
// ============================================================================

/// 获取驱动版本
pub const EVIOCGVERSION: u32 = 0x80044501;
/// 获取设备 ID
pub const EVIOCGID: u32 = 0x80084502;
/// 获取设备名称
pub const EVIOCGNAME: u32 = 0x80004506;
/// 获取支持的事件类型位图
pub const EVIOCGBIT: u32 = 0x80004520;
/// 获取设备属性
pub const EVIOCGPROP: u32 = 0x80004502;

// ============================================================================
// 输入设备 ID 结构
// ============================================================================

/// 输入设备 ID (Linux input_id)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputId {
    /// 总线类型
    pub bustype: u16,
    /// 厂商 ID
    pub vendor: u16,
    /// 产品 ID
    pub product: u16,
    /// 版本
    pub version: u16,
}

// ============================================================================
// evdev 设备
// ============================================================================

/// evdev 事件队列最大容量
const EVENT_QUEUE_SIZE: usize = 64;

/// evdev 设备结构
pub struct EvdevDevice {
    /// 设备名称
    pub name: [u8; 32],
    /// 设备 ID
    pub id: InputId,
    /// 是否为指针设备
    pub is_pointer: bool,
    /// 事件队列
    pub event_queue: Mutex<VecDeque<InputEvent>>,
}

impl EvdevDevice {
    /// 创建新的 evdev 设备
    pub fn new(name: &[u8], is_pointer: bool) -> Self {
        let mut name_arr = [0u8; 32];
        let len = name.len().min(31);
        name_arr[..len].copy_from_slice(&name[..len]);

        Self {
            name: name_arr,
            id: InputId {
                bustype: 0x0019, // BUS_VIRTIO
                vendor: 0x1AF4,  // Red Hat
                product: if is_pointer { 0x1052 } else { 0x1052 },
                version: 0x0001,
            },
            is_pointer,
            event_queue: Mutex::new(VecDeque::with_capacity(EVENT_QUEUE_SIZE)),
        }
    }

    /// 推入事件
    pub fn push_event(&self, event: InputEvent) {
        let mut queue = self.event_queue.lock();
        if queue.len() >= EVENT_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(event);
    }

    /// 读取事件
    pub fn pop_event(&self) -> Option<InputEvent> {
        self.event_queue.lock().pop_front()
    }

    /// 检查是否有事件
    pub fn has_event(&self) -> bool {
        !self.event_queue.lock().is_empty()
    }
}

// ============================================================================
// 全局 evdev 设备
// ============================================================================

/// 键盘 evdev 设备
pub static mut EVDEV_KEYBOARD: Option<EvdevDevice> = None;

/// 指针 evdev 设备
pub static mut EVDEV_POINTER: Option<EvdevDevice> = None;

/// 初始化 evdev 设备
pub fn init_evdev() {
    unsafe {
        // 创建键盘设备
        EVDEV_KEYBOARD = Some(EvdevDevice::new(b"VirtIO Keyboard", false));

        // 创建指针设备
        EVDEV_POINTER = Some(EvdevDevice::new(b"VirtIO Tablet", true));
    }
}

/// 向 evdev 设备推送事件
pub fn push_input_event(is_pointer: bool, event: InputEvent) {
    unsafe {
        if is_pointer {
            if let Some(ref dev) = EVDEV_POINTER {
                dev.push_event(event);
            }
        } else {
            if let Some(ref dev) = EVDEV_KEYBOARD {
                dev.push_event(event);
            }
        }
    }
}

// ============================================================================
// evdev ioctl 处理
// ============================================================================

/// 特殊 fd 用于 evdev 设备
pub const EVDEV_KEYBOARD_FD: i32 = 2000;
pub const EVDEV_POINTER_FD: i32 = 2001;

/// 处理 evdev ioctl
pub fn evdev_ioctl(fd: i32, cmd: u32, arg: usize) -> i64 {
    let device = unsafe {
        if fd == EVDEV_KEYBOARD_FD {
            EVDEV_KEYBOARD.as_ref()
        } else if fd == EVDEV_POINTER_FD {
            EVDEV_POINTER.as_ref()
        } else {
            return -22; // EINVAL
        }
    };

    let device = match device {
        Some(d) => d,
        None => return -19, // ENODEV
    };

    match cmd {
        EVIOCGVERSION => {
            // 返回 evdev 版本 (0x010001)
            let version: u32 = 0x010001;
            unsafe {
                core::ptr::write(arg as *mut u32, version);
            }
            0
        }

        EVIOCGID => {
            unsafe {
                core::ptr::write(arg as *mut InputId, device.id);
            }
            0
        }

        EVIOCGNAME => {
            // 返回设备名称
            unsafe {
                let name_ptr = arg as *mut u8;
                let name = &device.name;
                let len = name.iter().position(|&c| c == 0).unwrap_or(31) + 1;
                core::ptr::copy_nonoverlapping(name.as_ptr(), name_ptr, len.min(256));
            }
            0
        }

        EVIOCGBIT => {
            // 返回支持的事件类型
            let event_type = (cmd >> 8) & 0xFF;
            unsafe {
                let bits_ptr = arg as *mut u8;
                match event_type {
                    0 => {
                        // 返回支持的事件类型位图
                        let bits: [u8; 4] = [
                            0x01, // EV_SYN
                            0x03, // EV_KEY | EV_REL (键盘)
                            0x00,
                            0x00,
                        ];
                        core::ptr::copy_nonoverlapping(bits.as_ptr(), bits_ptr, 4);
                    }
                    1 => {
                        // EV_KEY 位图
                        // 简化：假设支持所有按键
                        for i in 0..32 {
                            core::ptr::write(bits_ptr.add(i), 0xFF);
                        }
                    }
                    2 => {
                        // EV_REL 位图 (鼠标)
                        if device.is_pointer {
                            core::ptr::write(bits_ptr, 0x03); // REL_X | REL_Y
                        }
                    }
                    3 => {
                        // EV_ABS 位图 (触摸屏)
                        if device.is_pointer {
                            core::ptr::write(bits_ptr, 0x03); // ABS_X | ABS_Y
                        }
                    }
                    _ => {}
                }
            }
            0
        }

        _ => -25, // ENOTTY
    }
}

/// 处理 evdev read
pub fn evdev_read(fd: i32, buf: usize, count: usize) -> i64 {
    let device = unsafe {
        if fd == EVDEV_KEYBOARD_FD {
            EVDEV_KEYBOARD.as_ref()
        } else if fd == EVDEV_POINTER_FD {
            EVDEV_POINTER.as_ref()
        } else {
            return -22; // EINVAL
        }
    };

    let device = match device {
        Some(d) => d,
        None => return -19, // ENODEV
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if count < event_size {
        return -22; // EINVAL
    }

    match device.pop_event() {
        Some(event) => {
            unsafe {
                core::ptr::write(buf as *mut InputEvent, event);
            }
            event_size as i64
        }
        None => -11, // EAGAIN (非阻塞模式)
    }
}
