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
pub use evdev::{EvdevDevice, evdev_file_ioctl};
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

    // Probe VirtIO Input devices via the shared ECAM walker (0x8000 stride
    // per slot + all 8 functions). The old inline walk here enumerated
    // ECAM slot bases only (function 0 of the first slots) — QEMU's virt
    // machine places virtio-keyboard-pci and virtio-tablet-pci on later
    // slots/functions, so the tablet was silently never probed and the
    // guest ended up with a keyboard but no pointer (review BUG:
    // "tablet 从未被探测").
    for ecam_addr in crate::drivers::pci::find_ecam_devices(0x1AF4, &[0x1052]) {
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

    // Initialize evdev devices
    evdev::init_evdev();

    (keyboard_count, pointer_count)
}

/// Poll input events
pub fn poll_events() {
    // Poll keyboard (irqsafe: called from softirq which may be preempted by hard IRQ)
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock_irqsave() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                evdev::push_input_event(false, event);
            }
        }
    }

    // Poll pointer device
    if let Some(ref mut ptr) = *INPUT_POINTER.lock_irqsave() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                evdev::push_input_event(true, event);
            }
        }
    }
}

/// Get keyboard event (legacy interface compatibility)
pub fn get_keyboard_event() -> Option<InputEvent> {
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock_irqsave() {
        kb.read_event()
    } else {
        None
    }
}

/// Get pointer event (legacy interface compatibility)
pub fn get_pointer_event() -> Option<InputEvent> {
    if let Some(ref mut ptr) = *INPUT_POINTER.lock_irqsave() {
        ptr.read_event()
    } else {
        None
    }
}
