//! The interrupt subsystem's ONE audited unsafe file (mainline
//! minimal-first consolidation, 2026-11-27): the raw edges the IRQ
//! layer still needs live here behind one audited item per family;
//! irqdesc.rs, irqchip.rs, irqthread.rs, domain.rs, ksoftirqd.rs,
//! preempt.rs, softirq.rs and tasklet.rs are safe code over them
//! (preempt.rs keeps its own stricter `#![deny(unsafe_code)]` plus
//! per-item promotions, unchanged).
//!
//! Edge families:
//! - the NMI word -> handler transmute (INV-IRQ-NMIHANDLER-1)
//! - caller-passed live TaskletStruct pointers (INV-IRQ-TASKLET-PTR-1)
//! - the per-CPU tasklet vectors + the intrusive splice/drain walk
//!   (INV-IRQ-TASKLET-VEC-1 / INV-IRQ-TASKLET-DRAIN-1)
//! - the softirq bracket-leak forensic reads (INV-IRQ-BRACKET-LEAK-1)

use crate::list::ListHead;
use super::tasklet::{
    TaskletStruct, TASKLET_HI_LOCK, TASKLET_HI_VEC, TASKLET_LOCK, TASKLET_VEC,
    TASKLET_STATE_RUN, TASKLET_STATE_SCHED,
};
use crate::config::MAX_CPUS;
use core::mem::offset_of;
use core::sync::atomic::Ordering;

// ------------------------------------------------------------------
// NMI dispatch
// ------------------------------------------------------------------

/// INV-IRQ-NMIHANDLER-1: the `NMI_HANDLERS` slots hold usize words
/// stored by `request_nmi` from a live `NmiHandler` fn pointer (the
/// round-trip: usize casts back are not primitive-castable, hence the
/// transmute); a non-zero word therefore converts back to exactly the
/// fn pointer that was stored, and the slot is never cleared while
/// dispatch is reachable.
pub(super) fn nmi_handler_from_word(word: usize) -> super::irqdesc::NmiHandler {
    // SAFETY: INV-IRQ-NMIHANDLER-1 above (request_nmi round-trip).
    unsafe { core::mem::transmute(word) }
}

// ------------------------------------------------------------------
// Tasklet scheduling / kill (caller-passed pointers)
// ------------------------------------------------------------------

/// INV-IRQ-TASKLET-PTR-1: every `*mut TaskletStruct` handed to the
/// schedule/kill edges comes from a driver embedding the struct in its
/// device-lifetime private allocation; the pointer is valid and live
/// for the duration of the call and no other edge frees it while the
/// SCHED/RUN bits say it is queued or running.
///
/// Schedule a tasklet for execution (TASKLET_SOFTIRQ vector).
pub(super) fn tasklet_schedule(t: *mut TaskletStruct) {
    // SAFETY: INV-IRQ-TASKLET-PTR-1 above (live device-lifetime struct);
    // the list link runs under the vector's per-CPU irqsave lock.
    unsafe {
        let tasklet = &mut *t;
        // Already scheduled? Skip.
        if tasklet.state.fetch_or(1u32 << TASKLET_STATE_SCHED, Ordering::AcqRel)
            & (1u32 << TASKLET_STATE_SCHED)
            != 0
        {
            return;
        }

        let cpu = crate::arch::cpu_id() as usize;
        let _lock = TASKLET_LOCK[cpu].lock_irqsave();
        tasklet.list.add_tail(&mut TASKLET_VEC[cpu] as *mut ListHead);
    }
}

/// Schedule a high-priority tasklet (HI_SOFTIRQ vector).
pub(super) fn tasklet_hi_schedule(t: *mut TaskletStruct) {
    // SAFETY: INV-IRQ-TASKLET-PTR-1 above; per-CPU irqsave lock held.
    unsafe {
        let tasklet = &mut *t;
        if tasklet.state.fetch_or(1u32 << TASKLET_STATE_SCHED, Ordering::AcqRel)
            & (1u32 << TASKLET_STATE_SCHED)
            != 0
        {
            return;
        }

        let cpu = crate::arch::cpu_id() as usize;
        let _lock = TASKLET_HI_LOCK[cpu].lock_irqsave();
        tasklet.list.add_tail(&mut TASKLET_HI_VEC[cpu] as *mut ListHead);
    }
}

/// Kill a tasklet: ensure it is not scheduled and wait for it to
/// finish. Used during driver teardown.
pub(super) fn tasklet_kill(t: *mut TaskletStruct) {
    // SAFETY: INV-IRQ-TASKLET-PTR-1 above — teardown path, struct not
    // concurrently freed.
    unsafe {
        let tasklet = &mut *t;
        // Repeatedly clear SCHED and wait for RUN to clear
        loop {
            tasklet.state.fetch_and(!(1u32 << TASKLET_STATE_SCHED), Ordering::AcqRel);
            if !tasklet.is_running() {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

// ------------------------------------------------------------------
// Per-CPU vectors + intrusive splice/drain
// ------------------------------------------------------------------

/// INV-IRQ-TASKLET-VEC-1: the per-CPU `TASKLET_VEC` / `TASKLET_HI_VEC`
/// static-mut list heads are written only (a) here at subsystem init,
/// before any tasklet can be scheduled, and (b) through `splice_all`
/// under the vector's own per-CPU lock with IRQs disabled.
pub(super) fn tasklet_vecs_init() {
    // SAFETY: INV-IRQ-TASKLET-VEC-1 — once-at-init, no concurrency yet.
    unsafe {
        for cpu in 0..MAX_CPUS {
            TASKLET_VEC[cpu].init();
            TASKLET_HI_VEC[cpu].init();
        }
    }
}

/// Splice the whole per-CPU list onto `dst` (caller holds the source's
/// per-CPU lock with IRQs disabled — the single audited splice edge of
/// the intrusive tasklet list).
pub(super) fn splice_all(dst: &mut ListHead, src: &mut ListHead) {
    // SAFETY: INV-IRQ-TASKLET-VEC-1 — per-CPU list accessed only under
    // its own lock with IRQs disabled, so no concurrent modification;
    // nodes were placed by tasklet_schedule under the same lock.
    unsafe {
        if !src.is_empty() {
            dst.next = src.next;
            dst.prev = src.prev;
            (*dst.next).prev = dst;
            (*dst.prev).next = dst;
            src.init();
        }
    }
}

/// Splice the TASKLET_SOFTIRQ per-CPU vector onto `local` (caller holds
/// the vector's per-CPU lock with IRQs disabled — INV-IRQ-TASKLET-VEC-1).
pub(super) fn splice_tasklet_vec(local: &mut ListHead, cpu: usize) {
    // SAFETY: INV-IRQ-TASKLET-VEC-1 — lock-held per-CPU access.
    unsafe { splice_all(local, &mut TASKLET_VEC[cpu]) }
}

/// The HI_SOFTIRQ twin of `splice_tasklet_vec`.
pub(super) fn splice_tasklet_hi_vec(local: &mut ListHead, cpu: usize) {
    // SAFETY: INV-IRQ-TASKLET-VEC-1 — lock-held per-CPU access.
    unsafe { splice_all(local, &mut TASKLET_HI_VEC[cpu]) }
}

/// Walk a privately-spliced tasklet list: CAS-clear SCHED/set RUN, run
/// the enabled callbacks, clear RUN. The offset_of! calculation
/// recovers the enclosing TaskletStruct pointer (the container_of
/// edge — one audited walk, shared by both action handlers since
/// round-5).
pub(super) fn drain_tasklet_list(local_head: &mut ListHead) {
    // SAFETY: INV-IRQ-TASKLET-DRAIN-1: local_head was spliced from the
    // per-CPU list under its lock; all list nodes are valid
    // TaskletStruct::list entries, and we hold no references into the
    // list that could alias the mutable derefs below.
    unsafe {
        let mut pos = local_head.next;
        while pos != local_head as *mut ListHead {
            let next = (*pos).next;
            let tasklet =
                (pos as usize - offset_of!(TaskletStruct, list)) as *mut TaskletStruct;

            // CAS (review batch 8): atomically clear SCHED and set RUN in
            // one swap — the Linux tasklet_trylock() +
            // test_and_clear_bit(SCHED) pair. This closes the double-link
            // race of the old plain store: an interrupt re-scheduling the
            // tasklet DURING func() sets SCHED (0->1) and links it once;
            // re-linking here as well left the tasklet on the per-CPU
            // list twice (double run + corruption on the second dequeue).
            let old_state = (*tasklet).state.swap(1u32 << TASKLET_STATE_RUN, Ordering::AcqRel);
            if old_state & (1u32 << TASKLET_STATE_SCHED) == 0 {
                // SCHED was cleared while queued (tasklet_kill): do not run.
                (*tasklet).state.fetch_and(!(1u32 << TASKLET_STATE_RUN), Ordering::AcqRel);
                pos = next;
                continue;
            }

            // Run if enabled
            if !(*tasklet).is_disabled() {
                if let Some(func) = (*tasklet).func {
                    func(tasklet);
                }
            }

            // Clear RUN only. If the tasklet was re-scheduled during
            // func(), THAT path (tasklet_schedule's fetch_or SCHED 0->1)
            // already linked it — do NOT link again here.
            (*tasklet).state.fetch_and(!(1u32 << TASKLET_STATE_RUN), Ordering::AcqRel);

            pos = next;
        }
    }
}

// ------------------------------------------------------------------
// Softirq bracket-leak forensics
// ------------------------------------------------------------------

/// INV-IRQ-BRACKET-LEAK-1: the diagnostic reads fields of tasks that
/// were current on this CPU moments before (pinned by execution, the
/// tp register values bracketing the softirq window), the fields read
/// are plain integers, and the diagnostic runs from the irq-exit path
/// with IRQs off — there is no safe expression for "read through this
/// raw task pointer" without inventing a reference the type system
/// cannot justify. Diagnostic-only: a torn/gone task yields a wrong
/// forensics line, never a fault on mapped kernel memory (the task
/// structures are kernel-heap allocations not unmapped on exit).
///
/// Returns (pid_a, preempt_a, pid_b, preempt_b) for the entry/exit tp
/// words of a softirq bracket.
pub(super) fn bracket_task_fields(a_tp: usize, b_tp: usize) -> (i32, i32, i32, i32) {
    let a = a_tp as *mut crate::process::task::Task;
    let b = b_tp as *mut crate::process::task::Task;
    // PER-CPU MIGRATION: the count fields are gone from the task; the
    // probe now reports this cpu's word for both legs (the bracket ran
    // on one cpu — that word is the only count there ever was).
    // SAFETY: INV-IRQ-BRACKET-LEAK-1 above (diagnostic scalar reads).
    let w = crate::interrupt::preempt::preempt_count();
    unsafe { ((*a).pid() as i32, w, (*b).pid() as i32, w) }
}

// ------------------------------------------------------------------
// Assembly-crossing symbols (the sched/unsafe.rs convention: a symbol
// exported to assembly IS a raw edge — the link boundary is outside the
// language's guarantees.)
// ------------------------------------------------------------------

/// The trap-return preempt gate for the arch asm paths (riscv64 user +
/// kernel twins, aarch64 user gate — the x86 twin is
/// `x86_preemptible`): reads THIS cpu's preempt_count word directly.
/// PER-CPU MIGRATION: the pre-migration gates loaded the per-task field
/// by offset (riscv64 `lw {task_ti_preempt}(tp)`, aarch64
/// `ldr [x20, #TI_PREEMPT]`); the word no longer lives in the task, and
/// these gates are reached only when need_resched is already set, so
/// the call sits on the resched-pending path only.
#[no_mangle]
pub extern "C" fn asm_preemptible() -> i64 {
    if crate::interrupt::preempt::preemptible() {
        1
    } else {
        0
    }
}
