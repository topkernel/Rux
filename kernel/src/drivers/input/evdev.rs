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
use alloc::boxed::Box;
use spin::Mutex;
use crate::fs::file::{File, FileOps};
use crate::fs::dev_t::{DevNo, DEV_EVDEV_KEYBOARD, DEV_EVDEV_POINTER};
use crate::fs::devfs;

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

// ============================================================================
// FileOps 实现
// ============================================================================

/// evdev 读取函数
fn evdev_file_read(file: &File, buf: &mut [u8]) -> isize {
    // 获取设备号
    let devno = unsafe {
        match *file.private_data.get() {
            Some(ptr) => *(ptr as *const DevNo),
            None => return -9, // EBADF
        }
    };

    // 根据设备号选择设备
    let device = unsafe {
        if devno == DEV_EVDEV_KEYBOARD {
            EVDEV_KEYBOARD.as_ref()
        } else if devno == DEV_EVDEV_POINTER {
            EVDEV_POINTER.as_ref()
        } else {
            return -19; // ENODEV
        }
    };

    let device = match device {
        Some(d) => d,
        None => return -19, // ENODEV
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if buf.len() < event_size {
        return -22; // EINVAL
    }

    // 轮询新事件
    poll_virtio_events();

    match device.pop_event() {
        Some(event) => {
            // 复制事件到缓冲区
            let src = &event as *const InputEvent as *const u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), event_size);
            }
            event_size as isize
        }
        None => -11, // EAGAIN (非阻塞模式)
    }
}

/// evdev 关闭函数
fn evdev_file_close(_file: &File) -> i32 {
    // 目前不需要特殊处理
    0
}

/// evdev FileOps
pub static EVDEV_OPS: FileOps = FileOps {
    read: Some(evdev_file_read),
    write: None,
    lseek: None,
    close: Some(evdev_file_close),
};

// ============================================================================
// 初始化和注册
// ============================================================================

/// 初始化 evdev 设备并注册到 devfs
pub fn init_evdev() {
    unsafe {
        // 创建键盘设备
        EVDEV_KEYBOARD = Some(EvdevDevice::new(b"VirtIO Keyboard", false));

        // 创建指针设备
        EVDEV_POINTER = Some(EvdevDevice::new(b"VirtIO Tablet", true));
    }

    // 注册设备操作
    devfs::registry::register_char_device(DEV_EVDEV_KEYBOARD, &EVDEV_OPS)
        .expect("Failed to register keyboard evdev");
    devfs::registry::register_char_device(DEV_EVDEV_POINTER, &EVDEV_OPS)
        .expect("Failed to register pointer evdev");

    // 创建设备节点
    devfs::mknod("/input/event0", DEV_EVDEV_KEYBOARD, 0o666)
        .expect("Failed to create /dev/input/event0");
    devfs::mknod("/input/event1", DEV_EVDEV_POINTER, 0o666)
        .expect("Failed to create /dev/input/event1");
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

/// 轮询 VirtIO 输入设备
fn poll_virtio_events() {
    use crate::drivers::input::{INPUT_KEYBOARD, INPUT_POINTER};

    // 轮询键盘
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                push_input_event(false, event);
            }
        }
    }

    // 轮询指针设备
    if let Some(ref mut ptr) = *INPUT_POINTER.lock() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                push_input_event(true, event);
            }
        }
    }
}

// ============================================================================
// 旧接口兼容（用于 ioctl）
// ============================================================================

/// 处理 evdev ioctl (通过 fd)
pub fn evdev_ioctl(fd: i32, cmd: u32, arg: usize) -> i64 {
    // 兼容旧的 fd 方式
    let device = unsafe {
        if fd == 2000 {  // EVDEV_KEYBOARD_FD
            EVDEV_KEYBOARD.as_ref()
        } else if fd == 2001 {  // EVDEV_POINTER_FD
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
            unsafe {
                let name_ptr = arg as *mut u8;
                let name = &device.name;
                let len = name.iter().position(|&c| c == 0).unwrap_or(31) + 1;
                core::ptr::copy_nonoverlapping(name.as_ptr(), name_ptr, len.min(256));
            }
            0
        }

        EVIOCGBIT => {
            let event_type = (cmd >> 8) & 0xFF;
            unsafe {
                let bits_ptr = arg as *mut u8;
                match event_type {
                    0 => {
                        let bits: [u8; 4] = [0x01, 0x03, 0x00, 0x00];
                        core::ptr::copy_nonoverlapping(bits.as_ptr(), bits_ptr, 4);
                    }
                    1 => {
                        for i in 0..32 {
                            core::ptr::write(bits_ptr.add(i), 0xFF);
                        }
                    }
                    2 => {
                        if device.is_pointer {
                            core::ptr::write(bits_ptr, 0x03);
                        }
                    }
                    3 => {
                        if device.is_pointer {
                            core::ptr::write(bits_ptr, 0x03);
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

/// 处理 evdev read (通过 fd) - 保留用于兼容
pub fn evdev_read(fd: i32, buf: usize, count: usize) -> i64 {
    let device = unsafe {
        if fd == 2000 {
            EVDEV_KEYBOARD.as_ref()
        } else if fd == 2001 {
            EVDEV_POINTER.as_ref()
        } else {
            return -22;
        }
    };

    let device = match device {
        Some(d) => d,
        None => return -19,
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if count < event_size {
        return -22;
    }

    poll_virtio_events();

    match device.pop_event() {
        Some(event) => {
            unsafe {
                core::ptr::write(buf as *mut InputEvent, event);
            }
            event_size as i64
        }
        None => -11,
    }
}
