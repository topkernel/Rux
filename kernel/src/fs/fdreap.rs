//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Post-grace reclamation of fd-table slot occupants — the deferred
//! `file_free_rcu` shape for the lock-free slot array.
//!
//! `FdTable::get_file` is an RCU reader: one raw slot load plus a
//! CONDITIONAL strong-count acquisition (`file_arc_try_clone`, the
//! `get_file_rcu` / `atomic_long_inc_not_zero` shape) inside a
//! read-side critical section — no bank claim, no irq save beyond the
//! reader window itself (get_file runs 130M+ times per boot). The
//! window between a reader's slot load and its count CAS is closed by
//! TWO mechanisms, neither of which waits in close(2):
//!
//! - The close DECISIONS (POSIX record-lock release, flock/OFD/lease
//!   release, IN_CLOSE_* fsnotify, ops.close) are refcount-driven:
//!   they run in `File::drop` at the LAST strong reference — in the
//!   closer's own context on the common single-fd close, so userspace
//!   still never observes a closed fd whose side effects have not run
//!   (the r26 LTP pipe08/fcntl15/inotify family). A reader whose CAS
//!   lands first keeps the description legitimately alive and the
//!   decisions move to ITS final drop — reference behavior (Linux
//!   fget-holder semantics).
//! - The MEMORY (the ArcInner) is pinned by a Weak reference parked
//!   here and freed only after a full grace period — the reference's
//!   `call_rcu(file_free_rcu)` shape, run asynchronously by fdreapd.
//!   A reader that loads a slot pointer and then pauses cannot touch
//!   freed memory: its read-side critical section (irqs off, preempt
//!   off) reports no quiescent state, so no grace period that started
//!   after the park can complete while the reader is between its load
//!   and its CAS — and by the time the grace passes, the count is
//!   zero and the reader's CAS fails cleanly (the ArcInner is still
//!   allocated because the parked Weak holds it).
//!
//! Why a reader can never RESURRECT a dying File: acquisition is a
//! CAS from a nonzero count; once the closer's drop takes the count
//! to zero, a racing CAS that read `1` fails on `0` and the reader
//! treats the fd as closed (EBADF — exactly the reference's fget race
//! outcome). The unconditional-increment protocol this replaces
//! needed the closer to wait out a grace period BEFORE dropping
//! (~250-330 ns charged to every close(2)); with conditional
//! acquisition the same guarantee costs close(2) one Weak bump and
//! one queue push.
//!
//! Timing note — REFERENCE PARITY (r26 fix, oc-rc reshuffle): the
//! reference kernel runs the final fput (`__fput`: record-lock
//! release, fsnotify_close, pipe reader-count drop) in TASK WORK at
//! the return-to-userspace boundary — never blocking close(2) on a
//! grace — and frees `struct file` via `call_rcu` asynchronously.
//! This module now has the same split: decisions at last-reference
//! drop (in the closer's context for the common case, i.e. before
//! close(2) returns), memory after an asynchronous grace.

#![forbid(unsafe_code)]
use crate::fs::file::File;
use alloc::sync::Arc;
use alloc::sync::Weak;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Parked Weaks awaiting a grace period. One global queue: the grace
/// wait is table-independent, so batching every table's retires into
/// one wait amortizes it. The Weak pins the ArcInner ALLOCATION only —
/// the File value inside has already been dropped (File::drop ran the
/// whole close decision chain when the strong count hit zero).
static RETIRED: crate::sync::spinlock::Spinlock<Vec<Weak<File>>> =
    crate::sync::spinlock::Spinlock::new(Vec::new());

/// Wake ticket for fdreapd. Parks up() only when a batch threshold is
/// crossed (a wake per close(2) cost one wakeup + IPI each and dirtied
/// the grace-period cachelines — the 2026-11-05 measurement of the old
/// protocol's TICKET path); fdreapd's drain is pure memory
/// reclamation, so batching costs nothing but a bounded, tiny queue
/// depth.
static TICKET: crate::sync::semaphore::Semaphore =
    crate::sync::semaphore::Semaphore::new(0);

/// Reaper aliveness. While false (spawn failed), parks drain inline
/// with a bounded forced grace in the closer — slower close, same
/// correctness; nothing may skip the grace.
static LIVE: AtomicBool = AtomicBool::new(false);

/// Park threshold: kick fdreapd when this many Weaks are queued. Pure
/// reclamation latency knob — no semantics.
const WAKE_BATCH: usize = 16;

/// DEBUG gauges: parked Weaks currently awaiting grace, and grace waits
/// that exceeded their budget (the monopolist-window family — see
/// synchronize_rcu_bounded). A non-zero RETIRED_DEPTH on a standard
/// boot should drain to zero within a grace period; sustained growth
/// names a stalled reaper or a monopolist CPU.
pub static RETIRED_DEPTH: AtomicUsize = AtomicUsize::new(0);
pub static GRACE_STALLS: AtomicUsize = AtomicUsize::new(0);

/// Grace budget per reaping cycle. Generous against ordinary load
/// spikes; a monopolist window exceeds it and the batch is requeued.
///
/// Per-arch: under single-threaded TCG a halted secondary vCPU only
/// wakes when the host thread round-robins — a CPU pinned in guest
/// userspace (shell arithmetic loops) starves the idler's QS reports
/// for SECONDS (measured: tick_age > 1000 ms on the soak boots, with
/// the idle QS frozen one generation behind for the whole window). The
/// timeout path is safe (requeue, never drop), but a 50 ms budget there
/// means constant re-waits and warn spam. KVM (x86_64) reports QS in
/// microseconds; 50 ms is already generous.
#[cfg(any(feature = "riscv64", feature = "aarch64"))]
const GP_BUDGET_MS: u64 = 2500;
#[cfg(not(any(feature = "riscv64", feature = "aarch64")))]
const GP_BUDGET_MS: u64 = 50;

/// Park a removed slot occupant's allocation for post-grace reclamation
/// and run its close side effects before returning — the reference
/// task_work-parity form for syscall-context displacement (close(2),
/// dup2(2)/dup3 displacement). Writers call this AFTER releasing their
/// entry lock (R14-5: no close op under it).
///
/// Sequence (the fput + task_work + call_rcu split):
/// 1. POSIX record locks: closing ANY fd referring to the file releases
///    the closing process's locks on it — a close-EVENT property, not a
///    last-reference property (the reference runs locks_remove_posix at
///    filp_close). Runs now, refcount-independent.
/// 2. Arm `close_pending`: the once-only close op runs in `File::drop`
///    at the final strong reference — this context for the common
///    single-fd close, the racing holder's release otherwise. Exactly
///    one drop-to-zero exists, so the op runs exactly once (the old
///    stale-flag hazard is structurally gone).
/// 3. Park a Weak: pins the ArcInner across any in-flight conditional
///    acquisition until fdreapd's grace passes.
/// 4. Drop the strong reference. No grace is waited here — close(2)
///    never blocks on RCU (reference parity).
pub fn retire_complete(file: Arc<File>) {
    // 1) Close-event decisions (refcount-independent).
    crate::fs::locks::posix_release_for_file(&file);
    // 2) Arm the final-drop close op (R31-B3 discipline, single site).
    file.close_pending.store(true, Ordering::Release);
    // 3) Pin the allocation for the grace window.
    let weak = Arc::downgrade(&file);
    let mut wake = false;
    {
        let mut q = RETIRED.lock_irqsave();
        q.push(weak);
        RETIRED_DEPTH.fetch_add(1, Ordering::Relaxed);
        if q.len() >= WAKE_BATCH {
            wake = true;
        }
    }
    // 4) Drop our strong reference: File::drop runs the decision chain
    //    (flock/OFD release, open-count unregister, O_ASYNC unregister,
    //    IN_CLOSE_* fsnotify, and the armed ops.close — sleepable ops
    //    stay in this syscall context, the same discipline the old
    //    displaced_close_decision had).
    drop(file);
    if wake {
        TICKET.up();
        return;
    }
    // Reaper down (spawn failed / pre-init): drain inline with a bounded
    // forced grace — boot-time stdio closes only. Correctness unchanged:
    // the batch is never dropped without a grace.
    if !LIVE.load(Ordering::Acquire) && crate::sched::current().is_some() {
        drain_with_grace();
    }
}

/// Take the whole parked queue, wait one forced grace period, then let
/// the Weaks go (freeing every ArcInner whose grace passed). Requeues
/// on a budget miss. Shared by the closer's inline fallback and fdreapd.
fn drain_with_grace() {
    let batch = {
        let mut q = RETIRED.lock_irqsave();
        core::mem::take(&mut *q)
    };
    if batch.is_empty() {
        return;
    }
    RETIRED_DEPTH.fetch_sub(batch.len(), Ordering::Relaxed);
    if crate::sync::rcu::synchronize_rcu_forced(GP_BUDGET_MS, "fdreap-inline") {
        // Drop the Weaks: each releases its ArcInner (the File value
        // itself already died at strong-count zero).
        drop(batch);
        return;
    }
    // Grace did not pass (monopolist window): requeue. When fdreapd is
    // up it retries (armed below); on the inline fallback path the next
    // close retries. The batch is never freed without a grace.
    GRACE_STALLS.fetch_add(1, Ordering::Relaxed);
    RETIRED_DEPTH.fetch_add(batch.len(), Ordering::Relaxed);
    {
        let mut q = RETIRED.lock_irqsave();
        q.extend(batch);
    }
    TICKET.up();
}

/// Spawn fdreapd. Late-boot slot (after secondaries online — its sleeps
/// and wakeups need the scheduler and online CPUs), before the first
/// userspace close. The initial TICKET kick drains anything parked
/// before the reaper existed.
pub fn init() {
    // Pin the ArcInner layout assumption file_arc_try_clone makes
    // (fails loudly at boot if a toolchain change moves the refcount
    // word the lock-free fd lookup CASes).
    crate::fs::file::verify_arc_inner_layout();
    let ok = crate::process::kthread::kthread_run(
        fdreapd_fn,
        core::ptr::null_mut(),
        "fdreapd",
    )
    .is_some();
    LIVE.store(ok, Ordering::Release);
    if ok {
        crate::pr_info!("fs: fdreapd retire reaper started");
        TICKET.up();
    } else {
        crate::pr_warn!("fs: fdreapd failed to start — closes drain inline with a forced grace");
    }
}

/// fdreapd main loop: sleep until parked work exists, wait one forced
/// grace period, then release the parked Weaks.
extern "C" fn fdreapd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    loop {
        if crate::process::kthread::kthread_should_stop() {
            break;
        }
        // INTERRUPTIBLE park (reference discipline for a perpetually-idle
        // daemon: Linux kthreads sleep on down_interruptible()/wait_event_
        // interruptible, so the hung-task detector never flags their idle
        // wait). A spurious wake is handled below (empty batch → continue).
        let _ = TICKET.down_interruptible();
        let batch = {
            let mut q = RETIRED.lock_irqsave();
            core::mem::take(&mut *q)
        };
        if batch.is_empty() {
            continue;
        }
        RETIRED_DEPTH.fetch_sub(batch.len(), Ordering::Relaxed);
        // One grace period covers the whole batch: every park (and its
        // slot-null store) precedes the ticket/wake that got us here,
        // which precedes this wait.
        if !crate::sync::rcu::synchronize_rcu_forced_sleepy(GP_BUDGET_MS, "fdreap") {
            // Grace did not pass: NO release — requeue and re-arm the
            // ticket. Each retry paces itself on the bounded wait.
            GRACE_STALLS.fetch_add(1, Ordering::Relaxed);
            RETIRED_DEPTH.fetch_add(batch.len(), Ordering::Relaxed);
            {
                let mut q = RETIRED.lock_irqsave();
                q.extend(batch);
            }
            TICKET.up();
            continue;
        }
        // Release the Weaks (each frees an ArcInner whose grace passed).
        drop(batch);
    }
    0
}
