//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Threaded IRQ service primitive (RT-plan R5)
//!
//! The x86_64 fast IRQ paths (the virtio-blk MSI-X completion vector,
//! the PIC INTx line table in arch::x86_64::trap) do not go through
//! irq_desc's action chain — they predate the irq-domain layer. This
//! module gives them the same forced-threading shape irqdesc.rs lands
//! for the PLIC/GIC arms: the hardirq top half only records `pending`
//! and wakes a per-IRQ SCHED_FIFO 50 kthread; the thread runs the
//! service and re-parks with the W3 lost-wake discipline.
//!
//! Lifecycle precedent: ksoftirqd (kthread_run + INTERRUPTIBLE park +
//! re-check-before-schedule). The timer/clock source and IPI vectors
//! never use this — they stay raw hardirq (the reference's shape).

#![forbid(unsafe_code)]
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::process::task::{SchedPolicy, Task, TaskState};

/// Thread spawning needs the scheduler (kthread enqueue touches the
/// GRQ), but IRQ registrations start long before sched::init (the
/// serial line and the virtio probes both run early). Until this latch
/// opens, `ThreadedIrq::spawn` no-ops and every dispatcher's inline
/// fallback runs the action in hardirq — the threaded shape activates
/// at `boot_init()` (called beside ksoftirqd::init, the ksoftirqd
/// lifecycle precedent).
static SPAWNS_ALLOWED: AtomicBool = AtomicBool::new(false);

/// Whether per-IRQ threads may be spawned yet.
pub fn spawns_allowed() -> bool {
    SPAWNS_ALLOWED.load(Ordering::Acquire)
}

/// Deferred per-IRQ thread pass: opens the latch and spawns every
/// thread a pre-scheduler registration asked for. Called from main.rs
/// right after ksoftirqd::init().
pub fn boot_init() {
    SPAWNS_ALLOWED.store(true, Ordering::Release);
    if !crate::interrupt::irqdesc::force_threaded() {
        return;
    }
    let mut n = 0usize;
    #[cfg(feature = "x86_64")]
    {
        n += crate::arch::x86_64::trap::irq_threads_boot_init();
    }
    n += crate::interrupt::irqdesc::irq_threads_boot_spawn();
    if n > 0 {
        crate::print_status("irq", &alloc::format!("threaded IRQs x{}", n), true);
    }
}

/// One threaded IRQ service slot. Embed in a static, `spawn` once at
/// registration time, `wake` from the hardirq top half.
pub struct ThreadedIrq {
    /// Set by the top half (Release, BEFORE the wake); consumed by the
    /// thread (swap false). Coalesces re-fires while the thread runs.
    pending: AtomicBool,
    /// The kthread (set once at spawn, never cleared). 0 = not spawned.
    task: AtomicUsize,
}

impl ThreadedIrq {
    pub const fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            task: AtomicUsize::new(0),
        }
    }

    /// Whether this service's thread has been spawned.
    pub fn spawned(&'static self) -> bool {
        self.task.load(Ordering::Acquire) != 0
    }

    /// Hardirq top half: record pending, then wake the thread. The order
    /// is the W3 discipline — pending is visible (Release) before the
    /// wake, so the thread's park re-check cannot miss it. Returns false
    /// when the thread was never spawned — callers with an inline
    /// fallback (the registration race) run it this once.
    pub fn wake(&'static self) -> bool {
        let raw = self.task.load(Ordering::Acquire);
        if raw == 0 {
            return false; // never spawned (or spawn failed)
        }
        self.pending.store(true, Ordering::Release);
        // SAFETY: stored once at spawn from a valid kthread pointer and
        // never cleared or freed (kthreads are refcounted by the task
        // table for the boot's lifetime).
        let t = raw as *mut Task;
        crate::sched::wake_up_process(t);
        true
    }

    /// Spawn the service kthread (registration-time, idempotent).
    /// `thread_fn` is the kthread entry; convention: it calls
    /// `self.thread_loop(service)` with the actual work. Returns false
    /// (spawn not attempted) before the scheduler is up — the caller's
    /// inline fallback covers the window and `boot_init` retries.
    pub fn spawn(
        &'static self,
        name: &'static str,
        thread_fn: extern "C" fn(*mut c_void) -> i32,
        arg: *mut c_void,
    ) -> bool {
        if self.task.load(Ordering::Acquire) != 0 {
            return true; // already spawned
        }
        if !spawns_allowed() {
            return false;
        }
        if let Some(t) = crate::process::process_hooks::kthread_run(thread_fn, arg, name) {
            let tp = t as *mut Task;
            crate::sched::sched::change_task_policy(tp, SchedPolicy::Fifo, 50);
            self.task.store(tp as usize, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// The kthread body: consume pending, run `service`, re-park. The
    /// W3 discipline on the park path: set INTERRUPTIBLE, re-check
    /// pending, only then schedule — a wake that raced the park window
    /// is not lost (same shape as irqdesc::irq_thread_fn and
    /// ksoftirqd_fn).
    pub fn thread_loop<F: FnMut()>(&'static self, mut service: F) -> i32 {
        loop {
            if crate::process::process_hooks::kthread_should_stop() {
                break;
            }
            if self.pending.swap(false, Ordering::AcqRel) {
                service();
                continue;
            }
            let raw = self.task.load(Ordering::Acquire);
            if raw == 0 {
                return 0;
            }
            // This IS the kthread the pointer names (set at spawn,
            // before the thread can run thread_loop) — the scheduler's
            // current() handle is that same task.
            let Some(t) = crate::sched::current() else { return 0 };
            t.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            if self.pending.swap(false, Ordering::AcqRel) {
                t.set_state(TaskState::new(TaskState::RUNNING));
                continue;
            }
            crate::sched::schedule();
        }
        0
    }
}
