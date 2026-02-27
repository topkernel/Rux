//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 输入子系统
//!
//! 提供统一的输入设备接口，包括：
//! - VirtIO Input 驱动（RISC-V 主要输入设备）
//! - PS/2 驱动（x86 兼容，RISC-V 上不可用）
//! - evdev 字符设备接口
//! - 输入事件定义

use crate::println;
use alloc::sync::Arc;
use spin::Mutex;

pub mod event;
pub mod ps2;
pub mod virtio_input;
pub mod evdev;

// 重导出常用类型
pub use event::*;
pub use evdev::{EvdevDevice, evdev_ioctl, evdev_read, EVDEV_KEYBOARD_FD, EVDEV_POINTER_FD};
pub use virtio_input::{VirtioInputDevice, probe_virtio_input};

// ============================================================================
// 全局输入设备
// ============================================================================

/// VirtIO 键盘设备
pub static INPUT_KEYBOARD: Mutex<Option<VirtioInputDevice>> = Mutex::new(None);

/// VirtIO 指针设备（鼠标/触摸屏）
pub static INPUT_POINTER: Mutex<Option<VirtioInputDevice>> = Mutex::new(None);

// ============================================================================
// 初始化
// ============================================================================

/// 初始化输入子系统
pub fn init() {
    // 初始化 PS/2 驱动（在 RISC-V 上不做任何事）
    ps2::init_keyboard();
    ps2::init_mouse();
}

/// 初始化 VirtIO Input 设备
pub fn init_virtio_input() -> (usize, usize) {
    let mut keyboard_count = 0;
    let mut pointer_count = 0;

    // 探测 VirtIO Input 设备
    for device in 0..32u8 {
        let ecam_addr = crate::drivers::pci::RISCV_PCIE_ECAM_BASE
            + ((device as u64) * crate::drivers::pci::PCIE_ECAM_SIZE);

        let vendor_id = unsafe { core::ptr::read_volatile((ecam_addr as *const u16)) };
        let device_id = unsafe { core::ptr::read_volatile((ecam_addr as *const u16).add(1)) };

        // VirtIO Input: Vendor 0x1AF4, Device 0x1052
        if vendor_id == 0x1AF4 && device_id == 0x1052 {
            if let Ok(virtio_pci) = crate::drivers::virtio::virtio_pci::VirtIOPCI::new(ecam_addr) {
                if let Some(input_dev) = VirtioInputDevice::new(virtio_pci) {
                    let is_pointer = input_dev.is_pointer();

                    if is_pointer {
                        if INPUT_POINTER.lock().is_none() {
                            *INPUT_POINTER.lock() = Some(input_dev);
                            pointer_count += 1;
                        }
                    } else {
                        if INPUT_KEYBOARD.lock().is_none() {
                            *INPUT_KEYBOARD.lock() = Some(input_dev);
                            keyboard_count += 1;
                        }
                    }
                }
            }
        }

        // 如果两个设备都找到了，停止探测
        if INPUT_KEYBOARD.lock().is_some() && INPUT_POINTER.lock().is_some() {
            break;
        }
    }

    // 初始化 evdev 设备
    evdev::init_evdev();

    (keyboard_count, pointer_count)
}

/// 轮询输入事件
pub fn poll_events() {
    // 轮询键盘
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                evdev::push_input_event(false, event);
            }
        }
    }

    // 轮询指针设备
    if let Some(ref mut ptr) = *INPUT_POINTER.lock() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                evdev::push_input_event(true, event);
            }
        }
    }
}

/// 获取键盘事件（兼容旧接口）
pub fn get_keyboard_event() -> Option<InputEvent> {
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock() {
        kb.read_event()
    } else {
        None
    }
}

/// 获取指针事件（兼容旧接口）
pub fn get_pointer_event() -> Option<InputEvent> {
    if let Some(ref mut ptr) = *INPUT_POINTER.lock() {
        ptr.read_event()
    } else {
        None
    }
}
