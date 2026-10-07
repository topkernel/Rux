//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 IPIs over the local APIC: dedicated vectors per IPI type
//! (instead of the riscv64 twin's bitmap multiplexer — the x86 IDT has
//! plenty of vectors and skipping the pending-bit handshake halves the
//! delivery latency), plus the CSD (call-single-data) queue machinery
//! for `smp_call_function` and the TLB shootdown.
//!
//! IPI types and vectors:
//! - RESCHEDULE (0xE2): set need_resched on the target
//! - CALL_FUNCTION (0xE1): drain the CSD queue on the target
//! - TLB_FLUSH (0xE3): full non-global flush (CR3 reload) on the target
//! - STOP (0xE4): halt the target permanently (panic path)
//! - IRQ_WORK (0xE5): reserved (no users yet)

use crate::list::ListHead;
use crate::sync::spinlock::Spinlock;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::config::MAX_CPUS;

// ============================================================================
// Vectors (IDT stubs registered by trap.rs; LOCAL_TIMER=0xE0 is the
// APIC driver's)
// ============================================================================

pub const VECTOR_CALL_FUNCTION: u64 = 0xE1;
pub const VECTOR_RESCHEDULE: u64 = 0xE2;
pub const VECTOR_TLB_FLUSH: u64 = 0xE3;
pub const VECTOR_STOP: u64 = 0xE4;
pub const VECTOR_IRQ_WORK: u64 = 0xE5;

pub const IPI_FIRST: u64 = VECTOR_CALL_FUNCTION;
pub const IPI_LAST: u64 = VECTOR_IRQ_WORK;

// ============================================================================
// IPI types (interface parity with the riscv64 twin)
// ============================================================================

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum IpiType {
    Reschedule,
    CallFunction,
    Stop,
    IrqWork,
    TlbFlush,
}

impl IpiType {
    fn vector(self) -> u64 {
        match self {
            IpiType::Reschedule => VECTOR_RESCHEDULE,
            IpiType::CallFunction => VECTOR_CALL_FUNCTION,
            IpiType::Stop => VECTOR_STOP,
            IpiType::IrqWork => VECTOR_IRQ_WORK,
            IpiType::TlbFlush => VECTOR_TLB_FLUSH,
        }
    }
}

// ============================================================================
// Send
// ============================================================================

/// Send an IPI of the given type to the target CPU.  Safe to call from
/// any context; no-op when the target is offline or the APIC is down.
pub fn send_ipi_type(target: usize, ipi_type: IpiType) {
    if target >= MAX_CPUS {
        return;
    }
    let lapic = crate::drivers::intc::apic::lapic_id(target);
    if lapic == u32::MAX {
        return;
    }
    crate::drivers::intc::apic::send_ipi(lapic, ipi_type.vector() as u8);
}

/// Send Reschedule IPI (interface parity with the twin).
pub fn send_reschedule_ipi(target_cpu: usize) {
    send_ipi_type(target_cpu, IpiType::Reschedule);
}

/// Broadcast a TLB-flush IPI to every OTHER started CPU.
///
/// The remote handler performs a full non-global flush (CR3 reload with
/// PCID off), so there is no per-page state to race with and nothing to
/// coalesce.  Fire-and-forget: the caller must have flushed its own TLB
/// entry(ies) locally (the mm callers pair this with invlpg/flush_tlb).
pub fn flush_tlb_others(_start: u64, _end: u64) {
    let me = crate::arch::cpu_id() as usize;
    for cpu in 0..MAX_CPUS {
        if cpu != me && crate::arch::smp::cpu_started(cpu) {
            send_ipi_type(cpu, IpiType::TlbFlush);
        }
    }
}

// ============================================================================
// Receive / dispatch (called from trap.rs with the IPI vector)
// ============================================================================

/// Dispatch a received IPI vector.  Called from interrupt context.
pub fn handle_ipi_vector(vector: u64) {
    match vector {
        VECTOR_RESCHEDULE => {
            // Only set the flag.  Calling schedule() here (IRQs off)
            // could deadlock on the runqueue lock the same way the
            // riscv64 twin documents; the trap-exit path reschedules.
            crate::sched::set_need_resched();
        }
        VECTOR_CALL_FUNCTION => csd_flush_queue(),
        VECTOR_TLB_FLUSH => {
            // Full non-global flush of the CURRENT address space: no
            // kernel PTEs carry the GLOBAL bit, so the CR3 reload
            // covers everything the local invlpg/flush_tlb covered.
            // SAFETY: reloading the current CR3 with PCID off.
            unsafe { crate::arch::cpu::write_cr3(crate::arch::cpu::read_cr3()) };
        }
        VECTOR_STOP => loop {
            crate::arch::cpu::disable_irq();
            // HLT parks this CPU forever (panic path); WFI's SAFETY
            // contract holds by construction here.
            crate::arch::cpu::wfi();
        },
        VECTOR_IRQ_WORK => {}
        _ => {}
    }
}

// ============================================================================
// smp_call_function (CSD queues — port of the riscv64 twin)
// ============================================================================

/// Callback data for cross-CPU function calls.
///
/// Allocated on the caller's heap, linked into the target CPU's
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
static CSD_LOCKS: [Spinlock<()>; MAX_CPUS] = [
    Spinlock::new(()),
    Spinlock::new(()),
    Spinlock::new(()),
    Spinlock::new(()),
];

/// Initialize CSD queues. Called once during boot.
fn csd_init() {
    for i in 0..MAX_CPUS {
        // SAFETY: CSD_QUEUES is a static mutable array. Called only once during boot
        // init before any CSD operations, so no concurrent access.
        unsafe {
            CSD_QUEUES[i].init();
        }
    }
}

/// Call a function on a remote CPU and wait for completion.
///
/// Safe to call under BKL. The caller must ensure `func` is safe to
/// execute on the target CPU with the given `info` argument.
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

    // Enqueue on target CPU's callback queue.  irqsafe: the IPI handler
    // on the target CPU dequeues with the same lock.
    {
        let _lock = CSD_LOCKS[target].lock_irqsave();
        // SAFETY: We hold CSD_LOCKS[target], which serializes all mutations to
        // CSD_QUEUES[target]. The csd and queue pointers remain valid for the
        // duration of the lock since csd is pinned by the Box below us.
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
                // SAFETY: tail was read from the queue under lock and points to a valid
                // ListHead node in the queue. The lock prevents concurrent modification.
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

    // csd_flush_queue() has already run the callback, set done=true,
    // and detached the queue. The csd.list pointers may still reference
    // other nodes in the detached list, but ListHead has no Drop impl,
    // so dropping the Box simply deallocates the CallSingleData.
}

/// Drain the per-CPU CSD queue (CALL_FUNCTION IPI handler).
fn csd_flush_queue() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Detach entire list under lock
    let mut head: *mut ListHead;
    {
        let _lock = CSD_LOCKS[cpu].lock_irqsave();
        // SAFETY: We hold CSD_LOCKS[cpu], serializing access to CSD_QUEUES[cpu].
        // Detaching the list and re-initializing the queue head is safe under the lock.
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
    // SAFETY: queue_ptr is a stable address of a static. The list was detached under
    // lock above, so we have exclusive ownership of all nodes until completion.
    let queue_ptr = unsafe { &CSD_QUEUES[cpu] as *const _ as *mut ListHead };
    // The list link sits at an offset INSIDE CallSingleData: node→struct
    // requires the back-adjustment (a bare cast reads the link pointers
    // as func/info and calls the queue head).
    let list_off = core::mem::offset_of!(CallSingleData, list);
    let mut node = head;
    while node != queue_ptr {
        // SAFETY: node points into the detached list of CallSingleData entries.
        // Each node was allocated via Box::new and is valid. The sender is
        // spin-waiting on done, so the Box is not freed until after we store true.
        unsafe {
            let csd = (node as *mut u8).sub(list_off) as *mut CallSingleData;
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
// Initialization
// ============================================================================

/// Initialize IPI support (CSD queues; vectors are wired in trap.rs).
pub fn init() {
    csd_init();
}
