//! RISC-V IPI (Inter-Processor Interrupt) support
//!
//! Bitmap-multiplexed IPI: each CPU has an AtomicU32 pending bitmap.
//! A single SBI IPI sets bits; the handler snapshots-and-clears and
//! dispatches each set bit to the registered callback.
//!
//! IPI types:
//! - RESCHEDULE:  Notify target CPU to reschedule
//! - CALL_FUNCTION: Execute a callback on target CPU
//! - STOP:        Halt target CPU
//! - IRQ_WORK:    Deferred IRQ work (placeholder)

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::config::MAX_CPUS;
use crate::sbi;

// ============================================================================
// IPI types
// ============================================================================

/// Number of IPI types
pub const NR_IPI_TYPES: usize = 4;

/// IPI type enumeration (bit positions in pending bitmap)
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum IpiType {
    /// Reschedule — set need_resched + schedule()
    Reschedule = 0,
    /// Call Function — drain CSD queue on target CPU
    CallFunction = 1,
    /// Stop — halt target CPU in WFI loop
    Stop = 2,
    /// IRQ work — deferred work (placeholder)
    IrqWork = 3,
}

impl IpiType {
    fn bit(self) -> u32 {
        1u32 << self as u8
    }
}

// ============================================================================
// Per-CPU pending bitmap
// ============================================================================

/// Per-CPU IPI pending bitmap. Bit N corresponds to IpiType variant N.
static IPI_PENDING: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

// ============================================================================
// Handler table
// ============================================================================

/// IPI handler table (write-once during init, read-only thereafter).
static mut IPI_HANDLERS: [Option<fn()>; NR_IPI_TYPES] = [None; NR_IPI_TYPES];

/// Register an IPI handler. Write-once — panics on double registration.
pub fn request_ipi(ipi_type: IpiType, handler: fn()) {
    let idx = ipi_type as usize;
    unsafe {
        if IPI_HANDLERS[idx].is_some() {
            panic!("IPI handler already registered for {:?}", ipi_type);
        }
        IPI_HANDLERS[idx] = Some(handler);
    }
}

// ============================================================================
// Send IPI
// ============================================================================

/// Send an IPI of the given type to the target CPU.
///
/// Atomically sets the pending bit and issues an SBI IPI.
/// Safe to call from any context (IRQ, softirq, task).
pub fn send_ipi_type(target: usize, ipi_type: IpiType) {
    if target >= MAX_CPUS {
        return;
    }

    let bit = ipi_type.bit();

    // Set pending bit (if already set, the SBI IPI is already in flight)
    let prev = IPI_PENDING[target].fetch_or(bit, Ordering::Release);
    if prev & bit != 0 {
        // Bit was already set — SBI IPI already sent, no need to resend
        return;
    }

    // Send SBI IPI to target
    let _ = sbi::send_ipi(target);
}

/// Send Reschedule IPI (backward-compatible convenience wrapper).
pub fn send_reschedule_ipi(target_cpu: usize) {
    send_ipi_type(target_cpu, IpiType::Reschedule);
}

// ============================================================================
// Receive / dispatch
// ============================================================================

/// Handle a software IPI interrupt on the given hart.
///
/// Called from the IRQ framework. Snapshots and clears the pending
/// bitmap, then dispatches each set bit to its handler in order.
pub fn handle_software_ipi(_hart: usize) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Atomically snapshot and clear all pending bits
    let pending = IPI_PENDING[cpu].swap(0, Ordering::AcqRel);
    if pending == 0 {
        return;
    }

    // Dispatch LSB-first (lowest IPI type has highest priority)
    let mut bits = pending;
    while bits != 0 {
        let idx = bits.trailing_zeros() as usize;
        if idx < NR_IPI_TYPES {
            unsafe {
                if let Some(handler) = IPI_HANDLERS[idx] {
                    handler();
                }
            }
        }
        bits &= bits - 1; // clear lowest set bit
    }
}

// ============================================================================
// smp_call_function
// ============================================================================

use crate::list::ListHead;
use alloc::boxed::Box;

/// Callback data for cross-CPU function calls.
///
/// Allocated on the caller's stack or heap, linked into the target CPU's
/// callback queue, and completed via the `done` flag.
#[repr(C)]
pub struct CallSingleData {
    /// Callback function
    pub func: fn(*mut core::ffi::c_void),
    /// Opaque argument
    pub info: *mut core::ffi::c_void,
    /// Intrusive list link
    pub list: ListHead,
    /// Completion flag — set to true by target CPU after callback runs
    pub done: AtomicBool,
}

impl CallSingleData {
    pub const fn new() -> Self {
        Self {
            func: |_| {},
            info: core::ptr::null_mut(),
            list: ListHead::new(),
            done: AtomicBool::new(false),
        }
    }
}

/// Per-CPU callback queues. Each target CPU drains its own queue.
static mut CSD_QUEUES: [ListHead; MAX_CPUS] = {
    const INIT: ListHead = ListHead::new();
    [INIT; MAX_CPUS]
};

/// Per-CPU writer lock for CSD queue (protects list_add_tail).
static CSD_LOCKS: [spin::Mutex<()>; MAX_CPUS] = [
    spin::Mutex::new(()),
    spin::Mutex::new(()),
    spin::Mutex::new(()),
    spin::Mutex::new(()),
];

/// Initialize CSD queues. Called once during boot.
fn csd_init() {
    for i in 0..MAX_CPUS {
        unsafe {
            CSD_QUEUES[i].init();
        }
    }
}

/// Call a function on a remote CPU and wait for completion.
///
/// Safe to call under BKL. The caller must ensure `func` is safe to
/// execute on the target CPU with the given `info` argument.
pub fn smp_call_function(target: usize, func: fn(*mut core::ffi::c_void), info: *mut core::ffi::c_void) {
    if target >= MAX_CPUS {
        return;
    }

    let current_cpu = crate::arch::cpu_id() as usize;
    if target == current_cpu {
        // Local call — execute directly
        func(info);
        return;
    }

    let mut csd = Box::new(CallSingleData {
        func,
        info,
        list: ListHead::new(),
        done: AtomicBool::new(false),
    });

    // Enqueue on target CPU's callback queue
    {
        let _lock = CSD_LOCKS[target].lock();
        unsafe {
            csd.list.init();
            if CSD_QUEUES[target].is_empty() {
                // First entry — set up circular list
                CSD_QUEUES[target].next = &mut csd.list as *mut _;
                CSD_QUEUES[target].prev = &mut csd.list as *mut _;
                csd.list.next = &mut CSD_QUEUES[target] as *mut _;
                csd.list.prev = &mut CSD_QUEUES[target] as *mut _;
            } else {
                // Insert at tail
                let tail = CSD_QUEUES[target].prev;
                unsafe {
                    (*tail).next = &mut csd.list as *mut _;
                    csd.list.prev = tail;
                    csd.list.next = &mut CSD_QUEUES[target] as *mut _;
                    CSD_QUEUES[target].prev = &mut csd.list as *mut _;
                }
            }
        }
    }

    // Send CallFunction IPI
    send_ipi_type(target, IpiType::CallFunction);

    // Spin-wait for completion (safe under BKL)
    while !csd.done.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // csd dropped here — but we leaked it into the queue above.
    // The CallFunction handler must set done=true AND unlink before we return.
    // Since we spin-wait, the handler has already finished.
    // Leak the Box to prevent double-free — the handler already ran the callback.
    Box::leak(csd);
}

/// Drain the per-CPU CSD queue (called by CallFunction IPI handler).
fn csd_flush_queue() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Detach entire list under lock
    let mut head: *mut ListHead;
    {
        let _lock = CSD_LOCKS[cpu].lock();
        unsafe {
            if CSD_QUEUES[cpu].is_empty() {
                return;
            }
            head = CSD_QUEUES[cpu].next;
            // Re-init queue head to empty
            CSD_QUEUES[cpu].init();
        }
    }

    // Walk detached list, call each callback
    let queue_ptr = unsafe { &CSD_QUEUES[cpu] as *const _ as *mut ListHead };
    let mut node = head;
    while node != queue_ptr {
        unsafe {
            let csd = node as *mut CallSingleData;
            let next = (*node).next;
            let f = (*csd).func;
            let arg = (*csd).info;
            f(arg);
            (*csd).done.store(true, Ordering::Release);
            node = next;
        }
    }
}

// ============================================================================
// IPI handlers (registered during init)
// ============================================================================

fn ipi_reschedule_handler() {
    crate::sched::set_need_resched();
    crate::sched::schedule();
}

fn ipi_call_function_handler() {
    csd_flush_queue();
}

fn ipi_stop_handler() {
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

fn ipi_irq_work_handler() {
    // Placeholder — no users yet
}

// ============================================================================
// Legacy IRQ handler (for PLIC IRQ 11-13)
// ============================================================================

/// Handle PLIC IPI (IRQ handler registered via request_irq).
///
/// IRQ 11 = software IPI (reschedule / call_function / etc.)
/// IRQ 12, 13 = stop (legacy, for backward compat)
fn ipi_irq_handler(irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    let hart = crate::arch::cpu_id() as usize;
    match irq {
        11 => {
            handle_software_ipi(hart);
        }
        12 | 13 => {
            // Legacy stop
            loop {
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
        _ => {}
    }
    crate::interrupt::IrqReturn::Handled
}

/// Register IPI handlers via the IRQ framework.
/// Called during init after the PLIC domain is created.
pub fn register_irq_handlers() {
    for irq in 11..14u32 {
        crate::interrupt::request_irq(
            irq,
            ipi_irq_handler,
            crate::interrupt::IRQF_SHARED,
            "IPI",
            0,
        ).ok();
    }
}

/// Legacy IPI handler (kept for compatibility with direct calls)
pub fn handle_ipi(irq: usize, hart: usize) {
    match irq {
        11 => {
            handle_software_ipi(hart);
        }
        12 | 13 => {
            loop {
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
        _ => {}
    }
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize IPI subsystem.
///
/// - Enables software interrupts (SSIP)
/// - Registers IPI handlers
/// - Initializes CSD queues
pub fn init() {
    // Initialize CSD queues
    csd_init();

    // Register IPI type handlers
    request_ipi(IpiType::Reschedule, ipi_reschedule_handler);
    request_ipi(IpiType::CallFunction, ipi_call_function_handler);
    request_ipi(IpiType::Stop, ipi_stop_handler);
    request_ipi(IpiType::IrqWork, ipi_irq_work_handler);

    // Enable software interrupt
    unsafe {
        core::arch::asm!(
            "csrsi sie, 2",  // Set bit 1 (SSIE = 0x2)
            options(nomem, nostack)
        );
    }

    // Register PLIC IRQ handlers
    register_irq_handlers();
}
