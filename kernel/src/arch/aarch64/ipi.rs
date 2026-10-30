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

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::config::MAX_CPUS;

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
    fn bit(self) -> u32 {
        1u32 << self as u8 as u32
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
    let idx = ipi_type as u8 as usize;
    // SAFETY: IPI_HANDLERS is a static mutable array written only
    // during boot init (before SGIs can arrive: the trap dispatch path
    // that calls handle_software_ipi only runs with DAIF unmasked,
    // which single-threaded boot has not done yet at ipi::init time).
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
/// Snapshots and clears the pending bitmap, then dispatches each set
/// bit LSB-first to its handler.
pub fn handle_software_ipi(_cpu: usize) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    let pending = IPI_PENDING[cpu].swap(0, Ordering::AcqRel);
    if pending == 0 {
        return;
    }

    let mut bits = pending;
    while bits != 0 {
        let idx = bits.trailing_zeros() as usize;
        if idx < NR_IPI_TYPES {
            // SAFETY: handler table is fully initialized in init()
            // before SGIs can be delivered (write-once, see request_ipi).
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
static mut CSD_QUEUES: [ListHead; MAX_CPUS] = {
    const INIT: ListHead = ListHead::new();
    [INIT; MAX_CPUS]
};

/// Per-CPU writer locks for the CSD queues.
static CSD_LOCKS: [Spinlock<()>; MAX_CPUS] = [
    Spinlock::new(()),
    Spinlock::new(()),
    Spinlock::new(()),
    Spinlock::new(()),
];

fn csd_init() {
    for i in 0..MAX_CPUS {
        // SAFETY: CSD_QUEUES is a static mutable array touched once
        // during single-threaded boot init, before any IPI can arrive.
        unsafe {
            CSD_QUEUES[i].init();
        }
    }
}

/// Call a function on a remote CPU and wait for completion.
///
/// Safe under BKL (the csd is Box-pinned; the wait yields — see the
/// riscv64 twin's perf-gate note on spin-waiting with preemption off).
pub fn smp_call_function(
    target: usize,
    func: fn(*mut core::ffi::c_void),
    info: *mut core::ffi::c_void,
) {
    if target >= MAX_CPUS {
        return;
    }

    let current_cpu = crate::arch::cpu_id() as usize;
    if target == current_cpu {
        func(info);
        return;
    }

    let mut csd = Box::new(CallSingleData {
        func,
        info,
        list: ListHead::new(),
        done: AtomicBool::new(false),
    });

    {
        let _lock = CSD_LOCKS[target].lock_irqsave();
        // SAFETY: we hold CSD_LOCKS[target], serializing all mutations
        // of CSD_QUEUES[target]; csd is Box-pinned and stays alive until
        // done is observed.
        unsafe {
            csd.list.init();
            if CSD_QUEUES[target].is_empty() {
                CSD_QUEUES[target].next = &mut csd.list as *mut _;
                CSD_QUEUES[target].prev = &mut csd.list as *mut _;
                csd.list.next = &mut CSD_QUEUES[target] as *mut _;
                csd.list.prev = &mut CSD_QUEUES[target] as *mut _;
            } else {
                let tail = CSD_QUEUES[target].prev;
                (*tail).next = &mut csd.list as *mut _;
                csd.list.prev = tail;
                csd.list.next = &mut CSD_QUEUES[target] as *mut _;
                CSD_QUEUES[target].prev = &mut csd.list as *mut _;
            }
        }
    }

    send_ipi_type(target, IpiType::CallFunction);

    while !csd.done.load(Ordering::Acquire) {
        crate::sched::cond_resched();
        core::hint::spin_loop();
    }
}

/// Drain this CPU's CSD queue (CallFunction IPI handler).
fn csd_flush_queue() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    let mut head: *mut ListHead;
    {
        let _lock = CSD_LOCKS[cpu].lock_irqsave();
        // SAFETY: under CSD_LOCKS[cpu]; detaching and re-initializing
        // the queue head is exclusive to the lock holder.
        unsafe {
            if CSD_QUEUES[cpu].is_empty() {
                return;
            }
            head = CSD_QUEUES[cpu].next;
            CSD_QUEUES[cpu].init();
        }
    }

    // SAFETY: queue_ptr is a stable static address; the detached list
    // is exclusively ours until completion; senders spin on done until
    // after we store true, so no Box is freed under us.
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
// Handlers
// ============================================================================

fn ipi_reschedule_handler() {
    // Only set the flag — schedule() from hard-IRQ context could
    // deadlock on the GRQ lock with IRQs off (riscv64 twin note). The
    // trap-return / cond_resched points pick it up.
    crate::sched::set_need_resched();
}

fn ipi_call_function_handler() {
    csd_flush_queue();
}

fn ipi_stop_handler() {
    loop {
        // SAFETY: WFI halts the core until an interrupt; this CPU was
        // told to stop, so parking forever is the intended behavior.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

fn ipi_wakeup_handler() {
    // WFI exit is the wake itself; nothing further to run.
}

fn ipi_tlb_flush_handler() {
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
