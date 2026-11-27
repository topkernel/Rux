#![forbid(unsafe_code)]
//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tasklet subsystem
//!
//! Tasklets are a dynamic bottom-half mechanism built on top of softirq.
//! Two priority levels: HI_SOFTIRQ (highest) and TASKLET_SOFTIRQ (normal).

// (INV-TASKLET-1: the tasklet family — the intrusive list protocol is
// F6-registered (safety-and-performance); the clone-to-claim rows
// ride the per-CPU list lock and the count's Release publication.)
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::config::MAX_CPUS;
use crate::list::ListHead;
use core::mem::offset_of;

// ============================================================================
// Tasklet state bits
// ============================================================================

/// Tasklet is scheduled (queued on a per-CPU list)
pub(super) const TASKLET_STATE_SCHED: u32 = 0;
/// Tasklet is currently running
pub(super) const TASKLET_STATE_RUN: u32 = 1;

// ============================================================================
// TaskletStruct
// ============================================================================

/// Tasklet descriptor
///
/// Embedded in driver-private structures. The `list` field links
/// tasklets into per-CPU queues.
#[repr(C)]
pub struct TaskletStruct {
    /// Intrusive list node for per-CPU queue linkage
    pub list: ListHead,
    /// State bitmask (TASKLET_STATE_SCHED, TASKLET_STATE_RUN)
    pub(super) state: AtomicU32,
    /// Disable count. Tasklet runs only when count == 0.
    count: AtomicU32,
    /// The callback function
    pub(super) func: Option<fn(*mut TaskletStruct)>,
}

impl TaskletStruct {
    /// Create a new tasklet (disabled).
    /// Initially disabled; call `enable()` before scheduling.
    pub const fn new() -> Self {
        Self {
            list: ListHead::new(),
            state: AtomicU32::new(0),
            count: AtomicU32::new(1), // disabled by default
            func: None,
        }
    }

    /// Create a tasklet with a callback.
    pub fn with_func(func: fn(*mut TaskletStruct)) -> Self {
        Self {
            list: ListHead::new(),
            state: AtomicU32::new(0),
            count: AtomicU32::new(0), // enabled
            func: Some(func),
        }
    }

    /// Initialize the tasklet at runtime.
    pub fn init(&mut self, func: fn(*mut TaskletStruct)) {
        self.list.init();
        self.state.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
        self.func = Some(func);
    }

    /// Check if tasklet is scheduled
    #[inline]
    pub fn is_scheduled(&self) -> bool {
        (self.state.load(Ordering::Acquire) & (1 << TASKLET_STATE_SCHED)) != 0
    }

    /// Check if tasklet is currently running
    #[inline]
    pub fn is_running(&self) -> bool {
        (self.state.load(Ordering::Acquire) & (1 << TASKLET_STATE_RUN)) != 0
    }

    /// Check if tasklet is disabled
    #[inline]
    pub fn is_disabled(&self) -> bool {
        self.count.load(Ordering::Acquire) != 0
    }

    /// Enable the tasklet (decrement disable count).
    /// If it was already scheduled, re-raise TASKLET_SOFTIRQ.
    pub fn enable(&self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
        if self.is_scheduled() {
            crate::interrupt::softirq::raise_softirq(
                crate::interrupt::softirq::SoftirqIndex::Tasklet as usize
            );
        }
    }

    /// Disable the tasklet (increment disable count).
    pub fn disable(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }
}

// ============================================================================
// Per-CPU tasklet lists
// ============================================================================

/// Per-CPU tasklet list head for normal priority (TASKLET_SOFTIRQ).
pub(super) static mut TASKLET_VEC: [ListHead; MAX_CPUS] = [const { ListHead::new() }; MAX_CPUS];

/// Per-CPU tasklet list head for high priority (HI_SOFTIRQ).
pub(super) static mut TASKLET_HI_VEC: [ListHead; MAX_CPUS] = [const { ListHead::new() }; MAX_CPUS];

/// Per-CPU spinlocks protecting tasklet list manipulation.
pub(super) static TASKLET_LOCK: [Spinlock<()>; MAX_CPUS] = [const { Spinlock::new(()) }; MAX_CPUS];

pub(super) static TASKLET_HI_LOCK: [Spinlock<()>; MAX_CPUS] = [const { Spinlock::new(()) }; MAX_CPUS];

// ============================================================================
// Scheduling
// ============================================================================

/// Schedule a tasklet for execution.
/// Adds to the per-CPU TASKLET_SOFTIRQ list and raises TASKLET_SOFTIRQ.
pub fn tasklet_schedule(t: *mut TaskletStruct) {
    // Raw-pointer edge lives in the audited unsafe.rs (INV-IRQ-TASKLET-PTR-1).
    super::edges::tasklet_schedule(t);

    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Tasklet as usize
    );
}

/// Schedule a high-priority tasklet.
/// Adds to the per-CPU HI_SOFTIRQ list and raises HI_SOFTIRQ.
pub fn tasklet_hi_schedule(t: *mut TaskletStruct) {
    // Raw-pointer edge lives in the audited unsafe.rs (INV-IRQ-TASKLET-PTR-1).
    super::edges::tasklet_hi_schedule(t);

    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Hi as usize
    );
}

// ============================================================================
// Kill
// ============================================================================

/// Kill a tasklet: ensure it is not scheduled and wait for it to finish.
/// Used during driver teardown.
pub fn tasklet_kill(t: *mut TaskletStruct) {
    // Raw-pointer edge lives in the audited unsafe.rs (INV-IRQ-TASKLET-PTR-1).
    super::edges::tasklet_kill(t);
}

// ============================================================================
// Softirq action handlers
// ============================================================================

// splice_all / drain_tasklet_list (the intrusive splice and the
// container_of walk) live in the audited unsafe.rs
// (INV-IRQ-TASKLET-VEC-1 / INV-IRQ-TASKLET-DRAIN-1).

/// TASKLET_SOFTIRQ action handler: detach the per-CPU list, then drain.
fn tasklet_action(_vec: usize) {
    let cpu = crate::arch::cpu_id() as usize;

    // 1. Detach the entire list under lock
    let mut local_head = ListHead::new();
    local_head.init();
    {
        let _lock = TASKLET_LOCK[cpu].lock_irqsave();
        super::edges::splice_tasklet_vec(&mut local_head, cpu);
    }

    // 2. Process local list (no lock needed — local to this context)
    super::edges::drain_tasklet_list(&mut local_head);
}

/// HI_SOFTIRQ action handler — identical path on the HI vector.
fn tasklet_hi_action(_vec: usize) {
    let cpu = crate::arch::cpu_id() as usize;

    let mut local_head = ListHead::new();
    local_head.init();
    {
        let _lock = TASKLET_HI_LOCK[cpu].lock_irqsave();
        super::edges::splice_tasklet_hi_vec(&mut local_head, cpu);
    }

    super::edges::drain_tasklet_list(&mut local_head);
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize the tasklet subsystem.
/// Registers tasklet_action and tasklet_hi_action as softirq handlers.
pub fn init() {
    // Init per-CPU list heads (edge in the audited unsafe.rs,
    // INV-IRQ-TASKLET-VEC-1)
    super::edges::tasklet_vecs_init();

    // Register softirq handlers
    crate::interrupt::softirq::open_softirq(
        crate::interrupt::softirq::SoftirqIndex::Tasklet as usize,
        tasklet_action,
    );
    crate::interrupt::softirq::open_softirq(
        crate::interrupt::softirq::SoftirqIndex::Hi as usize,
        tasklet_hi_action,
    );
}
