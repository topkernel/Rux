//! Post-grace reaping of fd-table slot occupants — the deferred-fput
//! shape for the lock-free slot array.
//!
//! `FdTable::get_file` is an RCU reader: one raw slot load + one
//! strong-count increment inside a read-side critical section — no bank
//! claim, no irq save (the retired bank protocol cost two extra locked
//! RMWs per lookup, and get_file runs 130M+ times per boot). The window
//! between a reader's slot load and its increment is closed by a GRACE
//! PERIOD, not by reader counts: a writer that removes a slot
//! (close/dup2/displacement) parks the removed occupant here, and this
//! reaper runs the last-reference close decision and drops the parked
//! references only after a full grace period.
//!
//! Why that is sound: a grace period requires every CPU to report a
//! quiescent state (user-return, context switch, or idle) at/after its
//! start. A reader between its load and its increment cannot hit any of
//! those points — the window contains no schedule point, and
//! `rcu_read_lock` holds preemption off across it — so no grace period
//! that started after a slot-null store can complete while such a
//! reader is open. The park (under the retire spinlock) and the
//! semaphore handoff give the slot-null store → grace-start ordering.
//!
//! Timing note: the close op (posix record-lock release, ops.close —
//! pipe reader-count drop, epoll purge) now runs in the reaper kthread
//! instead of inline at close(2) return. It may lag by a grace period
//! (microseconds on a standard boot); blocked peers are woken by the
//! close op itself, so a writer parked on a pipe whose reader closed
//! wakes when the reaper runs that release.

use crate::fs::file::{FdTable, File};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Parked slot occupants awaiting a grace period. One global queue: the
/// grace wait is table-independent, so batching every table's retires
/// into one wait amortizes it. Parked Arcs keep their strong count
/// alive, which is exactly the guarantee get_file's unconditional
/// increment relies on (the count cannot reach zero while parked).
static RETIRED: crate::sync::spinlock::Spinlock<Vec<Arc<File>>> =
    crate::sync::spinlock::Spinlock::new(Vec::new());

/// Wake ticket for the reaper: every park ups once.
static TICKET: crate::sync::semaphore::Semaphore =
    crate::sync::semaphore::Semaphore::new(0);

/// Reaper aliveness. While false (spawn failed, or a close before the
/// late-boot init slot), parks take the inline bounded-grace fallback —
/// slower close, same correctness; nothing may skip the grace.
static LIVE: AtomicBool = AtomicBool::new(false);

/// DFX gauges: parked references currently awaiting grace, and grace
/// waits that exceeded their budget (the monopolist-window family —
/// see synchronize_rcu_bounded). A non-zero RETIRED_DEPTH on a standard
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

/// Park a removed slot occupant for the post-grace drop. Writers call
/// this AFTER releasing their entry lock (R14-5: no close op under it).
pub fn retire(file: Arc<File>) {
    if !LIVE.load(Ordering::Acquire) {
        // Inline fallback: wait the grace for this one file here.
        // close(2) latency only — never correctness.
        if crate::sync::rcu::synchronize_rcu_forced(GP_BUDGET_MS, "fdreap-inline") {
            FdTable::displaced_close_decision(&file);
            return;
        }
        // Timed out (monopolist): fall through to the queue; when the
        // reaper is up it retries, and if it never comes up the final
        // table Drop re-runs the decision.
    }
    {
        let mut q = RETIRED.lock_irqsave();
        q.push(file);
    }
    RETIRED_DEPTH.fetch_add(1, Ordering::Relaxed);
    TICKET.up();
}

/// Spawn fdreapd. Late-boot slot (after secondaries online — its sleeps
/// and wakeups need the scheduler and online CPUs), before the first
/// userspace close.
pub fn init() {
    let ok = crate::process::kthread::kthread_run(
        fdreapd_fn,
        core::ptr::null_mut(),
        "fdreapd",
    )
    .is_some();
    LIVE.store(ok, Ordering::Release);
    if ok {
        crate::pr_info!("fs: fdreapd retire reaper started");
    } else {
        crate::pr_warn!("fs: fdreapd failed to start — closes fall back to inline grace waits");
    }
}

/// fdreapd main loop: sleep until parked work exists, wait one grace
/// period, then run the close decisions for the whole batch.
extern "C" fn fdreapd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    loop {
        if crate::process::kthread::kthread_should_stop() {
            break;
        }
        TICKET.down();
        let batch = {
            let mut q = RETIRED.lock_irqsave();
            core::mem::take(&mut *q)
        };
        if batch.is_empty() {
            // Spurious wake (a force-wake out of UNINTERRUPTIBLE, R77
            // family): nothing parked since our last take.
            continue;
        }
        RETIRED_DEPTH.fetch_sub(batch.len(), Ordering::Relaxed);
        // One grace period covers the whole batch: every park (and its
        // slot-null store) precedes the semaphore handoff that got us
        // here, which precedes this wait.
        if !crate::sync::rcu::synchronize_rcu_forced_sleepy(GP_BUDGET_MS, "fdreap") {
            // Grace did not pass: NO drop — requeue and re-arm the
            // ticket. Each retry paces itself on the bounded wait.
            GRACE_STALLS.fetch_add(1, Ordering::Relaxed);
            let n = batch.len();
            {
                let mut q = RETIRED.lock_irqsave();
                q.extend(batch);
            }
            RETIRED_DEPTH.fetch_add(n, Ordering::Relaxed);
            TICKET.up();
            continue;
        }
        for file in &batch {
            // Sleepable context: record-lock release and ops.close may
            // take mutexes, wake peers, or queue work (the discipline
            // close_fd ran at its drain point, moved here).
            FdTable::displaced_close_decision(file);
        }
    }
    0
}
