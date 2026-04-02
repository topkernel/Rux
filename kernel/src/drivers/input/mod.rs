//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Input subsystem
//!
//! Provides unified input device interface, including:
//! - VirtIO Input driver (main input device for RISC-V)
//! - PS/2 driver (x86 compatible, not available on RISC-V)
//! - evdev character device interface
//! - Input event definitions

use crate::println;
use alloc::sync::Arc;
use crate::sync::spinlock::Spinlock;

pub mod event;
pub mod ps2;
pub mod virtio_input;
pub mod evdev;

// Re-export common types
pub use event::*;
pub use evdev::{EvdevDevice, evdev_ioctl, evdev_read};
pub use virtio_input::{VirtioInputDevice, probe_virtio_input};

// ============================================================================
// Global input devices
// ============================================================================

/// VirtIO keyboard device
pub static INPUT_KEYBOARD: Spinlock<Option<VirtioInputDevice>> = Spinlock::new(None);

/// VirtIO pointer device (mouse/touchscreen)
pub static INPUT_POINTER: Spinlock<Option<VirtioInputDevice>> = Spinlock::new(None);

// ============================================================================
// Initialization
// ============================================================================

/// Initialize input subsystem
pub fn init() {
    // Initialize PS/2 driver (does nothing on RISC-V)
    ps2::init_keyboard();
    ps2::init_mouse();
}

/// Initialize VirtIO Input devices
pub fn init_virtio_input() -> (usize, usize) {
    let mut keyboard_count = 0;
    let mut pointer_count = 0;

    // Probe VirtIO Input devices
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

        // If both devices found, stop probing
        if INPUT_KEYBOARD.lock().is_some() && INPUT_POINTER.lock().is_some() {
            break;
        }
    }

    // Initialize evdev devices
    evdev::init_evdev();

    (keyboard_count, pointer_count)
}

/// Poll input events
pub fn poll_events() {
    // Poll keyboard
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                evdev::push_input_event(false, event);
            }
        }
    }

    // Poll pointer device
    if let Some(ref mut ptr) = *INPUT_POINTER.lock() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                evdev::push_input_event(true, event);
            }
        }
    }
}

/// Get keyboard event (legacy interface compatibility)
pub fn get_keyboard_event() -> Option<InputEvent> {
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock() {
        kb.read_event()
    } else {
        None
    }
}

/// Get pointer event (legacy interface compatibility)
pub fn get_pointer_event() -> Option<InputEvent> {
    if let Some(ref mut ptr) = *INPUT_POINTER.lock() {
        ptr.read_event()
    } else {
        None
    }
}
