//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::config::MAX_CPUS;
use crate::sbi;

// ============================================================================
// IPI types
// ============================================================================

/// Number of IPI types
pub const NR_IPI_TYPES: usize = 5;

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
    /// TLB flush — full sfence.vma on the target (remote shootdown,
    /// review 4.10: sfence.vma is hart-local, unmap paths must IPI peers)
    TlbFlush = 4,
}

impl IpiType {
    pub(crate) fn bit(self) -> u32 {
        1u32 << self as u8
    }
}

// ============================================================================
// Per-CPU pending bitmap
// ============================================================================

/// Per-CPU IPI pending bitmap. Bit N corresponds to IpiType variant N.
pub(crate) static IPI_PENDING: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

// ============================================================================
// Handler table
// ============================================================================

/// IPI handler table (write-once during init, read-only thereafter).
pub(crate) static mut IPI_HANDLERS: [Option<fn()>; NR_IPI_TYPES] = [None; NR_IPI_TYPES];
/// Register an IPI handler. Write-once — panics on double registration.
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-17): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn request_ipi(ipi_type: IpiType, handler: fn()) {
    crate::arch::arch_floor::riscv64::ipi::request_ipi(ipi_type, handler)
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

/// Broadcast a full-TLB-flush IPI to every OTHER started CPU.
///
/// `sfence.vma` only invalidates on the issuing hart, so any CPU that may
/// have cached translations of pages we just unmapped must be told to
/// flush (review 4.10). One IPI per batch unmap — the handler performs a
/// full `sfence.vma zero, zero`, so coalescing/dedup of the pending bit
/// (see send_ipi_type) never loses a flush: any in-flight request covers
/// every prior and concurrent batch.
///
/// Safe to call from any context; returns immediately (fire-and-forget —
/// remote harts flush asynchronously).
pub fn flush_tlb_others() {
    let me = crate::arch::cpu_id() as usize;
    let online = crate::arch::smp::num_started_cpus().max(1);
    for cpu in 0..core::cmp::min(online, MAX_CPUS) {
        if cpu != me {
            send_ipi_type(cpu, IpiType::TlbFlush);
        }
    }
}

// ============================================================================
// Receive / dispatch
// ============================================================================
/// Handle a software IPI interrupt on the given hart.
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-16): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn handle_software_ipi(_hart: usize) {
    crate::arch::arch_floor::riscv64::ipi::handle_software_ipi(_hart)
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
pub(crate) static mut CSD_QUEUES: [ListHead; MAX_CPUS] = {
    const INIT: ListHead = ListHead::new();
    [INIT; MAX_CPUS]
};

/// Per-CPU writer lock for CSD queue (protects list_add_tail).
pub(crate) static CSD_LOCKS: [Spinlock<()>; MAX_CPUS] = [const { Spinlock::new(()) }; MAX_CPUS];
/// Initialize CSD queues. Called once during boot.
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-15): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn csd_init() {
    crate::arch::arch_floor::riscv64::ipi::csd_init()
}
/// Call a function on a remote CPU and wait for completion.
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-14): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn smp_call_function(target: usize, func: fn(*mut core::ffi::c_void), info: *mut core::ffi::c_void) {
    crate::arch::arch_floor::riscv64::ipi::smp_call_function(target, func, info)
}
/// Drain the per-CPU CSD queue (called by CallFunction IPI handler).
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-13): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn csd_flush_queue() {
    crate::arch::arch_floor::riscv64::ipi::csd_flush_queue()
}

// ============================================================================
// IPI handlers (registered during init)
// ============================================================================

pub(crate) fn ipi_reschedule_handler() {
    // Only set the need_resched flag.  Do NOT call schedule() here —
    // this runs in interrupt context with IRQs disabled.  Calling
    // schedule() would try to acquire the GRQ spinlock, and if
    // another CPU already holds it, this CPU deadlocks with IRQs
    // off (timer ticks can't fire → softlockup).
    //
    // The return-from-trap path checks need_resched and calls
    // schedule() at a safe point.
    crate::sched::set_need_resched();
}

pub(crate) fn ipi_call_function_handler() {
    csd_flush_queue();
}
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-12): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn ipi_stop_handler() {
    crate::arch::arch_floor::riscv64::ipi::ipi_stop_handler()
}

pub(crate) fn ipi_irq_work_handler() {
    // Placeholder — no users yet
}
/// TLB shootdown handler: full flush of all non-global translations on
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn ipi_tlb_flush_handler() {
    crate::arch::arch_floor::riscv64::ipi::ipi_tlb_flush_handler()
}

// ============================================================================
// Legacy IRQ handler (for PLIC IRQ 11-13)
// ============================================================================
/// Handle PLIC IPI (IRQ handler registered via request_irq).
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn ipi_irq_handler(irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    crate::arch::arch_floor::riscv64::ipi::ipi_irq_handler(irq, _dev_id)
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
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-9): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn handle_ipi(irq: usize, hart: usize) {
    crate::arch::arch_floor::riscv64::ipi::handle_ipi(irq, hart)
}

// ============================================================================
// Initialization
// ============================================================================
/// Initialize IPI subsystem.
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-8): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init() {
    crate::arch::arch_floor::riscv64::ipi::init()
}
/// Per-hart SSIE enable for secondary harts (perf-sched lane, 2026-10-31).
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn per_hart_enable_ssoftware() {
    crate::arch::arch_floor::riscv64::ipi::per_hart_enable_ssoftware()
}
