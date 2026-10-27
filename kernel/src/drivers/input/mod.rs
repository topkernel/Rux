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
// IRQ-driven event delivery (producer wake for virtio-input)
// ============================================================================
//
// Before this wiring the event flow was pull-only: events moved off the
// virtqueues into the evdev buffers solely inside evdev read/poll
// dispatches. A waiter that BLOCKS has no drain trigger of its own —
// push_event (the evdev poll_waitq's only waker) runs at drain time, and
// nobody drains. Xorg's input thread blocks exactly like that (epoll_wait
// with no timeout, registered on the evdev poll_waitq by the P3 wait
// path), so keystrokes sat in the used ring forever: the r13live session
// logged zero INPTRC for 1500 s while the desktop kept rendering, while
// timeout-based pollers (inputcat's 5 s loops) got 14/14 events. The IRQ
// path closes the loop the way Linux's virtio-input does it: the device
// interrupt schedules the input tasklet, which drains both virtqueues
// and delivers through push_input_event, waking every
// poll/select/epoll waiter the moment an event lands.

/// Input bottom-half tasklet. Armed (init + enable) once during
/// virtio-input probe, scheduled by the hard-IRQ top half.
// SAFETY: touched from the probe (single-threaded init context) and from
// tasklet_schedule in hard-IRQ context, which is the API's designed use.
static mut INPUT_BH_TASKLET: crate::interrupt::tasklet::TaskletStruct =
    crate::interrupt::tasklet::TaskletStruct::new();

/// dev_id → ISR capability address for the registered input functions
/// (0 = keyboard, 1 = pointer). Stored before the IRQ registration that
/// can dispatch against them, read lock-free in the hard-IRQ top half
/// (same discipline as virtio-blk's ISR read).
static INPUT_ISR_BARS: [core::sync::atomic::AtomicU64; 2] =
    [core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0)];

/// Input tasklet bottom half: drain both event queues and deliver.
///
/// Collect/deliver split (INV-LOCK-6 discipline, same shape as
/// virtio-blk's block_bh_handler): events are popped off the used rings
/// under the device locks — read_event also re-posts each buffer to the
/// avail ring, so the device can keep streaming — and pushed into the
/// evdev buffers (which wakes the poll/select/epoll waiters) only after
/// every device lock is dropped.
fn input_bh_run(_t: *mut crate::interrupt::tasklet::TaskletStruct) {
    const BURST: usize = 16;
    const ROUNDS: usize = 8;
    for _ in 0..ROUNDS {
        let mut kbd = [InputEvent::default(); BURST];
        let mut n_kbd = 0usize;
        let mut ptr = [InputEvent::default(); BURST];
        let mut n_ptr = 0usize;
        {
            // SAFETY: INPUT_KEYBOARD is initialized before the IRQ
            // registration that can schedule this tasklet.
            let mut guard = INPUT_KEYBOARD.lock_irqsave();
            if let Some(ref mut dev) = *guard {
                while n_kbd < BURST {
                    match dev.read_event() {
                        Some(e) => {
                            kbd[n_kbd] = e;
                            n_kbd += 1;
                        }
                        None => break,
                    }
                }
            }
        }
        {
            // SAFETY: INPUT_POINTER is initialized before the IRQ
            // registration that can schedule this tasklet.
            let mut guard = INPUT_POINTER.lock_irqsave();
            if let Some(ref mut dev) = *guard {
                while n_ptr < BURST {
                    match dev.read_event() {
                        Some(e) => {
                            ptr[n_ptr] = e;
                            n_ptr += 1;
                        }
                        None => break,
                    }
                }
            }
        }
        if n_kbd == 0 && n_ptr == 0 {
            break; // both queues drained
        }
        for e in &kbd[..n_kbd] {
            evdev::push_input_event(false, *e);
        }
        for e in &ptr[..n_ptr] {
            evdev::push_input_event(true, *e);
        }
    }
}

/// Hard-IRQ top half for a virtio-input PCI function (riscv64: the gpex
/// swizzled INTx line, possibly shared with virtio-blk or the sibling
/// input function). Reads the ISR capability FIRST — per the virtio spec
/// that read drops the device's level-triggered line (the DRIV-H3
/// discipline) — then defers all virtqueue work to the input tasklet: no
/// queue lock and no wait-queue wake in hard-IRQ context (the same split
/// as virtio-blk's interrupt_handler_pci).
#[cfg(feature = "riscv64")]
fn virtio_input_irq_handler(_irq: u32, dev_id: usize) -> crate::interrupt::IrqReturn {
    use core::sync::atomic::Ordering;
    let idx = if dev_id == 0 { 0 } else { 1 };
    let isr_bar = INPUT_ISR_BARS[idx].load(Ordering::Relaxed);
    if isr_bar == 0 {
        return crate::interrupt::IrqReturn::None;
    }
    // SAFETY: isr_bar was stored from a probed device's ISR capability
    // before this handler was registered; the volatile read is the
    // device-side EOI.
    let isr = unsafe { core::ptr::read_volatile(isr_bar as *const u32) };
    if isr == 0 {
        // Not ours — the shared line was raised by another function.
        return crate::interrupt::IrqReturn::None;
    }
    // SAFETY: INPUT_BH_TASKLET is initialized by register_input_irq()
    // before any IRQ registration that can reach this point, and a
    // static tasklet struct lives forever.
    unsafe {
        crate::interrupt::tasklet::tasklet_schedule(core::ptr::addr_of_mut!(
            INPUT_BH_TASKLET
        ));
    }
    crate::interrupt::IrqReturn::Handled
}

/// x86_64 INTx service (registered through the shared virtio INTx
/// dispatcher, which has already read the ISR — dropping the line — and
/// only calls us when it is nonzero).
#[cfg(feature = "x86_64")]
fn input_intx_service() {
    // SAFETY: INPUT_BH_TASKLET is initialized by register_input_irq()
    // before the INTx registration that can reach this point.
    unsafe {
        crate::interrupt::tasklet::tasklet_schedule(core::ptr::addr_of_mut!(
            INPUT_BH_TASKLET
        ));
    }
}

/// Arm the input tasklet and hook the device's interrupt line. Called
/// from init_virtio_input() once per probed function, after the device
/// is stored in its INPUT_* static and before any event can be waited
/// on (`dev_id` 0 = keyboard, 1 = pointer).
pub fn register_input_irq(dev: &VirtioInputDevice, dev_id: usize) {
    let idx = if dev_id == 0 { 0 } else { 1 };
    INPUT_ISR_BARS[idx].store(dev.isr_cfg_bar(), core::sync::atomic::Ordering::Release);

    #[cfg(feature = "riscv64")]
    {
        let irq = dev.intx_irq_line();
        let _ = crate::interrupt::request_irq(
            irq,
            virtio_input_irq_handler,
            crate::interrupt::irqdesc::IRQF_SHARED,
            "virtio-input",
            dev_id,
        );
    }
    #[cfg(feature = "x86_64")]
    {
        let _ = dev.enable_pci_intx_irq(input_intx_service);
    }
}

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

    // Arm the bottom-half tasklet before any device IRQ is registered.
    // SAFETY: single-threaded kernel-init context; the tasklet is not
    // scheduled anywhere yet.
    unsafe {
        INPUT_BH_TASKLET.init(input_bh_run);
    }

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

    // Initialize evdev devices (capability bitmaps are read under the
    // device locks with plain guards — safe because the device IRQs are
    // not hooked yet).
    evdev::init_evdev();

    // IRQ registration LAST: once the line is hooked, the tasklet BH can
    // drain the queues on any virtio-input interrupt, and every INPUT_*
    // guard from here on must be irqsave (the evdev read/poll paths and
    // the BH are; nothing between the store above and this point is).
    // The lookup guards here are irqsave too: once the FIRST function is
    // registered its BH can preempt a plain holder of the second lock.
    if let Some(ref dev) = *INPUT_KEYBOARD.lock_irqsave() {
        register_input_irq(dev, 0);
    }
    if let Some(ref dev) = *INPUT_POINTER.lock_irqsave() {
        register_input_irq(dev, 1);
    }

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
