//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tiny RCU — Read-Copy-Update for non-preemptible kernels
//!
//! rcu_read_lock() = preempt_disable()
//! rcu_read_unlock() = preempt_enable()
//!
//! Grace period: all CPUs must pass through at least one quiescent state
//! (context switch, idle loop, or return to user mode).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::config::MAX_CPUS;
use crate::list::ListHead;

// ============================================================================
// Types
// ============================================================================

/// RCU callback function type.
/// `data` points to the `RcuHead` that was passed to `call_rcu`.
pub type RcuCallback = fn(*mut core::ffi::c_void);

/// Embedded RCU callback head.
///
/// Place at the end of a struct to enable deferred reclamation via `call_rcu`.
/// The callback receives the `RcuHead` pointer; use `ListHead::entry()` or
/// `container_of!` to recover the enclosing struct.
#[repr(C)]
pub struct RcuHead {
    /// Intrusive list link for per-CPU callback queue.
    pub list: ListHead,
    /// Callback invoked after grace period.
    pub func: RcuCallback,
}

impl RcuHead {
    pub const fn new() -> Self {
        Self {
            list: ListHead::new(),
            func: |_| {},
        }
    }

    pub fn init(&mut self) {
        self.list.init();
    }
}

// ============================================================================
// Per-CPU state
// ============================================================================

/// Per-CPU callback list head (static, zero-initialized, `init()` called at boot).
static mut RCU_CBS: [ListHead; MAX_CPUS] = {
    const INIT: ListHead = ListHead::new();
    [INIT; MAX_CPUS]
};

/// Per-CPU lock for the callback list (simple TAS spinlock).
static RCU_CBS_LOCK: [AtomicBool; MAX_CPUS] = [
    const { AtomicBool::new(false) },
    const { AtomicBool::new(false) },
    const { AtomicBool::new(false) },
    const { AtomicBool::new(false) },
];

// ============================================================================
// Grace-period generation counter
// ============================================================================

/// Global generation counter. Incremented when any CPU's softirq finishes
/// processing a batch of callbacks.
static RCU_GEN: AtomicU32 = AtomicU32::new(1);

/// Per-CPU quiescent-state generation. Records the most recent `RCU_GEN`
/// value this CPU reported via `rcu_note_context_switch()`.
static RCU_QS_GEN: [AtomicU32; MAX_CPUS] = [
    const { AtomicU32::new(0) },
    const { AtomicU32::new(0) },
    const { AtomicU32::new(0) },
    const { AtomicU32::new(0) },
];

// ============================================================================
// Public API: read-side
// ============================================================================

/// Enter an RCU read-side critical section (preempt_disable).
#[inline]
pub fn rcu_read_lock() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

/// Exit an RCU read-side critical section (preempt_enable).
#[inline]
pub fn rcu_read_unlock() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

// ============================================================================
// Public API: callback registration
// ============================================================================

/// Register an RCU callback. After all CPUs pass through a quiescent state,
/// `head.func` will be called with `head` as the argument from softirq context.
///
/// # Safety
/// - `head` must remain valid until the callback executes.
/// - `head.func` must be a valid function pointer.
/// - `head.list` must have been initialized via `init()`.
pub unsafe fn call_rcu(head: *mut RcuHead) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    lock_cb_list(cpu);
    (*head).list.add_tail(&mut RCU_CBS[cpu]);
    unlock_cb_list(cpu);

    // Raise softirq so callbacks get processed promptly.
    crate::interrupt::softirq::raise_softirq(
        crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
    );
}

// ============================================================================
// Public API: quiescent-state reporting
// ============================================================================

/// Report a quiescent state for the current CPU.
///
/// Call from: `__schedule()`, `cpu_idle_loop()`, and return-to-user path.
#[inline]
pub fn rcu_note_context_switch() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < MAX_CPUS {
        let gen = RCU_GEN.load(Ordering::Acquire);
        RCU_QS_GEN[cpu].store(gen, Ordering::Release);
    }
}

// ============================================================================
// Public API: grace-period wait
// ============================================================================

/// Wait for a full grace period.
///
/// Must be called from process context (preempt_count == 0).
pub fn synchronize_rcu() {
    let gen = RCU_GEN.load(Ordering::Acquire);

    // Report our own quiescent state.
    rcu_note_context_switch();

    // Ensure the local softirq gets a chance to run.
    crate::interrupt::softirq::raise_softirq(
        crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
    );

    loop {
        // Fast path: generation already advanced past our snapshot.
        if RCU_GEN.load(Ordering::Acquire) > gen {
            return;
        }

        // Check every online CPU has reported a QS at or after `gen`.
        let mut all_qs = true;
        for i in 0..MAX_CPUS {
            if !crate::sched::sched::cpu_online(i) {
                continue;
            }
            if RCU_QS_GEN[i].load(Ordering::Acquire) < gen {
                all_qs = false;
                break;
            }
        }

        if all_qs {
            // All CPUs reported QS but softirq hasn't advanced the counter yet.
            // Nudge the local softirq.
            crate::interrupt::softirq::raise_softirq(
                crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
            );
        }

        core::hint::spin_loop();
    }
}

// ============================================================================
// Softirq handler
// ============================================================================

/// RCU softirq handler — drains the local CPU callback list.
///
/// Registered at `SoftirqIndex::Rcu` (vector 9).
pub fn rcu_softirq_handler(_nr: usize) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Detach the entire callback list under lock.
    let mut batch = ListHead::new();
    {
        lock_cb_list(cpu);
        unsafe {
            if RCU_CBS[cpu].is_empty() {
                unlock_cb_list(cpu);
                return;
            }
            // Splice the list into `batch`.
            batch.next = RCU_CBS[cpu].next;
            batch.prev = RCU_CBS[cpu].prev;
            (*batch.next).prev = &mut batch;
            (*batch.prev).next = &mut batch;
            RCU_CBS[cpu].init();
        }
        unlock_cb_list(cpu);
    }

    // Invoke all callbacks.
    unsafe {
        let sentinel = &mut batch as *mut ListHead;
        let mut node = batch.next;
        while node != sentinel {
            let next = (*node).next;
            let head = node as *mut RcuHead;
            let func = (*head).func;
            func(head as *mut core::ffi::c_void);
            node = next;
        }
    }

    // Advance global generation so `synchronize_rcu()` callers can make progress.
    RCU_GEN.fetch_add(1, Ordering::AcqRel);
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize the RCU subsystem (called once during boot).
pub fn init() {
    for i in 0..MAX_CPUS {
        unsafe { RCU_CBS[i].init(); }
        RCU_QS_GEN[i].store(0, Ordering::Relaxed);
    }

    crate::interrupt::softirq::open_softirq(
        crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
        rcu_softirq_handler,
    );

    crate::pr_info!("rcu: tiny RCU initialized (max {} CPUs)", MAX_CPUS);
}

// ============================================================================
// Internal helpers
// ============================================================================

#[inline]
fn lock_cb_list(cpu: usize) {
    while RCU_CBS_LOCK[cpu]
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[inline]
fn unlock_cb_list(cpu: usize) {
    RCU_CBS_LOCK[cpu].store(false, Ordering::Release);
}
