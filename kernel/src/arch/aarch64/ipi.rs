//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 IPI support — GICv3 SGI (INTID 0) bitmap multiplexing.
//!
//! The riscv64 twin's contract on SGI hardware: each CPU carries an
//! AtomicU32 pending bitmap; the sender sets a bit and raises ONE GIC
//! SGI (ICC_SGI1R_EL1); the receiving trap path (trap.rs IRQ dispatch,
//! INTID 0 special case) calls handle_software_ipi, which
//! snapshots-and-clears the bitmap and dispatches each set bit to its
//! registered callback. Coalescing is safe because every handler below
//! is a full-drain operation (schedule-pending check, CSD queue drain,
//! full TLB flush).
//!
//! SGI delivery is edge-triggered per raise; the pending bit dedups
//! in-flight sends (a second raise while a bit is already set is a
//! no-op — the receiver drains both).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch::aarch64::MAX_CPUS;

// ============================================================================
// IPI types
// ============================================================================

/// Number of IPI types
pub const NR_IPI_TYPES: usize = 5;

/// IPI type enumeration (bit positions in the per-CPU pending bitmap)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpiType {
    /// Reschedule — set need_resched
    Reschedule = 0,
    /// Call Function — drain the CSD queue on the target
    CallFunction = 1,
    /// TLB shootdown — full tlbi vmalle1 on the target
    TlbShootdown = 2,
    /// Wakeup — kick an idle (WFI) cpu so it re-checks its runqueue
    Wakeup = 3,
    /// Stop — halt the target in a WFI loop
    Stop = 4,
}

impl IpiType {
    pub(crate) fn bit(self) -> u32 {
        1u32 << self as u8 as u32
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
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn request_ipi(ipi_type: IpiType, handler: fn()) {
    crate::arch::arch_floor::aarch64::ipi::request_ipi(ipi_type, handler)
}

// ============================================================================
// Send IPI
// ============================================================================

/// Send an IPI of the given type to the target CPU.
///
/// Sets the pending bit, then raises GIC SGI 0 (INTID 0) on the target
/// through ICC_SGI1R_EL1. Safe from any context. The pending-bit check
/// dedups: if a bit is already set the SGI is already in flight and the
/// receiver's drain covers this request too.
pub fn send_ipi_type(target: usize, ipi_type: IpiType) {
    if target >= MAX_CPUS {
        return;
    }

    let bit = ipi_type.bit();
    let prev = IPI_PENDING[target].fetch_or(bit, Ordering::Release);
    if prev & bit != 0 {
        return; // SGI already in flight for this type
    }

    // QEMU virt: cpu N sits at MPIDR Aff0=N for N < 4 (see
    // smp::index_to_mpidr), so the SGI target list is bit N of Aff0.
    crate::drivers::intc::gicv3::send_sgi(
        target as u8,
        crate::drivers::intc::gicv3::IPI_SGI_INTID,
    );
}

/// Send a Reschedule IPI (convenience wrapper, twin name).
pub fn send_reschedule_ipi(target_cpu: usize) {
    send_ipi_type(target_cpu, IpiType::Reschedule);
}

/// Broadcast a full-TLB-flush IPI to every OTHER started CPU, then
/// flush locally (tlbi vmalle1 is core-local — remote cores that may
/// hold cached translations of pages we unmapped must invalidate too;
/// riscv64 review 4.10 twin). Fire-and-forget: handlers drain async.
pub fn flush_tlb_others() {
    crate::arch::cpu::local_flush_tlb_all();
    let me = crate::arch::cpu_id() as usize;
    let online = crate::arch::smp::num_started_cpus().max(1);
    for cpu in 0..core::cmp::min(online, MAX_CPUS) {
        if cpu != me {
            send_ipi_type(cpu, IpiType::TlbShootdown);
        }
    }
}

// ============================================================================
// Receive / dispatch
// ============================================================================
/// Handle the IPI SGI on this CPU (called from the trap IRQ dispatch).
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn handle_software_ipi(_cpu: usize) {
    crate::arch::arch_floor::aarch64::ipi::handle_software_ipi(_cpu)
}

// ============================================================================
// smp_call_function (CSD queues)
// ============================================================================

use crate::list::ListHead;
use alloc::boxed::Box;

/// Callback data for cross-CPU function calls (riscv64 twin shape).
#[repr(C)]
pub struct CallSingleData {
    pub func: fn(*mut core::ffi::c_void),
    pub info: *mut core::ffi::c_void,
    /// Intrusive list link
    pub list: ListHead,
    /// Set by the target CPU after the callback runs
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

/// Per-CPU callback queues; each target CPU drains its own.
pub(crate) static mut CSD_QUEUES: [ListHead; MAX_CPUS] = {
    const INIT: ListHead = ListHead::new();
    [INIT; MAX_CPUS]
};

/// Per-CPU writer locks for the CSD queues.
pub(crate) static CSD_LOCKS: [Spinlock<()>; MAX_CPUS] = [const { Spinlock::new(()) }; MAX_CPUS];
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn csd_init() {
    crate::arch::arch_floor::aarch64::ipi::csd_init()
}
/// Call a function on a remote CPU and wait for completion.
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn smp_call_function( target: usize, func: fn(*mut core::ffi::c_void), info: *mut core::ffi::c_void, ) {
    crate::arch::arch_floor::aarch64::ipi::smp_call_function(target, func, info)
}
/// Drain this CPU's CSD queue (CallFunction IPI handler).
///
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn csd_flush_queue() {
    crate::arch::arch_floor::aarch64::ipi::csd_flush_queue()
}

// ============================================================================
// Handlers
// ============================================================================

pub(crate) fn ipi_reschedule_handler() {
    // Only set the flag — schedule() from hard-IRQ context could
    // deadlock on the GRQ lock with IRQs off (riscv64 twin note). The
    // trap-return / cond_resched points pick it up.
    crate::sched::set_need_resched();
}

pub(crate) fn ipi_call_function_handler() {
    csd_flush_queue();
}
/// Collected into `arch::arch_floor` (INV-ARCH-IPI-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn ipi_stop_handler() {
    crate::arch::arch_floor::aarch64::ipi::ipi_stop_handler()
}

pub(crate) fn ipi_wakeup_handler() {
    // WFI exit is the wake itself; nothing further to run.
}

pub(crate) fn ipi_tlb_flush_handler() {
    // Full non-global invalidation covers every queued request (the
    // pending-bit coalescing in send_ipi_type relies on this).
    crate::arch::cpu::local_flush_tlb_all();
}

// ============================================================================
// IRQ-framework registration / init
// ============================================================================

/// IPI handler entry when reached through the framework path (kept for
/// interface parity with the riscv64 twin's PLIC-registered handler).
pub fn handle_ipi(_irq: usize, cpu: usize) {
    handle_software_ipi(cpu);
}

/// IRQ handler registration hook (twin name). The SGI arrives through
/// the GIC claim path's INTID-0 special case in trap.rs — nothing to
/// register with the framework; kept as the no-op seam.
pub fn register_irq_handlers() {}

/// Initialize the IPI subsystem.
pub fn init() {
    csd_init();

    request_ipi(IpiType::Reschedule, ipi_reschedule_handler);
    request_ipi(IpiType::CallFunction, ipi_call_function_handler);
    request_ipi(IpiType::TlbShootdown, ipi_tlb_flush_handler);
    request_ipi(IpiType::Wakeup, ipi_wakeup_handler);
    request_ipi(IpiType::Stop, ipi_stop_handler);

    // SGIs 0-15 are enabled + Group 1 in each CPU's redistributor
    // (gicv3::redist_configure_local); nothing further to unmask.
    crate::print_status("ipi", "GICv3 SGI 0 + bitmap multiplexing", true);
}
