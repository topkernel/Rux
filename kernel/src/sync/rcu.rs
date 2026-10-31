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
    /// Whether this head has been initialized (list.init() called).
    initialized: bool,
    /// Intrusive list link for per-CPU callback queue.
    pub list: ListHead,
    /// Callback invoked after grace period.
    pub func: RcuCallback,
}

impl RcuHead {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            list: ListHead::new(),
            func: |_| {},
        }
    }

    pub fn init(&mut self) {
        self.list.init();
        self.initialized = true;
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

    // Guard against uninitialized RcuHead — if init() was never called,
    // add_tail would corrupt the list.
    if !(*head).initialized {
        return;
    }

    lock_cb_list(cpu);
    // SAFETY: head is a valid pointer (caller contract).  We hold cb_list lock
    // for this CPU, so RCU_CBS[cpu] is not accessed concurrently.  head.list
    // was initialised via init() (caller contract).
    (*head).list.add_tail(&mut RCU_CBS[cpu]);
    unlock_cb_list(cpu);

    // Raise softirq so callbacks get processed promptly.
    crate::interrupt::softirq::raise_softirq(
        crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
    );
}

/// Number of tasks blocked inside `synchronize_rcu()`. When zero — the
/// overwhelmingly common state — the QS report fast-path below costs one
/// relaxed load and stops there, keeping the per-syscall tax on the
/// user-return path at that single load (perf gate: p1/p2 must not move).
static RCU_WAITERS: AtomicU32 = AtomicU32::new(0);

// ============================================================================
// Public API: quiescent-state reporting
// ============================================================================

/// Report a quiescent state for the current CPU.
///
/// Call from: `__schedule()`, `cpu_idle_loop()`, and the return-to-user
/// path (the common trap tails). The user-return hookup is what makes
/// grace periods complete on syscall-heavy CPUs without waiting for
/// their scheduler tick: before it existed, a CPU spinning in user
/// syscalls never reported a QS and every `synchronize_rcu()` waiter
/// (e.g. the MmStruct drop on every process exit) stalled for a full
/// time slice (~10 ms) — the measured 38x p4_fork_exit gap.
#[inline]
pub fn rcu_note_context_switch() {
    if RCU_WAITERS.load(Ordering::Relaxed) == 0 {
        return;
    }
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

    // Announce the waiter BEFORE snapshotting/reporting so every
    // user-return from here on reports its QS (see RCU_WAITERS).
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);

    // Report our own quiescent state.
    rcu_note_context_switch();

    loop {
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
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        // Yield the CPU instead of busy-spinning.  A bare spin_loop()
        // deadlocks on UP (no other task can produce quiescent states)
        // and wastes CPU time on SMP.  schedule() lets other tasks run
        // so they can context-switch and report their quiescent states.
        crate::sched::schedule();
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
        // SAFETY: We hold cb_list lock for this CPU, so RCU_CBS[cpu] is only
        // accessed by us.  The list pointers were initialised in init().
        unsafe {
            if !RCU_CBS[cpu].is_empty() {
                // Splice the list into `batch`.
                batch.next = RCU_CBS[cpu].next;
                batch.prev = RCU_CBS[cpu].prev;
                (*batch.next).prev = &mut batch;
                (*batch.prev).next = &mut batch;
                RCU_CBS[cpu].init();
            }
        }
        unlock_cb_list(cpu);
    }

    // Invoke all callbacks.
    // SAFETY: We detached the entire callback list above and now own it
    // exclusively.  Each RcuHead was registered via call_rcu with a valid
    // func pointer and remains valid until the callback executes.
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
        // SAFETY: Called once during boot before any CPU uses RCU — no
        // concurrent access to RCU_CBS[i].
        unsafe { RCU_CBS[i].init(); }
        RCU_QS_GEN[i].store(0, Ordering::Relaxed);
    }

    crate::interrupt::softirq::open_softirq(
        crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
        rcu_softirq_handler,
    );

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
