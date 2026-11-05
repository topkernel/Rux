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
//! Timing note — REFERENCE PARITY (r26 fix): the reference kernel runs
//! the final fput (`__fput`: record-lock release, fsnotify_close, pipe
//! reader-count drop) in TASK WORK at the return-to-userspace boundary,
//! so userspace never observes a closed fd whose side effects have not
//! run: write-to-closed-pipe sees EPIPE immediately, a lock region is
//! free right after close(2) returns, and an inotify read sees
//! IN_CLOSE_WRITE/NOWRITE. The async-only reaper broke all of those
//! (LTP pipe02/pipe08/write05/writev01, fcntl14/15/33, inotify01/02/05,
//! execve03 — the r25-census regression family; producer 8d81c5da).
//! `retire_complete` is therefore the form syscall-context displacement
//! uses: park like before, then COMPLETE the close decisions for
//! everything parked up to and including ours — draining the queue in
//! the closer's own context when the reaper does not win the race —
//! before returning. The reaper remains as the parallel processor (and
//! the monopolist-timeout retry path).

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

/// Monotonic retire accounting. PUSHED_SEQ is the ordinal assigned to
/// each park (1-indexed); PROCESSED_SEQ is the ordinal of the last file
/// whose close decision COMPLETED. The queue is FIFO and every drain
/// takes a whole prefix of it, so "PROCESSED_SEQ >= my ordinal" proves
/// my file's close decision ran — regardless of whether the reaper or
/// my own task drained the batch. retire_complete waits on exactly that.
static PUSHED_SEQ: AtomicUsize = AtomicUsize::new(0);
static PROCESSED_SEQ: AtomicUsize = AtomicUsize::new(0);

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

/// 51st session: failed-grace budgets a close(2) tolerates before
/// deferring its completion batch to the reaper (the frozen-cpu
/// monopolist escape hatch — see the stall arm in retire_complete).
/// 16 x 50 ms = 800 ms worst case per close on x86 (4 s on the
/// slower-TCG arch budgets), then close(2) returns; a healthy boot
/// never reaches this (grace passes in the microsecond window).
const FDREAP_MAX_STALLS: usize = 16;

/// Park a removed slot occupant and COMPLETE its close decision before
/// returning — the reference task_work parity form for syscall-context
/// displacement (close(2), dup2(2)/dup3 displacement). Writers call this
/// AFTER releasing their entry lock (R14-5: no close op under it).
///
/// Linux runs the final fput in task work at the return-to-userspace
/// boundary, so a userspace observer never sees a closed fd whose side
/// effects have not run. This function restores that: the close
/// decisions (record-lock release, ops.close — pipe reader drop,
/// epoll purge — and the final Arc drop's IN_CLOSE_* fsnotify) complete
/// in the closer's own context (or the racing reaper's, which this wait
/// observes) before the syscall returns.
pub fn retire_complete(file: Arc<File>) {
    if !LIVE.load(Ordering::Acquire) || crate::sched::current().is_none() {
        // Reaper down (spawn failed / pre-init) or no task context:
        // inline bounded grace for this one file here. close(2) latency
        // only — never correctness.
        if crate::sync::rcu::synchronize_rcu_forced(GP_BUDGET_MS, "fdreap-inline") {
            FdTable::displaced_close_decision(&file);
            return;
        }
        // Timed out (monopolist): fall through to the queue; when the
        // reaper is up it retries, and if it never comes up the final
        // table Drop re-runs the decision.
    }
    let my_ord = {
        let mut q = RETIRED.lock_irqsave();
        q.push(file);
        PUSHED_SEQ.fetch_add(1, Ordering::AcqRel) + 1
    };
    RETIRED_DEPTH.fetch_add(1, Ordering::Relaxed);
    // NOTE: no TICKET.up() on this happy path. The closer below drains
    // the batch itself (one grace covers it), so an up here only wakes
    // fdreapd to find an empty queue — one wakeup + IPI per close(2),
    // and worse: a fdreapd that wins the take forces the CLOSER to wait
    // on fdreapd's sleepy (1-jiffy poll) grace form, whose jiffy
    // granularity turned a µs-scale report race into a full 10 ms
    // close(2) stall once per tick (measured 2026-11-05: probe
    // open+close median 1.07 µs, mean 2.45 µs; every stall in close(),
    // tick-periodic, ending at a jiffy boundary; -smp 1 clean at
    // mean 869 ns). With fdreapd asleep the secondary CPU stays halted
    // — quiescent by construction — and the closer's polled grace
    // completes in the microsecond window. fdreapd remains armed for
    // the requeue path (grace-budget exhaustion), which still ups.
    // Completion loop: wait until the close decisions for everything
    // parked up to and including ours have run. Usually we win the race
    // ourselves and drain the batch here (one grace covers the batch).
    let mut stalls = 0usize;
    while PROCESSED_SEQ.load(Ordering::Acquire) < my_ord {
        let batch = {
            let mut q = RETIRED.lock_irqsave();
            core::mem::take(&mut *q)
        };
        if batch.is_empty() {
            // Ours is inside the reaper's in-flight batch (between its
            // take and its decisions). Yield: the context switch both
            // lets the reaper run and reports our CPU's quiescent
            // state, which is what its grace wait needs to finish.
            crate::sched::schedule();
            continue;
        }
        let n = batch.len();
        RETIRED_DEPTH.fetch_sub(n, Ordering::Relaxed);
        if crate::sync::rcu::synchronize_rcu_forced_polled(GP_BUDGET_MS, "fdreap-close") {
            for f in &batch {
                // Sleepable syscall context: record-lock release and
                // ops.close may take mutexes, wake peers, or queue work
                // (the same discipline close_fd ran before the retire
                // reaper existed).
                FdTable::displaced_close_decision(f);
            }
            // Drop the batch Arcs HERE, in the closer's context: the
            // last-reference drops fire IN_CLOSE_* fsnotify, flock/OFD
            // release and lease bookkeeping synchronously — the
            // userspace-observable close semantics.
            drop(batch);
            PROCESSED_SEQ.fetch_add(n, Ordering::AcqRel);
        } else {
            // Grace did not pass (monopolist window): requeue — ours and
            // the batch's — and retry. Correctness requires completion
            // before return; the reaper retries in parallel.
            GRACE_STALLS.fetch_add(1, Ordering::Relaxed);
            stalls += 1;
            if stalls == 8 || stalls % 64 == 0 {
                crate::pr_warn!(
                    "fs: fdreap close completion stalled (grace budget exceeded {}x)",
                    stalls
                );
            }
            // 51st session — the monopolist escape hatch. The OH wedge
            // chain (ohc10-ohc18 evidence): one CPU's quiescent
            // generation froze permanently (RCU-GP-BOUND qs=N,M with M
            // never advancing), and every close(2) then blocked HERE
            // forever — hilogd's log rotation froze first (t~5s), its
            // socket backpressure froze every service holding a lock,
            // and the whole boot wedged (pixels 82f0a25f; debug AND
            // release kernels, with or without instruments). The
            // reference kernel's close(2) NEVER blocks on an RCU grace
            // (fput defers to task_work at return-to-userspace), so
            // unconditionally requiring a full grace in the closer's
            // context is our divergence. After FDREAP_MAX_STALLS failed
            // budgets, leave the batch to the reaper (it retries and
            // completes the decisions whenever the grace finally
            // passes) and RETURN — the user-visible close semantics
            // (peer EOF etc.) lag by the monopolist's lifetime in
            // exactly this pathological case, instead of the whole
            // system freezing behind close(2).
            if stalls >= FDREAP_MAX_STALLS {
                crate::pr_warn!(
                    "fs: fdreap close deferring completion after {} budgets (frozen-cpu monopolist) — reaper owns the batch",
                    stalls
                );
                RETIRED_DEPTH.fetch_add(n, Ordering::Relaxed);
                {
                    let mut q = RETIRED.lock_irqsave();
                    q.extend(batch);
                }
                TICKET.up();
                return;
            }
            RETIRED_DEPTH.fetch_add(n, Ordering::Relaxed);
            {
                let mut q = RETIRED.lock_irqsave();
                q.extend(batch);
            }
            TICKET.up();
        }
    }
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
        let n = batch.len();
        for file in &batch {
            // Sleepable context: record-lock release and ops.close may
            // take mutexes, wake peers, or queue work (the discipline
            // close_fd ran at its drain point, moved here).
            FdTable::displaced_close_decision(file);
        }
        // Drop the batch Arcs (final-reference IN_CLOSE_* fsnotify and
        // lock/lease release run at the drops), then publish completion
        // to any retire_complete waiter.
        drop(batch);
        PROCESSED_SEQ.fetch_add(n, Ordering::AcqRel);
    }
    0
}
