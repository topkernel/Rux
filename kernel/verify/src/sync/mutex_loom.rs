//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! loom interleaving harness over the extracted SleepMutex handoff state
//! machine (mutex_model.rs) — the S-1 pilot target #1.
//!
//! # Run
//!
//! ```sh
//! cd kernel/verify
//! RUSTFLAGS="--cfg loom" cargo test --lib sync::mutex_loom
//! ```
//!
//! Compiled ONLY with `--cfg loom` (see sync/mod.rs); the plain cargo
//! test / cargo kani / cargo miri paths never build this file.
//!
//! # Model (what is faithful, what is elided)
//!
//! The driver re-implements `SleepMutex::lock()`'s slow path and
//! `release()` control flow around the SHARED, sync-checked
//! `MutexState` transitions. Environment mapping:
//!
//! - `state: Spinlock<SleepMutexState>`  -> `loom::sync::Mutex<MutexState>`
//!   (loom explores interleavings at lock granularity; the internal
//!   TTAS spinlock itself is covered separately by spinlock_test.rs).
//! - `WaitQueueHead` (FIFO, exclusive, tail insert; wake marks the head
//!   entry woken and wakes its task; the entry leaves at the woken
//!   task's finish_wait) -> `queue: Mutex<VecDeque<pid>>` + one
//!   `(Mutex<bool> woken, Condvar)` park slot per task. Popping on
//!   wake is behaviorally equivalent for everything the mutex observes:
//!   wake_up_one returns nonzero iff an un-woken entry existed, wake
//!   order is head-first (FIFO), and finish_wait removes the entry if
//!   still present. The kernel's W1 refused-wake / sc100 sticky-token
//!   shapes are scheduler-level and cannot occur in the condvar model.
//! - `schedule()` -> blocking on the park condvar; `get_jiffies()` ->
//!   a model clock that advances by 1 on every read (time passes
//!   arbitrarily between scheduling points).
//! - The adaptive 4-iteration spin in `lock()` collapses to ONE fair
//!   try (the spin is a performance retry of the identical predicate —
//!   no protocol state involved).
//! - The R85 anti-starve self-wake timer is NOT modeled: it is a
//!   liveness backstop, and omitting a wake source only ADDS deadlocks
//!   loom can find (over-approximation). A deadlock reported here must
//!   be checked against "would the 5 s timer rescue it?" before being
//!   called a kernel bug; planted scenario P1 is exactly the pre-R85
//!   protocol where no rescue existed.
//!
//! # Checked properties
//!
//! - INV-M1..M4 after every state critical section (mutex_model.rs).
//! - INV-M5 token conservation: `state.waiters == #tasks whose waiter
//!   loop holds counted=true`, asserted inside every state section.
//! - Mutual exclusion at the scenario level (a shared in-section
//!   counter asserted to be exactly 1 between lock and unlock).
//! - Deadlock freedom: loom fails the run if every thread blocks with
//!   no wake possible (the planted scenarios pin this detection).
//!
//! # When the pre-R85 strand is reachable (found while writing P1)
//!
//! With only fair sleepable callers, the R84 gate makes a dropped
//! handoff unreachable-as-a-strand: a counted task is queued whenever
//! it is parked, so release()'s wake finds it; between finish_wait and
//! the next prepare_to_wait (the mid-loop window) a release wakes
//! nobody, but a lone mid-loop waiter is recovered by the `waiters == 1`
//! term of the re-check. The strand needs TWO counted mid-loop waiters
//! at once, and mid-loop waiters only exist if their token spend FAILED
//! — which requires an UNGATED barger (the early-boot/IRQ-context
//! spinner path) to re-take the free lock between the wake and the
//! spend. Scenario P1 therefore includes a barging ungated spinner,
//! mirroring the real system where that context class exists.

#![cfg(loom)]

use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::sync::{Arc, Condvar, Mutex};
use std::collections::VecDeque;

use super::mutex_model::{planted, MutexState};

/// Scenario knobs. `proto` selects the transition set; the faithful
/// set delegates to the shared MutexState (single line per method, so
/// drift is impossible); planted sets use the mutated copies in
/// mutex_model::planted (each marked with the bug it plants).
pub struct Cfg {
    /// ANTI_STARVE_JIFFIES analog; the clock advances 1/read, so a
    /// small bound makes the override reachable inside a scenario.
    pub anti_starve: u64,
    pub proto: Proto,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Proto {
    /// Current kernel protocol (R84 gate + tokens, R85 epoch recovery).
    Faithful,
    /// P1 (b148 class): pre-R85 — no epoch bump on dropped handoff and
    /// no epoch term in the re-check.
    NoEpochRecovery,
    /// P2: token spend forgets to leave the count (no waiters -= 1).
    TokenSpendSkipsDecrement,
    /// P4: re-park re-counts (the `if !counted` guard dropped).
    ReparkRecounts,
}

/// The modeled mutex. Park slots are pre-allocated; task ids are
/// 0..tasks (loom has no stable thread-identity query across
/// exploration, so the harness passes the id explicitly).
pub struct LoomMutex {
    state: Mutex<MutexState>,
    queue: Mutex<VecDeque<u32>>,
    park: Vec<(Mutex<bool>, Condvar)>,
    counted: Vec<AtomicBool>,
    clock: Mutex<u64>,
    cfg: Cfg,
}

impl LoomMutex {
    pub fn new(tasks: usize, cfg: Cfg) -> Self {
        Self {
            state: Mutex::new(MutexState::new()),
            queue: Mutex::new(VecDeque::new()),
            park: (0..tasks).map(|_| (Mutex::new(false), Condvar::new())).collect(),
            counted: (0..tasks).map(|_| AtomicBool::new(false)).collect(),
            clock: Mutex::new(0),
            cfg,
        }
    }

    fn now(&self) -> u64 {
        let mut c = self.clock.lock().unwrap();
        *c = c.wrapping_add(1);
        *c
    }

    fn counted_total(&self) -> u32 {
        self.counted.iter().filter(|f| f.load(Ordering::SeqCst)).count() as u32
    }

    /// Every state critical section goes through here: run the
    /// transition (the closure MUST mirror any counted-flag change into
    /// `self.counted` before returning), then assert INV-M1..M4 and
    /// INV-M5 conservation. Safe to read all flags here: the protocol
    /// only mutates a task's counted flag inside its own state section.
    fn with_state<T>(&self, f: impl FnOnce(&mut MutexState) -> T) -> T {
        let mut st = self.state.lock().unwrap();
        let out = f(&mut st);
        if let Some(inv) = st.invariant_violation() {
            panic!(
                "{} (locked={} owner={} depth={} waiters={})",
                inv, st.locked, st.owner, st.depth, st.waiters
            );
        }
        let live = self.counted_total();
        assert_eq!(
            st.waiters, live,
            "INV-M5 token conservation: waiters={} but counted tasks={}",
            st.waiters, live
        );
        out
    }

    /// Wait-queue wake_up_one: FIFO head, one wake. Returns woke count.
    fn wake_up_one(&self) -> usize {
        let pid = {
            let mut q = self.queue.lock().unwrap();
            match q.pop_front() {
                Some(p) => p,
                None => return 0,
            }
        };
        let slot = &self.park[pid as usize];
        let mut flag = slot.0.lock().unwrap();
        *flag = true;
        drop(flag);
        slot.1.notify_all();
        1
    }

    /// finish_wait: remove our entry if still queued.
    fn finish_wait(&self, pid: u32) {
        self.queue.lock().unwrap().retain(|&p| p != pid);
    }

    /// The slow-path acquire, mirroring SleepMutex::lock()'s waiter
    /// loop (adaptive spin elided to one fair try — see module doc).
    /// Returns true when the lock is HELD (fresh or recursive).
    pub fn lock_held(&self, pid: u32) -> bool {
        {
            let now = self.now();
            if self.with_state(|st| st.try_acquire_or_recurse_fair(now, pid)) {
                return true;
            }
        }
        let mut counted = false;
        let mut woken_retry = false;
        let mut first_park = 0u64;
        loop {
            if woken_retry {
                // Handoff: the free lock is ours if still free.
                let now = self.now();
                let pid2 = pid;
                let got = self.with_state(|st| {
                    let got = match self.cfg.proto {
                        Proto::TokenSpendSkipsDecrement => {
                            planted::try_acquire_wake_token_no_decrement(st, now, pid2)
                        }
                        _ => st.try_acquire_wake_token(now, pid2),
                    };
                    if got {
                        counted = false;
                        self.counted[pid2 as usize].store(false, Ordering::SeqCst);
                    }
                    got
                });
                if got {
                    return true;
                }
                woken_retry = false;
            }
            {
                let now = self.now();
                if self.with_state(|st| st.try_acquire_or_recurse_fair(now, pid)) {
                    debug_assert!(!counted);
                    return true;
                }
            }
            // prepare_to_wait: register on the queue (FIFO tail).
            self.queue.lock().unwrap().push_back(pid);
            // Snapshot the orphaned-handoff epoch BEFORE the
            // registration critical section.
            let my_epoch = self.state.lock().unwrap().handoff_epoch;
            // Registration + fairness-aware re-check (R7-B6b / R84/R85).
            let now = self.now();
            let pid2 = pid;
            let take = self.with_state(|st| {
                let take = match self.cfg.proto {
                    Proto::Faithful | Proto::TokenSpendSkipsDecrement => st
                        .register_and_recheck(
                            now,
                            pid2,
                            &mut counted,
                            my_epoch,
                            &mut first_park,
                            self.cfg.anti_starve,
                        ),
                    Proto::NoEpochRecovery => planted::register_and_recheck_no_epoch(
                        st,
                        now,
                        pid2,
                        &mut counted,
                        my_epoch,
                        &mut first_park,
                        self.cfg.anti_starve,
                    ),
                    Proto::ReparkRecounts => planted::register_and_recheck_recount(
                        st,
                        now,
                        pid2,
                        &mut counted,
                        my_epoch,
                        &mut first_park,
                        self.cfg.anti_starve,
                    ),
                };
                self.counted[pid2 as usize].store(counted, Ordering::SeqCst);
                take
            });
            if take {
                self.finish_wait(pid);
                return true;
            }
            // schedule(): park until wake_up_one flags us.
            {
                let slot = &self.park[pid as usize];
                let mut flag = slot.0.lock().unwrap();
                while !*flag {
                    slot.1.wait(&mut flag).unwrap();
                }
                *flag = false;
            }
            self.finish_wait(pid);
            woken_retry = true;
        }
    }

    /// The UNGATED acquire (lock()'s no-task / preempt-count degraded
    /// spinner): a free lock goes to whoever asks, gate ignored.
    pub fn ungated_try(&self, pid: u32) -> bool {
        let now = self.now();
        self.with_state(|st| st.try_acquire_or_recurse(now, pid))
    }

    /// release(): outermost drop frees, then hands one token down; a
    /// dropped handoff (empty queue) bumps the epoch.
    pub fn unlock(&self, pid: u32) {
        self.with_state(|st| st.release_outermost());
        let woke = self.wake_up_one();
        if woke == 0 {
            match self.cfg.proto {
                Proto::NoEpochRecovery => { /* P1: pre-R85 release skipped the bump */ }
                _ => self.with_state(|st| st.note_dropped_handoff()),
            }
        }
    }

    /// do_exit()'s release_if_owner + one handoff token. Note: a task
    /// killed while PARKED in the waiter loop leaks its count unit by
    /// design (bounded-latency insurance — anti-starve recovers); this
    /// driver cannot express killing a parked thread, so scenarios use
    /// exit_release only from holding context.
    pub fn exit_release(&self, pid: u32) {
        let was_held = self.with_state(|st| st.release_if_owner(pid));
        if was_held {
            let _ = self.wake_up_one();
        }
    }
}

// ==================== Scenarios (current protocol: must be clean) ====================

/// A worker: `rounds` lock/unlock cycles with a mutual-exclusion
/// assertion inside the critical section.
fn worker_body(m: &LoomMutex, crit: &AtomicUsize, pid: u32, rounds: usize) {
    for _ in 0..rounds {
        assert!(m.lock_held(pid), "lock() must return held");
        {
            let n = crit.fetch_add(1, Ordering::SeqCst) + 1;
            assert_eq!(n, 1, "mutual exclusion violated: {} tasks in section", n);
            let n = crit.fetch_sub(1, Ordering::SeqCst) - 1;
            assert_eq!(n, 0, "mutual exclusion violated: {} tasks in section", n);
        }
        m.unlock(pid);
    }
}

fn spawn_workers(m: &Arc<LoomMutex>, crit: &Arc<AtomicUsize>, ids: &[u32], rounds: usize) {
    let mut joins = Vec::new();
    for &pid in ids {
        let m = m.clone();
        let crit = crit.clone();
        joins.push(
            loom::thread::Builder::new()
                .spawn(move || worker_body(&m, &crit, pid, rounds))
                .unwrap(),
        );
    }
    for j in joins {
        j.join().unwrap();
    }
}

/// S1: two tasks ping-pong the lock through the full slow path.
#[test]
fn loom_mutex_two_tasks_clean() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(2, Cfg { anti_starve: u64::MAX, proto: Proto::Faithful }));
        let crit = Arc::new(AtomicUsize::new(0));
        spawn_workers(&m, &crit, &[0, 1], 2);
    });
}

/// S2: three contending tasks.
#[test]
fn loom_mutex_three_tasks_clean() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(3, Cfg { anti_starve: u64::MAX, proto: Proto::Faithful }));
        let crit = Arc::new(AtomicUsize::new(0));
        spawn_workers(&m, &crit, &[0, 1, 2], 2);
    });
}

/// S3: sleepers plus an UNGATED spinner (the no-task degradation path
/// in lock()): the spinner may barge past the gate by design; the
/// invariants, conservation, and mutual exclusion must still hold.
#[test]
fn loom_mutex_spinner_plus_sleepers_clean() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(3, Cfg { anti_starve: u64::MAX, proto: Proto::Faithful }));
        let crit = Arc::new(AtomicUsize::new(0));
        let ms = m.clone();
        let spinner = loom::thread::Builder::new()
            .spawn(move || {
                // bounded, like the kernel's degrade-to-spin loop
                for _ in 0..4 {
                    if ms.ungated_try(2) {
                        ms.unlock(2);
                        return;
                    }
                    loom::thread::yield_now();
                }
            })
            .unwrap();
        spawn_workers(&m, &crit, &[0, 1], 2);
        spinner.join().unwrap();
    });
}

/// S4: nested acquire (recursion) plus the do_exit force-release: a
/// task holding depth 2 "exits"; the other must still acquire.
#[test]
fn loom_mutex_recursion_and_exit_release_clean() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(2, Cfg { anti_starve: 8, proto: Proto::Faithful }));
        let ma = m.clone();
        let a = loom::thread::Builder::new()
            .spawn(move || {
                assert!(ma.lock_held(0));
                assert!(ma.lock_held(0)); // recursive
                ma.exit_release(0); // do_exit inside the nested section
            })
            .unwrap();
        let mb = m.clone();
        let crit = Arc::new(AtomicUsize::new(0));
        let b = loom::thread::Builder::new()
            .spawn(move || worker_body(&mb, &crit, 1, 1))
            .unwrap();
        a.join().unwrap();
        b.join().unwrap();
    });
}

/// S5: anti-starve override exercised with a small bound and a barging
/// ungated spinner: a waiter that loses its token to the barger must
/// still take the free lock once past the bound.
#[test]
fn loom_mutex_anti_starve_override_clean() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(2, Cfg { anti_starve: 2, proto: Proto::Faithful }));
        let crit = Arc::new(AtomicUsize::new(0));
        let ms = m.clone();
        let barger = loom::thread::Builder::new()
            .spawn(move || {
                for _ in 0..3 {
                    if ms.ungated_try(1) {
                        ms.unlock(1);
                    }
                    loom::thread::yield_now();
                }
            })
            .unwrap();
        spawn_workers(&m, &crit, &[0], 1);
        barger.join().unwrap();
    });
}

// ==================== Planted bugs (detection demos) ====================

/// P1 (the b148 pre-R85 class): with no epoch recovery, a dropped
/// handoff strands the free lock behind the fairness gate and parked
/// waiters have no waker (the anti-starve timer is not modeled —
/// pre-R85 had neither). The barging ungated spinner creates the two
/// mid-loop waiters the strand needs (see the module doc's reachability
/// note). loom must report the deadlock; this test PASSES when the
/// harness catches it.
#[test]
#[should_panic]
fn loom_mutex_planted_no_epoch_recovery_is_caught() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(3, Cfg { anti_starve: u64::MAX, proto: Proto::NoEpochRecovery }));
        let crit = Arc::new(AtomicUsize::new(0));
        let ms = m.clone();
        let barger = loom::thread::Builder::new()
            .spawn(move || {
                for _ in 0..4 {
                    if ms.ungated_try(2) {
                        ms.unlock(2);
                    }
                    loom::thread::yield_now();
                }
            })
            .unwrap();
        spawn_workers(&m, &crit, &[0, 1], 2);
        barger.join().unwrap();
    });
}

/// P2: a token spend that forgets `waiters -= 1` breaks INV-M5 in the
/// very section that spends the token.
#[test]
#[should_panic(expected = "INV-M5")]
fn loom_mutex_planted_token_skip_decrement_is_caught() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(2, Cfg { anti_starve: u64::MAX, proto: Proto::TokenSpendSkipsDecrement }));
        let crit = Arc::new(AtomicUsize::new(0));
        spawn_workers(&m, &crit, &[0, 1], 2);
    });
}

/// P4: re-park re-counting (the `if !counted` guard dropped) inflates
/// `waiters` against the task view — INV-M5 fires.
#[test]
#[should_panic(expected = "INV-M5")]
fn loom_mutex_planted_repark_recount_is_caught() {
    loom::model(|| {
        let m = Arc::new(LoomMutex::new(2, Cfg { anti_starve: u64::MAX, proto: Proto::ReparkRecounts }));
        let crit = Arc::new(AtomicUsize::new(0));
        spawn_workers(&m, &crit, &[0, 1], 2);
    });
}
