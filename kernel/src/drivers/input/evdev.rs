//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! evdev character device interface
//!
//! Provides compatible /dev/input/eventX device
//!
//! Linux-parity checklist (xf86-input-evdev / libevdev / evtest probe path):
//! - EVIOCGVERSION / EVIOCGID / EVIOCGNAME(len) / EVIOCGPROP(len)
//! - EVIOCGBIT(ev, len) from the virtio device's own capability bitmaps
//! - EVIOCGABS(abs) from the virtio device's ABS_INFO config
//! - EVIOCGRAB (exclusive access) / EVIOCGKEY / EVIOCGLED / EVIOCGSW
//! - blocking read (sleep until an event arrives) and poll reporting POLLIN
//! - 24-byte input_event passthrough including EV_SYN/SYN_REPORT packets

use super::event::*;
use alloc::collections::vec_deque::VecDeque;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::sync::spinlock::Spinlock;
use crate::fs::file::{File, FileOps};
use crate::fs::dev_t::{DevNo, DEV_EVDEV_KEYBOARD, DEV_EVDEV_POINTER};
use crate::fs::devfs;

// ============================================================================
// evdev ioctl command decoding
// ============================================================================

/// ioctl NR (command byte) base values from <uapi/linux/input.h>. The size
/// field of EVIOCGNAME/EVIOCGBIT-style commands carries the user buffer
/// length and must be honored, so commands are decoded by NR + DIR instead
/// of compared as whole u32s.
const NR_VERSION: u32 = 0x01; // EVIOCGVERSION
const NR_ID: u32 = 0x02; // EVIOCGID
const NR_REP_GET: u32 = 0x03; // EVIOCGREP
const NR_REP_SET: u32 = 0x04; // EVIOCSREP
const NR_NAME: u32 = 0x06; // EVIOCGNAME(len)
const NR_PHYS: u32 = 0x07; // EVIOCGPHYS(len)
const NR_UNIQ: u32 = 0x08; // EVIOCGUNIQ(len)
const NR_PROP: u32 = 0x09; // EVIOCGPROP(len)
const NR_KEY_STATE: u32 = 0x18; // EVIOCGKEY(len)
const NR_LED_STATE: u32 = 0x19; // EVIOCGLED(len)
const NR_SND_STATE: u32 = 0x1a; // EVIOCGSND(len)
const NR_SW_STATE: u32 = 0x1b; // EVIOCGSW(len)
const NR_BIT_BASE: u32 = 0x20; // EVIOCGBIT(ev, len): 0x20 + ev
const NR_ABS_BASE: u32 = 0x40; // EVIOCGABS(abs): 0x40 + abs
const NR_GRAB: u32 = 0x90; // EVIOCGRAB
const NR_SCLOCKID: u32 = 0xa0; // EVIOCSCLOCKID

const IOC_READ: u32 = 2; // _IOC_WRITE/_IOC_READ from asm-generic/ioctl.h
const IOC_WRITE: u32 = 1;

#[inline]
fn ioc_nr(cmd: u32) -> u32 {
    cmd & 0xff
}

#[inline]
fn ioc_size(cmd: u32) -> usize {
    ((cmd >> 16) & 0x3fff) as usize
}

#[inline]
fn ioc_dir(cmd: u32) -> u32 {
    cmd >> 30
}

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

/// input_absinfo (uapi layout): current value + the five virtio_absinfo
/// fields.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputAbsinfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

// ============================================================================
// Capability bitmaps
// ============================================================================

/// KEY_CNT = 768 bits = 96 bytes; uniform storage for every event type's
/// code bitmap (SYN/REL/ABS use far less).
const BITS_LEN: usize = 96;
/// Number of event types we carry code bitmaps for: EV_SYN..EV_ABS.
const BIT_TYPES: usize = 4;

/// Fill `out` with the derived "supported event types" bitmap (what Linux
/// reports for EVIOCGBIT(0, len)): a type is supported when its code bitmap
/// is non-empty; EV_SYN is always present (input core guarantees it).
fn evtype_bitmap(bits: &[[u8; BITS_LEN]; BIT_TYPES]) -> [u8; BITS_LEN] {
    let mut out = [0u8; BITS_LEN];
    out[0] |= 1 << (EV_SYN & 7); // bit 0
    if bits[1].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_KEY & 7); // bit 1
    }
    if bits[2].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_REL & 7); // bit 2
    }
    if bits[3].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_ABS & 7); // bit 3
    }
    out
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
    /// Code bitmaps for EV_SYN(0)/EV_KEY(1)/EV_REL(2)/EV_ABS(3), read from
    /// the virtio device config (VIRTIO_INPUT_CFG_EV_BITS) at init.
    ev_bits: [[u8; BITS_LEN]; BIT_TYPES],
    /// ABS axis info for ABS_X/ABS_Y (min/max/fuzz/flat/res from
    /// VIRTIO_INPUT_CFG_ABS_INFO) — reported through EVIOCGABS.
    absinfo: [InputAbsinfo; 2],
    /// EVIOCGRAB state (device-level; we have a single client in practice).
    grabbed: AtomicBool,
    /// LED state bitmap (bit N = LED_N), maintained from EV_LED writes the
    /// way Linux's evdev_write → input_inject_event does; readable back via
    /// EVIOCGLED(len).
    led_state: AtomicU32,
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
            ev_bits: [[0u8; BITS_LEN]; BIT_TYPES],
            absinfo: [InputAbsinfo::default(); 2],
            grabbed: AtomicBool::new(false),
            led_state: AtomicU32::new(0),
            event_queue: Spinlock::new(VecDeque::with_capacity(EVENT_QUEUE_SIZE)),
        }
    }

    /// Install a capability bitmap for event type `ev` (0..=3).
    pub fn set_ev_bits(&mut self, ev: usize, bits: &[u8; BITS_LEN]) {
        if ev < BIT_TYPES {
            self.ev_bits[ev] = *bits;
        }
    }

    /// Install ABS axis info for ABS_X (0) / ABS_Y (1).
    pub fn set_absinfo(&mut self, axis: usize, min: i32, max: i32, fuzz: i32, flat: i32, res: i32) {
        if axis < 2 {
            self.absinfo[axis] = InputAbsinfo {
                value: 0,
                minimum: min,
                maximum: max,
                fuzz,
                flat,
                resolution: res,
            };
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

/// Resolve the EvdevDevice a file was opened against. Returns None for
/// non-evdev files (caller falls through to other ioctl handlers).
// SAFETY: EVDEV_KEYBOARD / EVDEV_POINTER are initialized by init_evdev()
// before any file operations can occur; private_data holds a valid DevNo
// boxed by devfs_open for as long as the file lives.
unsafe fn device_of_file(file: &File) -> Option<&'static EvdevDevice> {
    let ptr = match *(file.private_data.get()) {
        Some(p) => p,
        None => return None,
    };
    let devno = *(ptr as *const DevNo);
    if devno == DEV_EVDEV_KEYBOARD {
        EVDEV_KEYBOARD.as_ref()
    } else if devno == DEV_EVDEV_POINTER {
        EVDEV_POINTER.as_ref()
    } else {
        None
    }
}

// ============================================================================
// FileOps implementation
// ============================================================================

/// Interruptible timed sleep (milliseconds) used by the blocking read —
/// the task actually sleeps (yields the CPU) instead of busy-polling and
/// wakes early on signals.
///
/// Returns false if no task context / timer was available (caller should
/// fall back to a yield-style retry).
fn sleep_ms_interruptible(ms: u64) -> bool {
    use crate::process::task::{Task, TaskState};

    let target =
        crate::drivers::timer::get_jiffies() + crate::drivers::timer::msecs_to_jiffies(ms);

    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return false,
    };
    // SAFETY: current is the running task (we are it); pid() is a const read.
    let pid = unsafe { (*current).pid() };

    // One-shot timer is the only waker (no IRQ path is registered for the
    // input queues), so registration failure means we must NOT sleep.
    let timer_id = crate::timer::add_timer_wakeup(target, pid);
    if timer_id == 0 {
        return false;
    }

    loop {
        let jiffies_now = crate::drivers::timer::get_jiffies();
        if jiffies_now >= target || crate::signal::signal_pending() {
            crate::timer::del_timer(timer_id);
            return true;
        }

        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first lost-
        // wakeup discipline, same as sys_nanosleep).
        // SAFETY: current is the running task's pointer.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        let jiffies_now = crate::drivers::timer::get_jiffies();
        if jiffies_now >= target || crate::signal::signal_pending() {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::RUNNING));
            }
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            // SAFETY: current is the running task's pointer.
            unsafe {
                crate::sched::dequeue_task(&*current);
            }
            crate::timer::del_timer(timer_id);
            return true;
        }

        // Syscall context runs with SIE=0 — re-arm so the timer tick can
        // reach this CPU and wake us.
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
        // Woke up (timer expiry or signal): loop tail re-checks and exits.
    }
}

/// evdev read function
///
/// Linux semantics: a blocking fd sleeps until at least one event is
/// available (returning exactly one input_event per read); O_NONBLOCK
/// returns -EAGAIN when the queue is empty. Events are drained from the
/// virtio queues before every check because the input path has no IRQ —
/// the reader itself is the poller.
fn evdev_file_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data contains a valid DevNo pointer set during device
    // open; the EVDEV_* statics are initialized by init_evdev() before any
    // file operations can occur.
    let device = match unsafe { device_of_file(file) } {
        Some(d) => d,
        None => return -9, // EBADF
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if buf.len() < event_size {
        return -22; // EINVAL
    }

    let nonblock = file.flags_bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0;

    loop {
        // Poll for new events
        poll_virtio_events();

        match device.pop_event() {
            Some(event) => {
                // Copy event to buffer
                let src = &event as *const InputEvent as *const u8;
                // SAFETY: src points to a valid InputEvent on the stack; buf is
                // guaranteed to be at least event_size bytes by the check above.
                unsafe {
                    core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), event_size);
                }
                return event_size as isize;
            }
            None => {
                if nonblock {
                    return -11; // EAGAIN
                }
                if crate::signal::signal_pending() {
                    return -4; // EINTR
                }
                if !sleep_ms_interruptible(EVDEV_READ_POLL_MS) {
                    // No timer slot: yield rather than burn the CPU.
                    crate::sched::yield_cpu();
                }
            }
        }
    }
}

/// Backstop poll interval for the blocking read sleep.
const EVDEV_READ_POLL_MS: u64 = 10;

/// evdev close function
fn evdev_file_close(file: &File) -> i32 {
    // Release a grab held through this fd and free the DevNo boxed by
    // devfs_open.
    // SAFETY: private_data contains a valid DevNo pointer set during open.
    unsafe {
        if let Some(ptr) = *file.private_data.get() {
            let devno = *(ptr as *const DevNo);
            if devno == DEV_EVDEV_KEYBOARD {
                if let Some(ref d) = EVDEV_KEYBOARD {
                    d.grabbed.store(false, Ordering::Release);
                }
            } else if devno == DEV_EVDEV_POINTER {
                if let Some(ref d) = EVDEV_POINTER {
                    d.grabbed.store(false, Ordering::Release);
                }
            }
            drop(Box::from_raw(ptr as *mut DevNo));
        }
    }
    0
}

/// evdev poll function
///
/// Report readable whenever the queue holds an event (drain the virtio
/// queues first so a keystroke that has not raised an IRQ yet still wakes
/// the poller).
fn evdev_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;

    // SAFETY: private_data contains a valid DevNo pointer set during open;
    // the EVDEV_* statics are initialized by init_evdev() before any file
    // operations can occur.
    let device = match unsafe { device_of_file(file) } {
        Some(d) => d,
        None => return POLLERR,
    };

    let mut ready = 0u16;
    if events & POLLIN != 0 {
        // Drain the virtio queues so recently-arrived input counts.
        poll_virtio_events();
        if device.has_event() {
            ready |= POLLIN | POLLRDNORM;
        }
    }
    if events & POLLOUT != 0 {
        // evdev is never writable.
    }
    ready
}

/// Event type constants from <uapi/linux/input.h> needed by the write path.
const EV_SYN: u16 = 0x00;
const EV_LED: u16 = 0x11;

/// evdev write function
///
/// Linux semantics (drivers/input/evdev.c evdev_write): the buffer must be
/// a positive multiple of sizeof(struct input_event); each record is handed
/// to the input core, which silently drops events the device cannot take
/// and keeps LED/SND state for EVIOCGLED/EVIOCGSND. X servers write EV_LED
/// + SYN_REPORT packets here whenever core keyboard LED state changes —
/// returning an error (the old no-write-op EBADF) made every
/// xf86-input-evdev LED update log "(EE) Failed to set keyboard controls:
/// Bad file descriptor".
fn evdev_file_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: private_data contains a valid DevNo pointer set during device
    // open; the EVDEV_* statics are initialized by init_evdev() before any
    // file operations can occur.
    let device = match unsafe { device_of_file(file) } {
        Some(d) => d,
        None => return -9, // EBADF
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if buf.is_empty() || buf.len() % event_size != 0 {
        return -22; // EINVAL
    }

    for chunk in buf.chunks(event_size) {
        // InputEvent is repr(C) with no padding: the byte view parses
        // in place.
        let (sec, rest) = chunk.split_at(8);
        let (usec, rest) = rest.split_at(8);
        let (tycode, val) = rest.split_at(4);
        let type_ = u16::from_ne_bytes([tycode[0], tycode[1]]);
        let code = u16::from_ne_bytes([tycode[2], tycode[3]]);
        let value = i32::from_ne_bytes([val[0], val[1], val[2], val[3]]);
        let _ = (sec, usec); // client-supplied timestamps are advisory

        if type_ == EV_LED && code < 32 {
            if value != 0 {
                device.led_state.fetch_or(1 << code, Ordering::AcqRel);
            } else {
                device.led_state.fetch_and(!(1 << code), Ordering::AcqRel);
            }
        }
        // Everything else (including the trailing EV_SYN/SYN_REPORT) needs
        // no action: there is no hardware to inject into, and Linux's input
        // core equally ignores events without a capable handler.
    }
    buf.len() as isize
}

/// evdev FileOps
pub static EVDEV_OPS: FileOps = FileOps {
    read: Some(evdev_file_read),
    write: Some(evdev_file_write),
    lseek: None,
    close: Some(evdev_file_close),
    poll: Some(evdev_file_poll),
};

// ============================================================================
// ioctl (per-file, dispatched from sys_ioctl via ops identity)
// ============================================================================

/// evdev ioctl entry — returns None when `file` is not an evdev node so the
/// generic ioctl path can continue.
///
/// Mirrors drivers/input/evdev.c: data-returning ioctls succeed with 0
/// (put_user/copy_to_user style), unknown requests get ENOTTY.
pub fn evdev_file_ioctl(file: &File, cmd: u32, arg: usize) -> Option<i64> {
    // Dispatch on the file's ops identity (pty_ioctl / is_fb_file pattern).
    if !file.get_ops().map_or(false, |ops| {
        core::ptr::eq(ops as *const _, &EVDEV_OPS as *const _)
    }) {
        return None;
    }

    // SAFETY: the EVDEV_* statics are initialized by init_evdev() before
    // any file operations can occur; device_of_file validates the DevNo.
    let device = match unsafe { device_of_file(file) } {
        Some(d) => d,
        None => return Some(-9), // EBADF
    };

    /// Copy `data` out honoring the length encoded in the ioctl command.
    /// Returns the ioctl return value.
    fn copy_out(arg: usize, cmd: u32, data: &[u8]) -> i64 {
        use crate::arch::uaccess::{access_ok, copy_to_user};
        let len = ioc_size(cmd).min(data.len());
        if len == 0 {
            return 0;
        }
        if !access_ok(arg, len) {
            return -14; // EFAULT
        }
        // SAFETY: arg validated with access_ok(len); data is a kernel buffer.
        if unsafe { copy_to_user(arg as *mut u8, data.as_ptr(), len) } > 0 {
            return -14; // EFAULT
        }
        0
    }

    let nr = ioc_nr(cmd);
    let dir = ioc_dir(cmd);

    let ret: i64 = match (nr, dir) {
        (NR_VERSION, IOC_READ) => copy_out(arg, cmd, &0x010001u32.to_ne_bytes()), // EV_VERSION
        (NR_ID, IOC_READ) => copy_out(
            arg,
            cmd,
            &{
                // SAFETY: InputId is repr(C), Copy; transmute-free byte view.
                let id = device.id;
                let mut b = [0u8; 8];
                b[0..2].copy_from_slice(&id.bustype.to_ne_bytes());
                b[2..4].copy_from_slice(&id.vendor.to_ne_bytes());
                b[4..6].copy_from_slice(&id.product.to_ne_bytes());
                b[6..8].copy_from_slice(&id.version.to_ne_bytes());
                b
            },
        ),
        (NR_NAME, IOC_READ) => {
            // NUL-terminated device name, truncated to the caller's buffer.
            let len = ioc_size(cmd);
            let name_len = device.name.iter().position(|&c| c == 0).unwrap_or(31) + 1;
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &device.name[..name_len.min(32)])
            }
        }
        (NR_PHYS, IOC_READ) | (NR_UNIQ, IOC_READ) => {
            // No physical path / unique id on virtio-input: empty string.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &[0u8])
            }
        }
        (NR_PROP, IOC_READ) => {
            // No INPUT_PROP_* flags.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &[0u8; 32][..len.min(32)])
            }
        }
        (NR_REP_GET, IOC_READ) => {
            // Repeat settings {delay, period} in ms (kernel defaults).
            let rep = [250u32, 33u32];
            let mut b = [0u8; 8];
            b[0..4].copy_from_slice(&rep[0].to_ne_bytes());
            b[4..8].copy_from_slice(&rep[1].to_ne_bytes());
            copy_out(arg, cmd, &b)
        }
        (NR_REP_SET, IOC_WRITE) => 0,
        (NR_KEY_STATE, IOC_READ) | (NR_LED_STATE, IOC_READ) | (NR_SND_STATE, IOC_READ)
        | (NR_SW_STATE, IOC_READ) => {
            // Live state bitmap. LED bits track EV_LED writes (Linux
            // input-core semantics); key/sound/switch are not tracked and
            // report "all clear" — advisory, as on any kernel without
            // stateful drivers.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                let state = if nr == NR_LED_STATE {
                    device.led_state.load(Ordering::Acquire)
                } else {
                    0
                };
                let mut b = [0u8; 32];
                b[..4].copy_from_slice(&state.to_ne_bytes());
                copy_out(arg, cmd, &b[..len.min(32)])
            }
        }
        (NR_GRAB, IOC_WRITE) => {
            // arg is the grab flag passed by value (not a pointer).
            if arg != 0 {
                if device.grabbed.swap(true, Ordering::AcqRel) {
                    // Already grabbed by another client.
                    -16 // EBUSY
                } else {
                    0
                }
            } else {
                device.grabbed.store(false, Ordering::Release);
                0
            }
        }
        (NR_SCLOCKID, IOC_WRITE) => {
            // Accept the clock switch request; timestamps already come from
            // the monotonic CLINT timer.
            0
        }
        _ => {
            if (NR_BIT_BASE..NR_BIT_BASE + 0x20).contains(&nr) && dir == IOC_READ {
                // EVIOCGBIT(ev, len) for ev in 0..=0x1f (EV_MAX). Types we
                // carry bitmaps for answer from the virtio config; the rest
                // return an all-zero bitmap (success) like Linux — libevdev
                // probes every type and treats ENOTTY as an error.
                let ev = (nr - NR_BIT_BASE) as usize;
                if ev == 0 {
                    let types = evtype_bitmap(&device.ev_bits);
                    copy_out(arg, cmd, &types)
                } else if ev < BIT_TYPES {
                    copy_out(arg, cmd, &device.ev_bits[ev])
                } else {
                    let len = ioc_size(cmd);
                    if len == 0 {
                        0
                    } else {
                        copy_out(arg, cmd, &[0u8; 32][..len.min(32)])
                    }
                }
            } else if (NR_ABS_BASE..NR_ABS_BASE + 8).contains(&nr) && dir == IOC_READ {
                // EVIOCGABS(abs)
                let abs = (nr - NR_ABS_BASE) as usize;
                let info = if abs < 2 {
                    device.absinfo[abs]
                } else {
                    InputAbsinfo::default()
                };
                let mut b = [0u8; 24];
                b[0..4].copy_from_slice(&info.value.to_ne_bytes());
                b[4..8].copy_from_slice(&info.minimum.to_ne_bytes());
                b[8..12].copy_from_slice(&info.maximum.to_ne_bytes());
                b[12..16].copy_from_slice(&info.fuzz.to_ne_bytes());
                b[16..20].copy_from_slice(&info.flat.to_ne_bytes());
                b[20..24].copy_from_slice(&info.resolution.to_ne_bytes());
                copy_out(arg, cmd, &b)
            } else {
                -25 // ENOTTY
            }
        }
    };
    Some(ret)
}

// ============================================================================
// Initialization and registration
// ============================================================================

/// Initialize evdev devices and register to devfs
pub fn init_evdev() {
    // SAFETY: Called once during kernel init; no concurrent access is possible.
    unsafe {
        // Create keyboard device
        EVDEV_KEYBOARD = Some(EvdevDevice::new(b"VirtIO Keyboard", false));

        // Create pointer device
        EVDEV_POINTER = Some(EvdevDevice::new(b"VirtIO Tablet", true));

        // Pull the capability bitmaps straight from the virtio devices'
        // config space (what a Linux guest would register), with the
        // previously-hardcoded bitmaps as fallback when the config read
        // yields nothing.
        if let Some(ref mut dev) = EVDEV_KEYBOARD {
            fill_capabilities(dev, &super::INPUT_KEYBOARD, false);
        }
        if let Some(ref mut dev) = EVDEV_POINTER {
            fill_capabilities(dev, &super::INPUT_POINTER, true);
        }
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

    // U3: registration-time sysfs entries + "add" uevents (DEVNAME is
    // "input/event0"/"input/event1" — the /dev-relative node path).
    crate::fs::sysfs::register_input_event(
        "event0",
        DEV_EVDEV_KEYBOARD.major,
        DEV_EVDEV_KEYBOARD.minor,
    );
    crate::fs::sysfs::register_input_event(
        "event1",
        DEV_EVDEV_POINTER.major,
        DEV_EVDEV_POINTER.minor,
    );
}

/// Fill an evdev device's capability bitmaps from the backing virtio
/// device (locks the INPUT_* static and reads its config space).
fn fill_capabilities(
    dev: &mut EvdevDevice,
    virt_dev: &Spinlock<Option<super::VirtioInputDevice>>,
    is_pointer: bool,
) {
    let mut bits = [[0u8; BITS_LEN]; BIT_TYPES];
    let mut have_any = false;
    {
        let mut guard = virt_dev.lock();
        if let Some(ref mut vd) = *guard {
            for ev in 0..BIT_TYPES {
                if vd.read_ev_bitmap(ev as u8, &mut bits[ev]) > 0 {
                    have_any = true;
                }
            }
            // Absolute axes: read ABS_INFO for X/Y.
            if let Some([min, max, fuzz, flat, res]) = vd.read_absinfo(ABS_X as u8) {
                dev.set_absinfo(0, min as i32, max as i32, fuzz as i32, flat as i32, res as i32);
            }
            if let Some([min, max, fuzz, flat, res]) = vd.read_absinfo(ABS_Y as u8) {
                dev.set_absinfo(1, min as i32, max as i32, fuzz as i32, flat as i32, res as i32);
            }
        }
    }

    if !have_any {
        // Config space unreadable: report what the driver actually emits.
        bits[0][0] = 1; // EV_SYN/SYN_REPORT
        if is_pointer {
            // BTN_LEFT/RIGHT/MIDDLE = 0x110..0x112
            bits[1][0x110 / 8] = 0b0000_0111;
            bits[2][0] = 0x03; // REL_X | REL_Y
            bits[2][1] = 0x01; // REL_WHEEL (0x08)
            bits[3][0] = 0x03; // ABS_X | ABS_Y
            dev.set_absinfo(0, 0, 32767, 0, 0, 0);
            dev.set_absinfo(1, 0, 32767, 0, 0, 0);
        } else {
            // Standard keyboard keys 0x01..=0x58.
            for code in 1u16..=0x58 {
                bits[1][(code / 8) as usize] |= 1 << (code % 8);
            }
        }
    }

    for (ev, b) in bits.iter().enumerate() {
        dev.set_ev_bits(ev, b);
    }
}

/// Push event to evdev device
pub fn push_input_event(is_pointer: bool, event: InputEvent) {
    // Keyboard events feed the Ctrl-Alt-Del detector (SA_CAD semantics):
    // track Ctrl/Alt state and latch on a Delete press with both held.
    // Called from the evdev read/poll path — task context — but latching
    // only (atomic stores) keeps it safe from any context; execution
    // happens in cad_deliver_pending() on the next return to user.
    if !is_pointer {
        cad_detect(event);
    }
    // SAFETY: EVDEV_KEYBOARD and EVDEV_POINTER are initialized by init_evdev()
    // before any events can be pushed.
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
// Ctrl-Alt-Del detection (virtio-keyboard)
// ============================================================================

/// Modifier state for the CAD detector (bit flags, IRQ-safe atomics not
/// needed: push_input_event callers hold no shared state and the worst
/// race is a missed/extra detection on a simultaneous key event).
static CAD_MOD_CTRL: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static CAD_MOD_ALT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Track Ctrl/Alt and latch a Ctrl-Alt-Del: a Delete PRESS while at least
/// one Ctrl and one Alt are down (left or right either). Value semantics
/// per input.h: 0 = release, 1 = press, 2 = autorepeat (repeat counts —
/// a held Delete keeps telling the user wants out).
fn cad_detect(event: InputEvent) {
    if event.type_ != EV_KEY {
        return;
    }
    match event.code {
        super::event::KEY_LEFTCTRL | super::event::KEY_RIGHTCTRL => {
            CAD_MOD_CTRL.store(event.value != 0, core::sync::atomic::Ordering::Release);
        }
        super::event::KEY_LEFTALT | super::event::KEY_RIGHTALT => {
            CAD_MOD_ALT.store(event.value != 0, core::sync::atomic::Ordering::Release);
        }
        super::event::KEY_DELETE if event.value != 0 => {
            if CAD_MOD_CTRL.load(core::sync::atomic::Ordering::Acquire)
                && CAD_MOD_ALT.load(core::sync::atomic::Ordering::Acquire)
            {
                crate::syscall::process::ctrl_alt_del_latch();
            }
        }
        _ => {}
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
