//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Sleeping mutex — the coarse-lock primitive (reference recipe).
//!
//! This is the kernel's mutex for locks that may be held across blocking
//! I/O (VFS_MUTATION_LOCK, EXT4_BIG_LOCK). It follows the reference
//! kernel's locking discipline (see run notes of the fix-lostpark lane,
//! "Design review vs the reference kernel"):
//!
//! - Rule 0: a lock that may be held across blocking I/O is a MUTEX,
//!   not a spinlock. Holders of this lock may sleep in virtio I/O for
//!   milliseconds at a time; waiters therefore park on a wait queue
//!   instead of spinning preempt-disabled (a preempt-disabled spinner
//!   cannot be preempted to run the holder — the LTP r2 WEDGE family).
//! - Rule 1: spinlocks never sleep. The only spinlock here is the
//!   internal `state` word, held for a few instructions and never
//!   across I/O.
//! - Rule 2: the sleeping slowpath is queue-based and wake-safe:
//!   `prepare_to_wait` registers the waiter and sets its sleep state,
//!   the park site is the waiter's OWN `schedule()`, and release wakes
//!   the queue head (FIFO, wake-one handoff). No bare yield exists
//!   anywhere in the acquire.
//!
//! # Context restrictions
//!
//! **NOT usable from interrupt context.** A waiter must be able to call
//! `schedule()`; hardirq/softirq/NMI context may never sleep. Two
//! defensive degradations are kept from the fs-era implementation
//! (behavioral compatibility):
//!   - no current task (very early boot, secondary harts): degrade to
//!     pure spinning — single-CPU boot paths, the holder is on this CPU;
//!   - preempt_count != 0 (caller inside a spinlock section — a lock
//!     ordering bug): degrade to pure spinning rather than refuse, but
//!     this path must never be reached deliberately; a task that holds
//!     a spinlock and then acquires a sleeping mutex violates the
//!     hierarchy in docs/architecture/lock-ordering.md.
//!
//! # Fairness (R84/R85 history, preserved)
//!
//! Wake-one alone is not a handoff: a fresh fast-path caller can barge
//! past a woken-but-not-yet-scheduled waiter indefinitely under KVM's
//! .so page-fault churn (the b139 starvation: 10+ s D-state waiters
//! while holders cycled). The acquire therefore gates fresh sleepable
//! callers on a waiters count (parked tasks + unspent wake tokens),
//! the woken waiter spends its token as unconditional ownership
//! (reference mutex HANDOFF in miniature), and an orphaned handoff
//! (wake dropped on an empty queue) is recovered through a handoff
//! epoch plus an anti-starvation self-wake timer. All of this is
//! bounded-latency insurance: in normal operation tokens flow FIFO and
//! the overrides never fire.
//!
//! # Recursion
//!
//! Recursive per task (owner pid + depth): a loop device backed by a
//! file on another ext4 instance re-enters ext4 from inside the block
//! layer while holding the big lock, so nested acquire by the SAME
//! task is legal and the outermost guard releases.
//!
//! # Lock-ordering position
//!
//! This primitive sits OUTSIDE the spinlock hierarchy: it is taken in
//! syscall/task context, may be nested inside another SleepMutex
//! (VFS_MUTATION_LOCK -> EXT4_BIG_LOCK is the sanctioned order), and
//! everything it protects is above the state spinlock. It must never
//! be taken while holding any spinlock.

use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

/// R77-style stall tripwire support: is the recorded owner still a live
/// task? (pid-hash lookup; the stall tripwire only fires while the owner
/// lives — a DEAD owner means the lock leaked with its holder unless
/// release_if_owner ran.)
fn locked_owner_alive_wait(owner: &u32) -> bool {
    if *owner == 0 {
        return false;
    }
    let t = crate::process::pid_hash::pid_hash_lookup_pinned(*owner);
    if t.is_null() {
        return false;
    }
    let alive = unsafe { !(*t).state().is_dead() };
    crate::process::task::Task::task_put(t);
    alive
}

struct SleepMutexState {
    locked: bool,
    owner: u32,
    depth: u32,
    /// Jiffies stamp of the outermost acquire — with the waiters'
    /// tripwire below it names a holder that never releases (a leaked
    /// lock after a synchronous kill inside the critical section parks
    /// every later user in D-state forever).
    acquired_at: u64,
    /// R84 fairness: tasks parked on `wait` PLUS tasks holding an
    /// unspent release() wake token. A task enters the count when it
    /// first registers on the queue and leaves it in the same critical
    /// section in which it takes the lock (token spend or sole-waiter
    /// take). While nonzero, the fair fast path refuses fresh sleepable
    /// acquirers.
    waiters: u32,
    /// R85: orphaned-handoff epoch. release() hands the lock to ONE
    /// waiter via wake_up_one; when the queue is empty at that moment
    /// (every counted waiter is mid-loop between finish_wait and its
    /// next prepare_to_wait) the wake is dropped and the lock sits FREE
    /// behind a nonzero `waiters` with nobody left to wake it. Each such
    /// drop bumps this epoch: a waiter whose registration snapshot of
    /// the epoch predates the bump may take the free lock at its
    /// re-check, so an orphaned handoff is recovered by the FIRST
    /// mid-loop waiter to re-register.
    handoff_epoch: u64,
}

/// Sleeping mutex for coarse, I/O-holding serialization (see the module
/// doc). All users are `static`s, hence the `&'static self` API.
pub struct SleepMutex {
    /// The lock word + bookkeeping. Held only for a few instructions at
    /// a time (never across I/O) — a genuine spinlock per rule 1.
    state: Spinlock<SleepMutexState>,
    /// Tasks blocked while another task holds the mutex (the holder may
    /// sleep in block I/O for a long time).
    wait: WaitQueueHead,
    /// Diagnostic name — used verbatim in the stall/census prints
    /// (e.g. "EXT4-BIGLOCK-STALL"); keep stable, logs parse them.
    name: &'static str,
    /// Whether fsw (fs-write census) notes fire on acquire/contend.
    /// True only for the ext4 big lock so its counters stay comparable
    /// across the perf gate; other instances do not pollute them.
    fsw_notes: bool,
}

impl SleepMutex {
    /// R84 fairness: a waiter older than this may take a FREE lock at
    /// the registration re-check even with other waiters queued — the
    /// self-heal that bounds any residual gate pathology at a few
    /// seconds of extra wait instead of an unbounded wedge.
    const ANTI_STARVE_JIFFIES: u64 = crate::config::KERNEL_HZ as u64 * 5;

    /// R84: stall tripwire threshold on the WAITER's own queue age.
    /// Normal parking lasts milliseconds; 3 s queued means the handoff
    /// is not reaching this waiter.
    const STALL_WAITER_JIFFIES: u64 = crate::config::KERNEL_HZ as u64 * 3;

    /// A waiter parked longer than this may take a free lock at the
    /// registration re-check even with other waiters queued: bounded-
    /// latency self-heal so the gate itself can never strand a free
    /// lock behind a stuck count.
    pub const fn new(name: &'static str, fsw_notes: bool) -> Self {
        Self {
            state: Spinlock::new(SleepMutexState {
                locked: false,
                owner: 0,
                depth: 0,
                acquired_at: 0,
                waiters: 0,
                handoff_epoch: 0,
            }),
            wait: WaitQueueHead::new(),
            name,
            fsw_notes,
        }
    }

    /// Try to take the lock / recurse into it, UNGATED: a free lock goes
    /// to whoever asks. Caller must NOT hold the state spinlock.
    ///
    /// R84: this is not the general acquire. It survives for exactly the
    /// callers the queue cannot serve: the non-sleeping spinner in
    /// lock() (no task context or preempt_count != 0 — it cannot
    /// register on the waitqueue, so gating it would livelock it behind
    /// a queue that never drains under load).
    ///
    /// The recursion arm requires `locked`: `owner == 0` also names the
    /// FREE state, and a pid-0 caller (get_current_pid() with no
    /// current task — the very contexts the degrade path serves) would
    /// otherwise "recurse into" a momentarily free lock during a
    /// handoff window (depth=1 on locked=false; the bogus guard's later
    /// release can then free a lock another task genuinely holds).
    /// Found by the S-1 loom harness (kernel/verify mutex_loom.rs).
    fn try_acquire_or_recurse(&'static self, pid: u32) -> Option<SleepMutexGuard> {
        let mut st = self.state.lock();
        if !st.locked {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::drivers::timer::get_jiffies();
            Some(SleepMutexGuard { lock: self })
        } else if st.locked && st.owner == pid {
            st.depth += 1;
            Some(SleepMutexGuard { lock: self })
        } else {
            None
        }
    }

    /// R84 fairness: the acquire for fresh sleepable callers. Identical
    /// to try_acquire_or_recurse except a FREE lock is only taken when
    /// no waiter is parked and no wake token is outstanding
    /// (`waiters == 0`) — everyone else must queue. This is what stops
    /// the barging: without it, any task on another CPU could take the
    /// just-released lock during the scheduling window of the waiter
    /// that release() had woken, and under KVM's .so-page-fault churn
    /// the bargers never stopped.
    ///
    /// The recursion arm requires `locked` for the same reason as in
    /// try_acquire_or_recurse: lock() runs its adaptive fair-try BEFORE
    /// the can_sleep check, so a pid-0 no-task caller reaches this arm;
    /// gated on the free branch (waiters > 0), it must be refused, not
    /// "recursed" into the free lock via the owner == 0 alias.
    fn try_acquire_or_recurse_fair(&'static self, pid: u32) -> Option<SleepMutexGuard> {
        let mut st = self.state.lock();
        if !st.locked && st.waiters == 0 {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::drivers::timer::get_jiffies();
            Some(SleepMutexGuard { lock: self })
        } else if st.locked && st.owner == pid {
            st.depth += 1;
            Some(SleepMutexGuard { lock: self })
        } else {
            None
        }
    }

    /// R84 fairness: spend the release() handoff token. The woken
    /// waiter takes a FREE lock unconditionally and leaves the waiter
    /// count in the same critical section — the count keeps covering the
    /// token holder for the whole wake-to-take window, which is what
    /// holds fresh callers at the gate until ownership has actually
    /// changed hands. A held lock or a lost race against another token
    /// holder returns None: the caller re-parks and keeps its count.
    ///
    /// The recursion arm requires `locked` (same owner == 0 alias as
    /// the other acquires; see try_acquire_or_recurse).
    fn try_acquire_wake_token(&'static self, pid: u32) -> Option<SleepMutexGuard> {
        let mut st = self.state.lock();
        if st.locked && st.owner == pid {
            st.depth += 1;
            Some(SleepMutexGuard { lock: self })
        } else if !st.locked {
            if st.waiters > 0 {
                st.waiters -= 1;
            }
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::drivers::timer::get_jiffies();
            Some(SleepMutexGuard { lock: self })
        } else {
            None
        }
    }

    /// Acquire (sleep on contention, recursive per task).
    ///
    /// NOT for interrupt context (see module doc); syscall/task context
    /// only. Returns a guard that releases on drop (outermost only).
    pub fn lock(&'static self) -> SleepMutexGuard {
        let pid = crate::process::current_pid();
        if self.fsw_notes {
            crate::dfx::fsw::note_biglock();
        }

        // Adaptive: a short spin first covers the common case of a
        // holder that is on-CPU and about to release (boot-time short
        // sections never touch the waitqueue at all). R84: gated so the
        // spin cannot barge past parked waiters either.
        for _ in 0..4 {
            if let Some(g) = self.try_acquire_or_recurse_fair(pid) {
                return g;
            }
            for _ in 0..64 {
                core::hint::spin_loop();
            }
        }

        // Contended for real. If we have no task context (early boot /
        // IRQ) we cannot sleep — degrade to pure spin (see module doc:
        // IRQ-context use is a bug; this is a defensive fallback). A
        // task that holds a spinlock (preempt_count != 0) cannot sleep
        // EITHER: __schedule's preempt discipline refuses to switch it
        // out, so the wait-then-schedule loop below degenerates into a
        // hot retry loop whose dequeue_if_enqueued hammers the GRQ lock
        // and starves the other CPUs' timer ticks (the sendmsg02
        // stall-quiet wedge). Spin instead: interrupts stay enabled,
        // the scheduler is never touched, and whichever CPU runs the
        // holder can still take its ticks and release us.
        let can_sleep = crate::sched::current().is_some()
            && crate::interrupt::preempt::preempt_count() == 0;
        if self.fsw_notes {
            crate::dfx::fsw::note_biglock_contend();
        }
        let cur = match crate::sched::current() {
            Some(t) if can_sleep => t,
            _ => {
                loop {
                    if let Some(g) = self.try_acquire_or_recurse(pid) {
                        return g;
                    }
                    for _ in 0..64 {
                        core::hint::spin_loop();
                    }
                }
            }
        };

        // R83b: baseline header snapshot for the waiter self-watch below.
        let mut wait_snap = crate::dfx::r83hunt::WatchSnap {
            ksp: 0, user_sp: 0, preempt: 0, pick: 0, claim: 0, flags: 0,
        };
        let mut wait_snap_set = false;

        // R84 waiter bookkeeping:
        //  counted      <=> st.waiters already includes us (we are on the
        //                   queue or hold an unspent wake token); the count
        //                   is entered once at first registration and left
        //                   in the same critical section as a take.
        //  woken_retry  <=> schedule() just returned: the release() that
        //                   woke us spent its one handoff token on us.
        //  first_park   <=> jiffies of our FIRST registration; re-parks
        //                   keep it so the anti-starvation override below
        //                   measures true queue age.
        //  timer_id     <=> R85: anti-starve deadline wake armed for the
        //                   CURRENT park (0 = none). A parked waiter has
        //                   no waker of its own when a handoff is dropped
        //                   (empty-queue release) or its token is lost —
        //                   without this timer the ANTI_STARVE override
        //                   below can never fire, because it is only
        //                   evaluated on a loop iteration and iterating
        //                   requires a wake.
        let mut counted = false;
        let mut woken_retry = false;
        let mut first_park = 0u64;
        let mut timer_id = 0u64;

        loop {
            if woken_retry {
                // Handoff: the free lock is ours if it is still free.
                // Losing it means another token holder won the race or a
                // spurious wake — fall through and re-park like any
                // contender (our count stays).
                if let Some(g) = self.try_acquire_wake_token(pid) {
                    counted = false;
                    return g;
                }
                woken_retry = false;
            }
            if let Some(g) = self.try_acquire_or_recurse_fair(pid) {
                return g;
            }
            // R83b (R74 producer hunt): every capture so far had the
            // victim parked in THIS wait when its ti_kernel_sp turned to
            // foreign data. ti_kernel_sp/ti_user_sp have NO legal writer
            // while a task sleeps here (their only writers are the task's
            // own trap exit / exec paths), so any change between two
            // iterations of this loop is the scribbler — reported from
            // inside the exposure window itself.
            {
                let now_snap = crate::dfx::r83hunt::watch_snap(cur as *mut _);
                if !wait_snap_set {
                    wait_snap = now_snap;
                    wait_snap_set = true;
                } else if now_snap.ksp != wait_snap.ksp || now_snap.user_sp != wait_snap.user_sp {
                    crate::dfx::r83hunt::waitwatch_report(
                        cur as *mut _,
                        &wait_snap,
                        &now_snap,
                    );
                    wait_snap = now_snap;
                }
            }
            // Register on the wait queue BEFORE re-checking the state
            // (R7-B6b discipline): any release() that pairs with this
            // attempt finds us queued and hands its wake token to us.
            // EXCLUSIVE + tail insert => FIFO, one wake per release.
            // The state set here is the waiter's OWN sleep state, set
            // immediately before its OWN schedule() below — reference
            // rule 2's park-site ownership.
            self.wait.prepare_to_wait(cur, true, false);
            // R85: snapshot the orphaned-handoff epoch BEFORE the
            // registration critical section below — a release() that
            // dropped its wake (empty queue) between this read and the
            // re-check shows up as an epoch change, letting us take the
            // free lock it stranded instead of parking past it.
            let my_epoch = {
                let st = self.state.lock();
                st.handoff_epoch
            };
            // R7-B6b re-check, fairness-aware (R84): count ourselves under
            // the state lock — once; re-parks keep the count — then take
            // the free lock ONLY when no OTHER waiter/token exists
            // (`waiters == 1`: us). That covers the release that raced
            // our registration (its wake found an empty queue — nobody
            // else can ever hand the lock out, so we must). With older
            // waiters queued the pending handoff belongs to them and we
            // sleep like everyone else. The anti-starvation override
            // (parked > ANTI_STARVE_JIFFIES and the lock is free) is the
            // self-heal that guarantees the gate can never strand a free
            // lock behind a stuck count. R85 adds the epoch term: a
            // handoff that was DROPPED (empty-queue release bumped the
            // epoch after we registered) belongs to nobody — take the
            // free lock now.
            {
                let now = crate::drivers::timer::get_jiffies();
                let mut st = self.state.lock();
                if !counted {
                    st.waiters += 1;
                    counted = true;
                    first_park = now;
                }
                // The recursion arm requires `locked` (owner == 0 also
                // names the free state; see try_acquire_or_recurse).
                let take = if st.locked && st.owner == pid {
                    // Unreachable while parked (a counted waiter has not
                    // taken the lock, so it cannot own it), kept so
                    // recursion is honored on every path.
                    st.waiters -= 1;
                    counted = false;
                    st.depth += 1;
                    true
                } else if !st.locked
                    && (st.waiters == 1
                        || st.handoff_epoch != my_epoch
                        || now.wrapping_sub(first_park)
                            > Self::ANTI_STARVE_JIFFIES)
                {
                    st.waiters -= 1;
                    counted = false;
                    st.locked = true;
                    st.owner = pid;
                    st.depth = 1;
                    st.acquired_at = now;
                    true
                } else {
                    false
                };
                drop(st);
                if take {
                    self.wait.finish_wait(cur);
                    // A concurrent release()'s wake_up_one may have seen
                    // us queued+sleeping and enqueued us between the
                    // registration and the acquire — undo it (the
                    // per-class on_rq guards make it a no-op when we
                    // were not actually enqueued). Mirrors Semaphore::down.
                    crate::sched::dequeue_task(&*cur);
                    if timer_id != 0 && crate::timer::timer_pending(timer_id) {
                        crate::timer::del_timer(timer_id);
                    }
                    return SleepMutexGuard { lock: self };
                }
            }
            // Sleep until release() wakes us (UNINTERRUPTIBLE: sections
            // protected by this lock are not signal-interruptible and the
            // guard must be carried back out of the critical section).
            //
            // R77 wedge-hunt tripwire: a holder that never releases
            // (killed synchronously inside the critical section — RAII
            // never runs through do_exit) shows up as held growing. R84:
            // a WAITER parked/retrying for many seconds while holders
            // cycle (held ~0) is the starvation shape the fairness gate
            // exists to kill. R85: the stranded-LOCK-FREE shape (owner=0,
            // waiters stuck, dropped handoff) is reported too — the b148
            // silent crawl printed nothing while the whole cohort sat in
            // D-state against a free lock. Rate-limited to one line per
            // 10 s.
            {
                let now = crate::drivers::timer::get_jiffies();
                let (owner, depth, held, nwaiters, locked) = {
                    let st = self.state.lock();
                    (
                        st.owner,
                        st.depth,
                        now.saturating_sub(st.acquired_at),
                        st.waiters,
                        st.locked,
                    )
                };
                let waited = now.wrapping_sub(first_park);
                let stranded_free = !locked && nwaiters > 0 && waited >= Self::STALL_WAITER_JIFFIES;
                if (waited >= Self::STALL_WAITER_JIFFIES
                    || held >= crate::config::KERNEL_HZ as u64 * 10)
                    && (stranded_free || locked_owner_alive_wait(&owner))
                {
                    static LAST_WAIT_REPORT: core::sync::atomic::AtomicU64 =
                        core::sync::atomic::AtomicU64::new(0);
                    let last = LAST_WAIT_REPORT.load(core::sync::atomic::Ordering::Relaxed);
                    if now.wrapping_sub(last) >= crate::config::KERNEL_HZ as u64 * 10
                        && LAST_WAIT_REPORT
                            .compare_exchange(
                                last,
                                now,
                                core::sync::atomic::Ordering::Relaxed,
                                core::sync::atomic::Ordering::Relaxed,
                            )
                            .is_ok()
                    {
                        let cur_pid = crate::process::current_pid();
                        if stranded_free {
                            crate::pr_err!(
                                "{}-STALL-FREE waiter={} owner=none waiters={} waited={}j (dropped handoff / stuck count)",
                                self.name,
                                cur_pid,
                                nwaiters,
                                waited
                            );
                        } else {
                            crate::pr_err!(
                                "{}-STALL waiter={} holder={} depth={} held={}j waiters={} waited={}j",
                                self.name,
                                cur_pid,
                                owner,
                                depth,
                                held,
                                nwaiters,
                                waited
                            );
                        }
                    }
                }
            }
            // R85: arm the anti-starve self-wake. schedule() below blocks
            // until SOMETHING wakes us; when the handoff was dropped (the
            // empty-queue release) or our token is stolen, that someone
            // does not exist — without this timer the override above can
            // never evaluate and a stranded waiter sleeps indefinitely
            // (re-armed on every re-park so a held lock is polled at
            // ANTI_STARVE granularity, not hot-looped). Timer-table full
            // (id 0) degrades to the old wake-dependent behavior.
            if timer_id != 0 {
                crate::timer::del_timer(timer_id);
                timer_id = 0;
            }
            timer_id = crate::timer::add_timer_wakeup(
                crate::drivers::timer::get_jiffies() + Self::ANTI_STARVE_JIFFIES,
                pid,
            );
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            if timer_id != 0 {
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
                timer_id = 0;
            }
            self.wait.finish_wait(cur);
            // Woken: the release() that issued this wake spent its handoff
            // token on us — spend it at the top of the loop. (R85: a timer
            // or foreign wake ALSO lands here with no token; the token
            // spend fails or succeeds on lock state alone, so a spurious
            // retry is harmless.)
            woken_retry = true;
        }
    }

    /// RAII alias, mirroring the older `Mutex::guard()` spelling.
    pub fn guard(&'static self) -> SleepMutexGuard {
        self.lock()
    }

    fn release(&'static self) {
        {
            let mut st = self.state.lock();
            if st.depth > 0 {
                st.depth -= 1;
                if st.depth == 0 {
                    st.locked = false;
                    st.owner = 0;
                }
            }
        }
        // Hand the lock to one waiter. R84: this wake is a HANDOFF
        // token, not a retry hint — the woken waiter takes the free lock
        // unconditionally in lock(), and try_acquire_or_recurse_fair
        // keeps fresh callers queued behind it until the token is spent.
        // Still done outside the state spinlock: the waker takes the
        // waitqueue lock, the waiter's finish_wait takes it too — never
        // both at once. Waking an empty queue is a no-op (an outstanding
        // token holder takes the free lock on its own), so this is safe
        // even without waiter accounting.
        let woke = self.wait.wake_up_one();
        if woke == 0 {
            // R85: the handoff was DROPPED — the queue was empty, so
            // every counted waiter is mid-loop between finish_wait and
            // its next prepare_to_wait and the just-freed lock has no
            // taker: the fresh-caller gate (waiters > 0) bars everyone
            // else. Bump the orphaned-handoff epoch so the first
            // mid-loop waiter to re-register takes the free lock at its
            // re-check instead of parking past it. (No counted waiters
            // at all: the bump is inert — nobody's registration snapshot
            // can predate it.)
            let mut st = self.state.lock();
            if !st.locked && st.waiters > 0 {
                st.handoff_epoch += 1;
            }
        }
    }

    /// R73 exit-path release: force-drop the lock when its owner dies.
    ///
    /// do_exit() does not unwind Rust guards — a task killed while inside
    /// a critical section (signal during a slow TCG read, OOM kill,
    /// abort) leaks the lock forever: every later user then spins or
    /// sleeps on a dead owner. Called from do_exit only — the dying task
    /// can no longer make forward progress in its critical section, so
    /// dropping the lock is strictly better than stranding the whole
    /// subsystem. Hand out exactly ONE retry token, same as release():
    /// the woken waiter either takes the lock (and its later release()
    /// hands the next token down the queue) or re-sleeps — a storm of
    /// woken retryers is pointless load on the scheduler.
    pub fn release_if_owner(&'static self, pid: u32) {
        let was_held = {
            let mut st = self.state.lock();
            if st.locked && st.owner == pid {
                if st.depth != 1 {
                    crate::pr_err!(
                        "{}: force-released at exit: pid {} depth {} (nested sections abandoned)",
                        self.name,
                        pid,
                        st.depth
                    );
                }
                st.locked = false;
                st.owner = 0;
                st.depth = 0;
                true
            } else {
                false
            }
        };
        if was_held {
            let woke = self.wait.wake_up_one();
            if woke == 0 {
                // R85: same dropped-handoff recovery as release().
                let mut st = self.state.lock();
                if !st.locked && st.waiters > 0 {
                    st.handoff_epoch += 1;
                }
            }
        }
    }

    /// Diagnostic snapshot (VW forensic instrumentation): (locked, owner,
    /// depth, queued, woken-flagged). Called from the UART dump path (RX
    /// IRQ context) — uses TRY-locks only: a blocking lock here would
    /// self-deadlock against an interrupted holder on this CPU and wedge
    /// the dump itself. queued=usize::MAX means "list lock busy".
    pub fn vw_state(&'static self) -> (bool, u32, u32, usize, usize) {
        let mut locked = false;
        let mut owner = 0u32;
        let mut depth = 0u32;
        if let Some(st) = self.state.try_lock() {
            locked = st.locked;
            owner = st.owner;
            depth = st.depth;
        } else {
            owner = u32::MAX; // state lock busy marker
        }
        let (queued, woken) = if let Some((t, w)) = self.wait.vw_queue_census_try() {
            (t, w)
        } else {
            (usize::MAX, 0)
        };
        (locked, owner, depth, queued, woken)
    }

    /// R85 (b148 hunt): one SBI-console census line for the periodic
    /// taskdump — the b148 wedge was invisible because nothing printed
    /// the lock's own state (zero STALL prints while every service sat
    /// D-parked). Try-locks only (taskdump context may be the watchdog);
    /// busy locks print as '?' fields, never block.
    pub fn dfx_census_line(&'static self) {
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
        let (locked, owner, depth, waiters, epoch) = match self.state.try_lock() {
            Some(st) => (st.locked, st.owner, st.depth, st.waiters, st.handoff_epoch),
            None => (false, u32::MAX, 0, u32::MAX, u64::MAX),
        };
        let (queued, woken) = match self.wait.vw_queue_census_try() {
            Some(v) => v,
            None => (usize::MAX, 0),
        };
        taskdump_raw_line(self.name.as_bytes());
        taskdump_raw_line(b"-CENSUS locked=");
        taskdump_dec(locked as u64);
        taskdump_raw_line(b" owner=");
        taskdump_dec(owner as u64);
        taskdump_raw_line(b" depth=");
        taskdump_dec(depth as u64);
        taskdump_raw_line(b" waiters=");
        taskdump_dec(waiters as u64);
        taskdump_raw_line(b" epoch=");
        taskdump_dec(epoch);
        taskdump_raw_line(b" queued=");
        taskdump_dec(queued as u64);
        taskdump_raw_line(b" wokenflag=");
        taskdump_dec(woken as u64);
        taskdump_raw_line(b"\n");
    }
}

/// Guard: releases on drop (outermost only).
pub struct SleepMutexGuard {
    lock: &'static SleepMutex,
}

impl Drop for SleepMutexGuard {
    fn drop(&mut self) {
        self.lock.release();
    }
}
