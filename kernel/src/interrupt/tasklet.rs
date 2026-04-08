//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tasklet subsystem
//!
//! Tasklets are a dynamic bottom-half mechanism built on top of softirq.
//! Two priority levels: HI_SOFTIRQ (highest) and TASKLET_SOFTIRQ (normal).

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::config::MAX_CPUS;
use crate::list::ListHead;
use core::mem::offset_of;

// ============================================================================
// Tasklet state bits
// ============================================================================

/// Tasklet is scheduled (queued on a per-CPU list)
const TASKLET_STATE_SCHED: u32 = 0;
/// Tasklet is currently running
const TASKLET_STATE_RUN: u32 = 1;

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
    state: AtomicU32,
    /// Disable count. Tasklet runs only when count == 0.
    count: AtomicU32,
    /// The callback function
    func: Option<fn(*mut TaskletStruct)>,
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
static mut TASKLET_VEC: [ListHead; MAX_CPUS] = [
    ListHead::new(), ListHead::new(),
    ListHead::new(), ListHead::new(),
];

/// Per-CPU tasklet list head for high priority (HI_SOFTIRQ).
static mut TASKLET_HI_VEC: [ListHead; MAX_CPUS] = [
    ListHead::new(), ListHead::new(),
    ListHead::new(), ListHead::new(),
];

/// Per-CPU spinlocks protecting tasklet list manipulation.
static TASKLET_LOCK: [Spinlock<()>; MAX_CPUS] = [
    Spinlock::new(()), Spinlock::new(()),
    Spinlock::new(()), Spinlock::new(()),
];

static TASKLET_HI_LOCK: [Spinlock<()>; MAX_CPUS] = [
    Spinlock::new(()), Spinlock::new(()),
    Spinlock::new(()), Spinlock::new(()),
];

// ============================================================================
// Scheduling
// ============================================================================

/// Schedule a tasklet for execution.
/// Adds to the per-CPU TASKLET_SOFTIRQ list and raises TASKLET_SOFTIRQ.
pub fn tasklet_schedule(t: *mut TaskletStruct) {
    // SAFETY: caller must pass a valid, live pointer to a TaskletStruct that
    // remains valid for the duration of this call (typical: embedded in a
    // driver-private structure allocated for the device lifetime).
    unsafe {
        let tasklet = &mut *t;
        // Already scheduled? Skip.
        if tasklet.state.fetch_or(
            1u32 << TASKLET_STATE_SCHED, Ordering::AcqRel
        ) & (1u32 << TASKLET_STATE_SCHED) != 0 {
            return;
        }

        let cpu = crate::arch::cpu_id() as usize;
        let _lock = TASKLET_LOCK[cpu].lock_irqsave();
        tasklet.list.add_tail(&mut TASKLET_VEC[cpu] as *mut ListHead);
    }

    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Tasklet as usize
    );
}

/// Schedule a high-priority tasklet.
/// Adds to the per-CPU HI_SOFTIRQ list and raises HI_SOFTIRQ.
pub fn tasklet_hi_schedule(t: *mut TaskletStruct) {
    // SAFETY: caller must pass a valid, live pointer to a TaskletStruct.
    unsafe {
        let tasklet = &mut *t;
        if tasklet.state.fetch_or(
            1u32 << TASKLET_STATE_SCHED, Ordering::AcqRel
        ) & (1u32 << TASKLET_STATE_SCHED) != 0 {
            return;
        }

        let cpu = crate::arch::cpu_id() as usize;
        let _lock = TASKLET_HI_LOCK[cpu].lock_irqsave();
        tasklet.list.add_tail(&mut TASKLET_HI_VEC[cpu] as *mut ListHead);
    }

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
    // SAFETY: caller must pass a valid pointer to a TaskletStruct that is not
    // concurrently freed; used during driver teardown when the struct is still live.
    unsafe {
        let tasklet = &mut *t;
        // Repeatedly clear SCHED and wait for RUN to clear
        loop {
            tasklet.state.fetch_and(
                !(1u32 << TASKLET_STATE_SCHED), Ordering::AcqRel
            );
            if !tasklet.is_running() {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

// ============================================================================
// Softirq action handlers
// ============================================================================

/// TASKLET_SOFTIRQ action handler.
/// Called by `__do_softirq` when TASKLET_SOFTIRQ is pending.
fn tasklet_action(_vec: usize) {
    let cpu = crate::arch::cpu_id() as usize;

    // 1. Detach the entire list under lock
    let mut local_head = ListHead::new();
    local_head.init();

    {
        let _lock = TASKLET_LOCK[cpu].lock_irqsave();
        // SAFETY: per-CPU list is accessed only under its own lock with IRQs
        // disabled, so no concurrent modification. TASKLET_VEC[cpu] is a valid
        // static mutable list head, and list nodes were placed by tasklet_schedule.
        unsafe {
            if !TASKLET_VEC[cpu].is_empty() {
                // Splice: move all entries from per-CPU list to local list
                local_head.next = TASKLET_VEC[cpu].next;
                local_head.prev = TASKLET_VEC[cpu].prev;
                (*local_head.next).prev = &mut local_head;
                (*local_head.prev).next = &mut local_head;
                TASKLET_VEC[cpu].init();
            }
        }
    }

    // 2. Process local list (no lock needed — local to this context)
    // SAFETY: local_head was spliced from the per-CPU list above; all list
    // nodes are valid TaskletStruct::list entries. The offset_of! calculation
    // recovers the enclosing TaskletStruct pointer. We hold no references into
    // the list that could alias with the mutable derefs below.
    unsafe {
        let mut pos = local_head.next;
        while pos != &mut local_head as *mut ListHead {
            let next = (*pos).next;
            let tasklet = (pos as usize - offset_of!(TaskletStruct, list)) as *mut TaskletStruct;

            // Clear SCHED bit and set RUN bit
            (*tasklet).state.store(
                1u32 << TASKLET_STATE_RUN,
                Ordering::Release
            );

            // Run if enabled
            if !(*tasklet).is_disabled() {
                if let Some(func) = (*tasklet).func {
                    func(tasklet);
                }
            }

            // Clear RUN bit; if SCHED was re-set, re-queue
            let old_state = (*tasklet).state.fetch_and(
                !(1u32 << TASKLET_STATE_RUN), Ordering::AcqRel
            );
            if old_state & (1u32 << TASKLET_STATE_SCHED) != 0 {
                let _lock = TASKLET_LOCK[cpu].lock_irqsave();
                (*tasklet).list.add_tail(
                    &mut TASKLET_VEC[cpu] as *mut ListHead
                );
                crate::interrupt::softirq::raise_softirq_irqoff(
                    crate::interrupt::softirq::SoftirqIndex::Tasklet as usize
                );
            }

            pos = next;
        }
    }
}

/// HI_SOFTIRQ action handler.
/// Identical to tasklet_action but uses HI_VEC and HI_LOCK.
fn tasklet_hi_action(_vec: usize) {
    let cpu = crate::arch::cpu_id() as usize;

    let mut local_head = ListHead::new();
    local_head.init();

    {
        let _lock = TASKLET_HI_LOCK[cpu].lock_irqsave();
        // SAFETY: same reasoning as tasklet_action splice above — per-CPU list
        // accessed only under its own lock with IRQs disabled.
        unsafe {
            if !TASKLET_HI_VEC[cpu].is_empty() {
                local_head.next = TASKLET_HI_VEC[cpu].next;
                local_head.prev = TASKLET_HI_VEC[cpu].prev;
                (*local_head.next).prev = &mut local_head;
                (*local_head.prev).next = &mut local_head;
                TASKLET_HI_VEC[cpu].init();
            }
        }
    }

    // SAFETY: same reasoning as tasklet_action local list processing above.
    unsafe {
        let mut pos = local_head.next;
        while pos != &mut local_head as *mut ListHead {
            let next = (*pos).next;
            let tasklet = (pos as usize - offset_of!(TaskletStruct, list)) as *mut TaskletStruct;

            (*tasklet).state.store(
                1u32 << TASKLET_STATE_RUN,
                Ordering::Release
            );

            if !(*tasklet).is_disabled() {
                if let Some(func) = (*tasklet).func {
                    func(tasklet);
                }
            }

            let old_state = (*tasklet).state.fetch_and(
                !(1u32 << TASKLET_STATE_RUN), Ordering::AcqRel
            );
            if old_state & (1u32 << TASKLET_STATE_SCHED) != 0 {
                let _lock = TASKLET_HI_LOCK[cpu].lock_irqsave();
                (*tasklet).list.add_tail(
                    &mut TASKLET_HI_VEC[cpu] as *mut ListHead
                );
                crate::interrupt::softirq::raise_softirq_irqoff(
                    crate::interrupt::softirq::SoftirqIndex::Hi as usize
                );
            }

            pos = next;
        }
    }
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize the tasklet subsystem.
/// Registers tasklet_action and tasklet_hi_action as softirq handlers.
pub fn init() {
    // Init per-CPU list heads
    for cpu in 0..MAX_CPUS {
        // SAFETY: called once during subsystem init before any tasklet is
        // scheduled; no concurrent access to these per-CPU list heads yet.
        unsafe {
            TASKLET_VEC[cpu].init();
            TASKLET_HI_VEC[cpu].init();
        }
    }

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
