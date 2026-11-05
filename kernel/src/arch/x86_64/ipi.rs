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
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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
/// The remote handler performs a full non-global flush (INVPCID
/// all-contexts with PCID on, CR3 reload otherwise), so there is no
/// per-page state to race with and nothing to coalesce.  Fire-and-forget:
/// the caller must have flushed its own TLB entry(ies) locally (the mm
/// callers pair this with invlpg/flush_tlb).
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
            // kernel PTEs carry the GLOBAL bit, so this covers
            // everything the local invlpg/flush_tlb covered. With PCID
            // active the CR3 reload would only reach the CURRENT PCID's
            // entries, so the flush is INVPCID all-contexts (every PCID
            // on this CPU) — the shootdown callers invalidate mappings
            // that may be cached under any mm's PCID.
            if crate::arch::mm::asid::pcid_enabled() {
                // SAFETY: type 3 ignores the descriptor content and is
                // valid regardless of the current CR3.
                unsafe {
                    crate::arch::x86_64::cpu::invpcid(
                        crate::arch::x86_64::cpu::INVPCID_ALL_NON_GLOBAL,
                        0,
                        0,
                    )
                };
            } else {
                // SAFETY: reloading the current CR3 with PCID off.
                unsafe { crate::arch::cpu::write_cr3(crate::arch::cpu::read_cr3()) };
            }
        }
        VECTOR_STOP => loop {
            crate::arch::cpu::disable_irq();
            // HLT parks this CPU forever (panic path); WFI's SAFETY
            // contract holds by construction here.
            crate::arch::cpu::wfi();
        },
        VECTOR_IRQ_WORK => snapshot_self(),
        _ => {}
    }
}

// ============================================================================
// SP-5 round 5: peer LAPIC-timer snapshot probe (frozen-tick trap)
// ============================================================================
//
// The OOM-FORENSIC trap needs the OTHER CPU's live LAPIC state, but x2APIC
// registers are per-CPU MSRs readable only in person. This IRQ_WORK
// handler captures this CPU's snapshot into shared slots and publishes a
// sequence number; the requester polls `SNAP_DONE == its request` with a
// timebase-bounded wait (the tick may be dead on either side, so jiffies
// must not be used). A timeout is itself evidence: that CPU is not
// servicing interrupts at all.

/// Per-CPU snapshot request counter (requester bumps; handler echoes).
pub static SNAP_REQ: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Last request sequence serviced by that CPU (0 = none yet).
pub static SNAP_DONE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Captured live state (valid once SNAP_DONE echoes the request).
pub static SNAP_LVT: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub static SNAP_TMICT: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub static SNAP_TMCCT: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub static SNAP_SVR: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub static SNAP_TPR: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub static SNAP_IRR7: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub static SNAP_ISR7: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
/// Arm-trail seq observed by the target at snapshot time.
pub static SNAP_ARMSEQ: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Timebase value at the snapshot (age reference for the requester).
pub static SNAP_TB: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// IRQ_WORK handler body: capture THIS CPU's LAPIC timer state.
fn snapshot_self() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }
    let req = SNAP_REQ[cpu].load(Ordering::Acquire);
    if let Some(s) = crate::drivers::intc::apic::lapic_timer_snapshot() {
        SNAP_LVT[cpu].store(s.lvt, Ordering::Relaxed);
        SNAP_TMICT[cpu].store(s.tmict, Ordering::Relaxed);
        SNAP_TMCCT[cpu].store(s.tmcct, Ordering::Relaxed);
        SNAP_SVR[cpu].store(s.svr, Ordering::Relaxed);
        SNAP_TPR[cpu].store(s.tpr, Ordering::Relaxed);
        SNAP_IRR7[cpu].store(s.irr7, Ordering::Relaxed);
        SNAP_ISR7[cpu].store(s.isr7, Ordering::Relaxed);
        SNAP_ARMSEQ[cpu].store(
            crate::drivers::intc::apic::TIMER_ARM_SEQ[cpu].load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        SNAP_TB[cpu].store(crate::drivers::timer::read_time(), Ordering::Relaxed);
    }
    SNAP_DONE[cpu].store(req, Ordering::Release);
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

    csd_fire(target, &mut csd.list as *mut ListHead);

    // Wait for completion. Perf gate (2026-10-20): this used to be a
    // pure spin — and with no kernel preemption, a caller waiting on a
    // target CPU stuck in a long preempt-disabled section (e.g. fork's
    // COW page-table walk) monopolized ITS OWN CPU too, so one walk on
    // CPU-A plus one membarrier on CPU-B froze BOTH. Yielding here is
    // safe: the csd is Box-pinned (not stack), nothing holds the CSD
    // lock at this point, and the memory stays valid until done is
    // observed (sleeping tasks keep their frames; the Box frees below).
    while !csd.done.load(Ordering::Acquire) {
        crate::sched::cond_resched();
        core::hint::spin_loop();
    }

    // csd_flush_queue() has already run the callback, set done=true,
    // and detached the queue. The csd.list pointers may still reference
    // other nodes in the detached list, but ListHead has no Drop impl,
    // so dropping the Box simply deallocates the CallSingleData.
}

/// Enqueue a CSD on the target's queue and fire the CALL_FUNCTION IPI.
///
/// Shared by [`smp_call_function`] and the bounded diagnostic variant
/// [`smp_call_function_timeout`]. `csd_list` must point at a stable
/// (Box-pinned or leaked) CallSingleData's `list` field — the address
/// the target's queue links; the container is recovered on the target
/// by the list-offset back-adjustment in csd_flush_queue.
fn csd_fire(target: usize, csd_list: *mut ListHead) {
    // Enqueue on target CPU's callback queue.  irqsafe: the IPI handler
    // on the target CPU dequeues with the same lock.
    {
        let _lock = CSD_LOCKS[target].lock_irqsave();
        // SAFETY: We hold CSD_LOCKS[target], which serializes all mutations
        // to CSD_QUEUES[target]. The csd_list pointer stays valid until
        // `done` is set (caller contract, same as smp_call_function).
        unsafe {
            (*csd_list).init();
            if CSD_QUEUES[target].is_empty() {
                // First entry — set up circular list
                CSD_QUEUES[target].next = csd_list;
                CSD_QUEUES[target].prev = csd_list;
                (*csd_list).next = &mut CSD_QUEUES[target] as *mut _;
                (*csd_list).prev = &mut CSD_QUEUES[target] as *mut _;
            } else {
                // Insert at tail
                let tail = CSD_QUEUES[target].prev;
                // SAFETY: tail was read from the queue under lock and points to a valid
                // ListHead node in the queue. The lock prevents concurrent modification.
                unsafe {
                    (*tail).next = csd_list;
                    (*csd_list).prev = tail;
                    (*csd_list).next = &mut CSD_QUEUES[target] as *mut _;
                    CSD_QUEUES[target].prev = csd_list;
                }
            }
        }
    }
    send_ipi_type(target, IpiType::CallFunction);
}

/// Bounded diagnostic cross-CPU call — the frozen-CPU truth probe form
/// (52nd session, rcu-gp-frozen hunt). Same enqueue+IPI protocol as
/// [`smp_call_function`], but the wait is bounded: a target monopolized
/// with IRQs off (or wedged holding the CSD lock) cannot answer, and
/// the CALLER must survive that. Returns whether the callback ran. On
/// timeout the CSD stays linked and its memory is LEAKED —
/// deliberately: the node must outlive a target that may drain its
/// queue arbitrarily far in the future, and this is a one-shot-per-boot
/// diagnostic (a few dozen bytes).
pub fn smp_call_function_timeout(
    target: usize,
    func: fn(*mut core::ffi::c_void),
    info: *mut core::ffi::c_void,
    max_ms: u64,
) -> bool {
    if target >= MAX_CPUS {
        return false;
    }
    let current_cpu = crate::arch::cpu_id() as usize;
    if target == current_cpu {
        func(info);
        return true;
    }
    let csd = Box::into_raw(Box::new(CallSingleData {
        func,
        info,
        list: ListHead::new(),
        done: AtomicBool::new(false),
    }));
    // SAFETY: csd is a valid allocation we own; the list field's address
    // is stable for the lifetime of the Box (leaked on timeout).
    csd_fire(
        target,
        unsafe { &raw mut (*csd).list as *mut ListHead },
    );

    let hz = crate::drivers::timer::HZ as u64;
    let deadline = crate::drivers::timer::get_jiffies() + max_ms * hz / 1000;
    // SAFETY: csd is a valid allocation we own until done is observed;
    // done is set only by the target after detaching the node.
    while !unsafe { (*csd).done.load(Ordering::Acquire) } {
        if crate::drivers::timer::get_jiffies() >= deadline {
            return false; // node stays linked + leaked; see doc comment
        }
        crate::sched::cond_resched();
        core::hint::spin_loop();
    }
    // SAFETY: done was observed; the target no longer touches the node
    // (ListHead has no Drop; plain dealloc is correct).
    unsafe { drop(Box::from_raw(csd)) };
    true
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
