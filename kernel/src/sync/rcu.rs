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

/// Consecutive failed grace rounds (reset by any passing round) and the
/// one-shot latch for the frozen-CPU task dump — see wait_qs_sleepy.
static GP_CONSEC_FAIL: AtomicU32 = AtomicU32::new(0);
static GP_DUMPED: AtomicBool = AtomicBool::new(false);

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
            if !cpu_quiescent(i, gen) {
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

/// Bounded grace-period wait — the exit-path liveness fix (lb125 decode).
///
/// The single sticking point of the bimodal OH boots: a CPU whose current
/// task runs unpreemptible for a long window (phantom preempt counts, the
/// monopolist family, or any long preempt-off kernel loop) reports NO
/// quiescent state, so an UNBOUNDED synchronize_rcu() stalls for the whole
/// window. The exit path waits it in THREE places (MmStruct::drop's raw-
/// borrower grace wait, thread kernel-stack free, release_task's task-
/// memory free) — a stalled exit cannot reach ZOMBIE + the deferred
/// SIGCHLD, and init — whose whole loop serializes on child exits —
/// freezes with it: no params, no service starts, no reaping. On lb125
/// the chipset_init child (pid 466) exited at 42.03s and init learned at
/// 105.39s (63s inside the grace wait): every service-start freeze,
/// device_manager 15s-timeout chain and the crawl-vs-healthy bimodality
/// of the whole campaign queued behind that one wait.
///
/// Our RCU read sections are preempt-disable-based and microseconds long
/// (pid_hash lookups); every real reader is done in far less than the
/// bound. A wait that exceeds it is by construction a monopolist window,
/// not a reader — proceed and NAME it. The wall-clock protections the
/// exit path already stacks on top (vma_write walker exclusion leaves an
/// empty tree for late readers; deferred_free holds the struct free
/// another 1.5 s) are unchanged.
pub fn synchronize_rcu_bounded(max_ms: u64, site: &'static str) {
    let _ = synchronize_rcu_bounded_checked(max_ms, site);
}

/// Bounded grace-period wait that reports whether the grace period
/// completed in budget. Callers whose drop REQUIRES the grace (the
/// fd-table retire reaper requeues its batch on a timeout instead of
/// dropping) need the outcome; everyone else keeps the void form.
///
/// NOTE (fake-grace hazard): this form snapshots the current RCU_GEN
/// without advancing it. The generation only advances when an RCU
/// softirq finishes a callback batch, so on a quiet system the snapshot
/// equals the value every CPU already reported at the LAST grace — the
/// wait returns instantly with no grace at all. Correct only for
/// callers whose lifetimes do not hinge on a fresh grace. Lifetime
/// callers use `synchronize_rcu_forced`.
pub fn synchronize_rcu_bounded_checked(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.load(Ordering::Acquire);
    announce_and_wait(gen, max_ms, site)
}

/// Snapshot-form grace wait with SLEEPING polls — the exit-path form.
///
/// Same snapshot semantics as `synchronize_rcu_bounded_checked` (an
/// instant pass stays possible when reports are already fresh — e.g.
/// right after another waiter's round), but the wait SLEEPS between
/// checks instead of yielding in a runnable spin: a waiter that is the
/// only runnable task on its CPU repicks itself through `schedule()`
/// and never idles, which on single-accelerator boots both burns the
/// CPU and starves the tick (measured as the ~1s TIMER-SOFTIRQ-STALL
/// class during long userspace phases). The exit paths pay this wait
/// per exit, so the fast-pass matters: keep the snapshot, sleep only
/// while actually behind.
pub fn synchronize_rcu_bounded_sleepy(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.load(Ordering::Acquire);
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();
    wait_qs_sleepy(gen, max_ms, site)
}

/// TRUE bounded grace period, for callers that free objects on its
/// strength: advance RCU_GEN first, so every quiescent-state report
/// already recorded is BELOW the waited-for generation and cannot
/// satisfy the check. Every CPU must then pass a report site
/// (user-return, context switch, idle) that executed after this call
/// began — which is exactly the guarantee an RCU read-side window
/// (preempt-off load→increment, no schedule point inside) needs.
pub fn synchronize_rcu_forced(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    announce_and_wait(gen, max_ms, site)
}

/// Forced grace period with SLEEPING polls — the fd-table reaper's form.
///
/// Same protocol as `synchronize_rcu_forced` (forced epoch, every CPU
/// must report a fresh QS), but the wait SLEEPS one jiffy between
/// checks instead of yielding in a runnable loop: a kthread that is the
/// only runnable task on its CPU repicks itself through `schedule()`
/// (the same-task repick fast path never enters idle), so its CPU never
/// reaches an idle QS report and a quiet boot livelocks the wait. The
/// sleep is the khungtaskd idiom: arm a one-shot timer, INTERRUPTIBLE,
/// re-check, schedule, clean up the racing timer. Checks come first —
/// a grace round whose reporters are already past (idle and
/// syscall-exiting CPUs report continuously while a waiter is armed)
/// still completes without sleeping at all.
pub fn synchronize_rcu_forced_sleepy(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();
    wait_qs_sleepy(gen, max_ms, site)
}

/// Forced grace period with a bounded POLL phase before the sleepy
/// fallback — the fd-close completion form (r26).
///
/// A busy peer CPU stores its QS report at its next return to user
/// (microseconds away), but the sleepy form would only notice at its
/// next 1-jiffy poll — turning every synchronous close into a ~1-jiffy
/// stall (measured: LTP inotify11's 10k-file churn blew its 30 s
/// budget). This form spin-polls (with the pause hint) for the
/// typical µs-scale report window FIRST, and only falls back to the
/// sleeping poll for a genuinely monopolist CPU.
pub fn synchronize_rcu_forced_polled(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();
    // Poll phase: ~100 µs of pause-hinted checks covers a peer's
    // user-return several times over on KVM.
    for _ in 0..50_000 {
        if all_online_cpus_reported(gen) {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        core::hint::spin_loop();
    }
    wait_qs_sleepy(gen, max_ms, site)
}

/// The sleeping poll loop shared by the sleepy wait forms: check first
/// (fresh reporters pass without ever sleeping), then one-jiffy
/// INTERRUPTIBLE sleeps armed by a one-shot timer (the khungtaskd
/// idiom), until every online CPU reports at/after `gen` or the budget
/// expires. Returns whether the round completed.
fn wait_qs_sleepy(gen: u32, max_ms: u64, site: &'static str) -> bool {
    use crate::process::task::TaskState;

    let hz = crate::drivers::timer::HZ as u64;
    let deadline = crate::drivers::timer::get_jiffies() + max_ms * hz / 1000;

    loop {
        if all_online_cpus_reported(gen) {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            GP_CONSEC_FAIL.store(0, Ordering::Relaxed);
            return true;
        }
        if crate::drivers::timer::get_jiffies() >= deadline {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            static SGP_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = SGP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 200 {
                // Explicit integers, not {:?} arrays — array Debug
                // formatting pulls enough core::fmt code to push the
                // debug image over the boot.S 20MB window (the window
                // margin is single-digit KB).
                let (q0, q1, q2, q3) = (
                    RCU_QS_GEN[0].load(Ordering::Acquire),
                    RCU_QS_GEN[1].load(Ordering::Acquire),
                    RCU_QS_GEN[2].load(Ordering::Acquire),
                    RCU_QS_GEN[3].load(Ordering::Acquire),
                );
                // Name the monopolist: each NON-quiescent online CPU's
                // current task (comm/pid/preempt_count). A stuck qs with a
                // non-idle current means that task has not returned to
                // user or scheduled since its last report — the phantom
                // preempt-count / kernel busy-loop family.
                for c in 0..4 {
                    if !crate::sched::sched::cpu_online(c)
                        || crate::sched::sched::cpu_running_idle(c)
                        || cpu_quiescent(c, gen)
                    {
                        continue;
                    }
                    let cur = crate::sched::sched::cpu_state(c).current;
                    if !cur.is_null() {
                        let comm: &[u8; 16] = unsafe { (*cur).comm() };
                        let comm_s = core::str::from_utf8(comm)
                            .unwrap_or("?");
                        let nr = unsafe { (*cur).last_syscall_nr.load(Ordering::Relaxed) };
                        let a0 = unsafe { (*cur).last_syscall_a0.load(Ordering::Relaxed) };
                        crate::pr_warn!(
                            "RCU-STUCK-CPU{} task={} pid={} preempt={} lastsys={} a0={:#x}",
                            c,
                            comm_s.split(char::from(0)).next().unwrap_or("?"),
                            unsafe { (*cur).pid() },
                            unsafe { (*cur).preempt_count() },
                            nr,
                            a0
                        );
                    }
                }
                crate::pr_warn!(
                    "RCU-GP-BOUND site={} waited>{}ms (sleepy): no full quiescent round gen={} qs={},{},{},{} on={},{},{},{}",
                    site,
                    max_ms,
                    gen,
                    q0,
                    q1,
                    q2,
                    q3,
                    crate::sched::sched::cpu_online(0),
                    crate::sched::sched::cpu_online(1),
                    crate::sched::sched::cpu_online(2),
                    crate::sched::sched::cpu_online(3)
                );
            }
            // Comprehensive-fix residual hunt (2026-11-04, cgf1): a CPU
            // frozen non-quiescent across MANY consecutive grace budgets
            // is the wedge-family monopolist (IF=0/preempt-off holder —
            // its own tick cannot fire, so every tick-side instrument is
            // blind to it; the 13th/25th cross-CPU-dump prescription).
            // This waiter runs on ANOTHER CPU in task (syscall) context,
            // so it can walk the frozen CPU's tasks. One-shot per boot;
            // fires only after 32 consecutive failed rounds (~1.6s+ of a
            // frozen CPU at the 50ms fdreap budget) — never on a healthy
            // boot (a passing grace resets the counter above).
            if GP_CONSEC_FAIL.fetch_add(1, Ordering::Relaxed) + 1 == 32
                && !GP_DUMPED.swap(true, Ordering::AcqRel)
            {
                crate::debug::taskdump::dump_all_tasks("rcu-gp-frozen-cpu");
            }
            return false;
        }

        let current = match crate::sched::current() {
            Some(t) => t,
            None => {
                // Early-boot / no-task context: no sleep possible —
                // plain yield rounds keep the wait alive.
                crate::sched::schedule();
                continue;
            }
        };
        // SAFETY: current is this task's own pointer.
        let my_pid = unsafe { (*current).pid() };
        let target = crate::drivers::timer::get_jiffies() + 1;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        if timer_id != 0 {
            // SAFETY: current is this task's own pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            // Re-check QS AFTER marking sleeping: the last missing
            // reporter typically reports in the microseconds between
            // the failed check at the loop top and here (the fd-close
            // completion loop yields through schedule(), which is a
            // report site). Aborting the sleep when it lands converts
            // what was otherwise a full 1-jiffy stall — measured
            // 2026-11-05: one close(2) per 10 ms tick stalled up to
            // 10 ms in fdreapd's sleepy wait (probe: open+close median
            // 1.07 us but mean 2.45 us, every stall ending at a jiffy
            // boundary) — back into the microsecond poll window.
            if all_online_cpus_reported(gen) {
                // SAFETY: current is this task's own pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while
                    // still executing — take ourselves back off.
                    crate::sched::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
                continue;
            }
            // Re-check the one-shot AFTER marking sleeping: it may have
            // fired between arming and set_state (khungtaskd lesson).
            if crate::drivers::timer::get_jiffies() >= target {
                // SAFETY: current is this task's own pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while
                    // still executing — take ourselves back off.
                    crate::sched::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::cpu::restore_irq(true);
                crate::sched::schedule();
                // Spurious wake before the deadline: drop the timer and
                // loop (re-armed next iteration).
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full — stay runnable, yield one round.
            crate::sched::schedule();
        }
    }
}

/// Whether every online CPU has reported a quiescent state at or after
/// generation `gen`.
fn all_online_cpus_reported(gen: u32) -> bool {
    for i in 0..MAX_CPUS {
        if !crate::sched::sched::cpu_online(i) {
            continue;
        }
        if !cpu_quiescent(i, gen) {
            return false;
        }
    }
    true
}

/// Whether CPU `i` is quiescent with respect to generation `gen`:
/// either it stored a QS report at/after `gen`, or it is currently
/// executing its idle task. A halted idle CPU stores reports only once
/// per wake, so its `RCU_QS_GEN` entry can lag arbitrarily far behind
/// reality while the CPU is in fact executing nothing — the fd-table
/// reader window (get_file's load→increment) runs only over a non-idle
/// `current`, so idle-loop execution is quiescent by construction.
/// This keeps grace periods from stalling on parked secondary CPUs
/// (r26: without it, every synchronous close-completion wait paid the
/// sleepy poll's 1-jiffy granularity).
#[inline]
fn cpu_quiescent(i: usize, gen: u32) -> bool {
    RCU_QS_GEN[i].load(Ordering::Acquire) >= gen
        || crate::sched::sched::cpu_running_idle(i)
}

/// Shared body of both bounded waits: announce a waiter, report our own
/// QS, then wait (bounded, resched between checks) for every online
/// CPU's QS generation to reach `gen`.
fn announce_and_wait(gen: u32, max_ms: u64, site: &'static str) -> bool {
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();

    let hz = crate::drivers::timer::HZ as u64;
    let deadline = crate::drivers::timer::get_jiffies() + max_ms * hz / 1000;

    loop {
        let mut all_qs = true;
        let mut stuck_cpu = 0usize;
        for i in 0..MAX_CPUS {
            if !crate::sched::sched::cpu_online(i) {
                continue;
            }
            if !cpu_quiescent(i, gen) {
                all_qs = false;
                stuck_cpu = i;
                break;
            }
        }
        if all_qs {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        if crate::drivers::timer::get_jiffies() >= deadline {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            static GPB_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = GPB_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 200 {
                crate::pr_warn!(
                    "RCU-GP-BOUND site={} waited>{}ms: cpu{} reports no quiescent state (monopolist window)",
                    site,
                    max_ms,
                    stuck_cpu
                );
            }
            return false;
        }
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
