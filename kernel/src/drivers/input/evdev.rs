//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! evdev character device interface
//!
//! Provides compatible /dev/input/eventX device

use super::event::*;
use super::{INPUT_KEYBOARD, INPUT_POINTER};
use alloc::collections::vec_deque::VecDeque;
use alloc::boxed::Box;
use crate::sync::spinlock::Spinlock;
use crate::fs::file::{File, FileOps};
use crate::fs::dev_t::{DevNo, DEV_EVDEV_KEYBOARD, DEV_EVDEV_POINTER};
use crate::fs::devfs;

// ============================================================================
// evdev ioctl commands
// ============================================================================

/// Get driver version
pub const EVIOCGVERSION: u32 = 0x80044501;
/// Get device ID
pub const EVIOCGID: u32 = 0x80084502;
/// Get device name
pub const EVIOCGNAME: u32 = 0x80004506;
/// Get supported event type bitmap
pub const EVIOCGBIT: u32 = 0x80004520;
/// Get device properties
pub const EVIOCGPROP: u32 = 0x80004502;

// ============================================================================
// Input device ID structure
// ============================================================================

/// Input device ID (input_id)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputId {
    /// Bus type
    pub bustype: u16,
    /// Vendor ID
    pub vendor: u16,
    /// Product ID
    pub product: u16,
    /// Version
    pub version: u16,
}

// ============================================================================
// evdev device
// ============================================================================

/// evdev event queue maximum capacity - from config
const EVENT_QUEUE_SIZE: usize = crate::config::EVDEV_EVENT_QUEUE_SIZE;

/// evdev device structure
pub struct EvdevDevice {
    /// Device name
    pub name: [u8; 32],
    /// Device ID
    pub id: InputId,
    /// Whether it is a pointer device
    pub is_pointer: bool,
    /// Event queue
    pub event_queue: Spinlock<VecDeque<InputEvent>>,
}

impl EvdevDevice {
    /// Create new evdev device
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
            event_queue: Spinlock::new(VecDeque::with_capacity(EVENT_QUEUE_SIZE)),
        }
    }

    /// Push event (called from softirq context)
    pub fn push_event(&self, event: InputEvent) {
        let mut queue = self.event_queue.lock_irqsave();
        if queue.len() >= EVENT_QUEUE_SIZE {
            queue.pop_front();
        }
        queue.push_back(event);
    }

    /// Read event
    pub fn pop_event(&self) -> Option<InputEvent> {
        self.event_queue.lock_irqsave().pop_front()
    }

    /// Check if there are events
    pub fn has_event(&self) -> bool {
        !self.event_queue.lock_irqsave().is_empty()
    }
}

// ============================================================================
// Global evdev devices
// ============================================================================

/// Keyboard evdev device
pub static mut EVDEV_KEYBOARD: Option<EvdevDevice> = None;

/// Pointer evdev device
pub static mut EVDEV_POINTER: Option<EvdevDevice> = None;

// ============================================================================
// FileOps implementation
// ============================================================================

/// evdev read function
fn evdev_file_read(file: &File, buf: &mut [u8]) -> isize {
    // Get device number
    let devno = unsafe {
        match *file.private_data.get() {
            Some(ptr) => *(ptr as *const DevNo),
            None => return -9, // EBADF
        }
    };

    // Select device based on device number
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

    // Poll for new events
    poll_virtio_events();

    match device.pop_event() {
        Some(event) => {
            // Copy event to buffer
            let src = &event as *const InputEvent as *const u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), event_size);
            }
            event_size as isize
        }
        None => -11, // EAGAIN (non-blocking mode)
    }
}

/// evdev close function
fn evdev_file_close(_file: &File) -> i32 {
    // No special handling needed currently
    0
}

/// evdev FileOps
pub static EVDEV_OPS: FileOps = FileOps {
    read: Some(evdev_file_read),
    write: None,
    lseek: None,
    close: Some(evdev_file_close),
    poll: None,
};

// ============================================================================
// Initialization and registration
// ============================================================================

/// Initialize evdev devices and register to devfs
pub fn init_evdev() {
    unsafe {
        // Create keyboard device
        EVDEV_KEYBOARD = Some(EvdevDevice::new(b"VirtIO Keyboard", false));

        // Create pointer device
        EVDEV_POINTER = Some(EvdevDevice::new(b"VirtIO Tablet", true));
    }

    // Register device operations
    devfs::registry::register_char_device(DEV_EVDEV_KEYBOARD, &EVDEV_OPS)
        .expect("Failed to register keyboard evdev");
    devfs::registry::register_char_device(DEV_EVDEV_POINTER, &EVDEV_OPS)
        .expect("Failed to register pointer evdev");

    // Create device nodes
    devfs::mknod("/input/event0", DEV_EVDEV_KEYBOARD, 0o666)
        .expect("Failed to create /dev/input/event0");
    devfs::mknod("/input/event1", DEV_EVDEV_POINTER, 0o666)
        .expect("Failed to create /dev/input/event1");
}

/// Push event to evdev device
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

/// Poll VirtIO input devices
fn poll_virtio_events() {
    use crate::drivers::input::{INPUT_KEYBOARD, INPUT_POINTER};

    // Poll keyboard
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                push_input_event(false, event);
            }
        }
    }

    // Poll pointer device
    if let Some(ref mut ptr) = *INPUT_POINTER.lock() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                push_input_event(true, event);
            }
        }
    }
}

// ============================================================================
// Legacy interface compatibility (for ioctl)
// ============================================================================

/// Handle evdev ioctl (via fd)
pub fn evdev_ioctl(fd: i32, cmd: u32, arg: usize) -> i64 {
    // Compatible with old fd-based approach
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

/// Handle evdev read (via fd) - kept for compatibility
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
