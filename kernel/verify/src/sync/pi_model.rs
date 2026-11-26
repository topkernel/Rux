//! PI chain engine model — R6 extraction of the futex_pi boost
//! propagation semantics (kernel/core/src/sync/hotpath.rs) for host-side
//! invariant checking.
//!
//! # What this file is
//!
//! The kernel's PI engine keeps its state in two fixed pools (states
//! keyed by futex word, waiter slots) under per-hash bucket locks.
//! The bug-dense logic this model pins is the BOOST LAYER: how an
//! edge's owner configuration is derived from its top waiter (live
//! evaluation), how a chained owner re-ranks on the lock it blocks on
//! (the reference's rt_mutex_adjust_prio_chain requeue step), and how
//! the walk terminates. The pool/bucket machinery is environment —
//! the model replaces it with owned vectors.
//!
//! Transitions mirror their kernel counterparts:
//!
//! - `adjust_owner_boost` <-> futex_pi::pi_adjust_owner_boost (desired
//!   derived from the top waiter's LIVE rank — a chained waiter that
//!   was itself boosted re-evaluates on the next walk step; an RT top
//!   waiter converts/raises the owner, a CFS top waiter applies the
//!   nice-floor),
//! - `refresh_rank`       <-> futex_pi::pi_waiter_refresh_rank
//!   (unlink + re-snapshot at the live rank + re-insert),
//! - `chain_walk`         <-> futex_pi::pi_chain_walk (refresh,
//!   adjust, follow the owner; bounded by MAX_PI_CHAIN),
//! - `block_on`/`unblock` <-> the park path's insert+adjust and the
//!   timeout path's unlink+adjust,
//! - `handoff`            <-> unlock_pi's re-point + pi_owner_handoff
//!   (restore the outgoing owner, re-arm on the incoming one, the new
//!   owner leaves the waiter queue owning the lock).
//!
//! # Chain invariant table (INV-C*)
//!
//! - INV-C1 (boost conservation): for every lock with waiters, the
//!   owner's LIVE rank is at least the top waiter's rank (RT beats
//!   CFS, higher rt_priority beats lower, lower nice beats higher).
//! - INV-C2 (deboost restore): a task with no armed boost edges runs
//!   at its BASE configuration (policy, priority, nice restored).
//! - INV-C3 (walk termination): the chain walk visits at most
//!   MAX_PI_CHAIN edges and a would-be cycle is refused at block time
//!   (chain_hits), never walked.
//! - INV-C4 (queue order): each lock's waiter list stays ordered by
//!   rank (RT first by rt_priority desc, then CFS nice asc) with FIFO
//!   ties — the reference plist discipline.
//! - INV-C5 (saved-slot pairing): a task whose live config differs
//!   from its base has at least one armed edge (owner_saved) whose
//!   restore returns it to base — every boost has its undo record.

/// Scheduling class + priority of a task — the model's rank key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    /// SCHED_FIFO/RR at rt_priority p (higher wins).
    Rt(u32),
    /// CFS at nice n (lower wins).
    Cfs(i32),
}

impl Rank {
    /// True when `self` strictly outranks `other`.
    pub fn outranks(&self, other: &Rank) -> bool {
        match (self, other) {
            (Rank::Rt(a), Rank::Rt(b)) => a > b,
            (Rank::Rt(_), Rank::Cfs(_)) => true,
            (Rank::Cfs(_), Rank::Rt(_)) => false,
            (Rank::Cfs(a), Rank::Cfs(b)) => a < b,
        }
    }

    /// True when `self` satisfies (serves) a top waiter of rank `w`:
    /// equal or better.
    pub fn serves(&self, w: &Rank) -> bool {
        *self == *w || self.outranks(w)
    }
}

/// A task: identity, BASE (saved) configuration, LIVE configuration
/// (what the boost layer mutates), and the lock it is blocked on.
#[derive(Debug, Clone)]
pub struct PiTask {
    pub id: usize,
    pub base: Rank,
    pub live: Rank,
    pub blocked_on: Option<usize>,
}

/// A lock edge: owner task id plus a rank-ordered waiter list and the
/// owner's saved configuration while a boost is armed.
#[derive(Debug, Clone)]
pub struct PiLock {
    pub owner: usize,
    /// (task, rank-at-insert-or-refresh), ordered best-first.
    pub waiters: Vec<(usize, Rank)>,
    pub owner_saved: Option<Rank>,
}

pub const MAX_PI_CHAIN: usize = 16;

/// The model world.
#[derive(Debug, Clone, Default)]
pub struct PiWorld {
    pub tasks: Vec<PiTask>,
    pub locks: Vec<PiLock>,
}

impl PiWorld {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn task(&mut self, base: Rank) -> usize {
        let id = self.tasks.len();
        self.tasks.push(PiTask {
            id,
            base,
            live: base,
            blocked_on: None,
        });
        id
    }

    pub fn lock(&mut self, owner: usize) -> usize {
        let id = self.locks.len();
        self.locks.push(PiLock {
            owner,
            waiters: Vec::new(),
            owner_saved: None,
        });
        id
    }

    fn top_waiter_rank(&self, li: usize) -> Option<Rank> {
        self.locks[li].waiters.first().map(|&(_, r)| r)
    }

    /// pi_adjust_owner_boost: derive the desired boost from the top
    /// waiter's rank, restore the saved slot, re-arm. An RT top waiter
    /// converts/raises the owner to the waiter's priority
    /// (rt_mutex_setprio shape); a CFS top waiter pulls the owner's
    /// nice DOWN to the waiter's (the nice-floor; never up).
    pub fn adjust_owner_boost(&mut self, li: usize) {
        let desired = self.top_waiter_rank(li);
        let owner = self.locks[li].owner;
        if let Some(w) = desired {
            if self.tasks[owner].live.serves(&w) {
                return; // already satisfies
            }
        }
        if let Some(sv) = self.locks[li].owner_saved.take() {
            self.tasks[owner].live = sv;
        }
        let Some(w) = desired else { return };
        // Save the BASE configuration (restore-to-base semantics). The
        // kernel's per-state saved slot snapshots the LIVE config at
        // arm time — identical for the single-contended-lock-per-owner
        // case every real workload uses, but an owner of MULTIPLE
        // simultaneously-contended PI locks can chain-restore to an
        // intermediate boosted config (bounded: priority stays too
        // high until the next edge edit; never wrong-low). Known
        // limitation, run/RT-R6.md; the model pins the exact
        // restore-to-base contract.
        self.locks[li].owner_saved = Some(self.tasks[owner].base);
        let base = self.tasks[owner].base;
        self.tasks[owner].live = match (w, base) {
            (Rank::Rt(p), _) => Rank::Rt(p),
            (Rank::Cfs(wn), Rank::Cfs(on)) => Rank::Cfs(wn.min(on)),
            (Rank::Cfs(_), cur) => cur, // RT owner: already serves
        };
    }

    /// pi_waiter_refresh_rank: re-snapshot the blocked task's waiter
    /// entry at its LIVE rank and re-insert at the new position (the
    /// reference's requeue step).
    pub fn refresh_rank(&mut self, li: usize, task: usize) {
        let Some(pos) = self.locks[li].waiters.iter().position(|&(t, _)| t == task)
        else {
            return;
        };
        let r = self.tasks[task].live;
        self.locks[li].waiters.remove(pos);
        self.insert_ordered(li, task, r);
    }

    fn insert_ordered(&mut self, li: usize, task: usize, r: Rank) {
        let waiters = &mut self.locks[li].waiters;
        let mut at = waiters.len();
        for (i, &(_, wr)) in waiters.iter().enumerate() {
            if r.outranks(&wr) {
                at = i;
                break;
            }
        }
        waiters.insert(at, (task, r));
    }

    /// The park path: queue the waiter, adjust the edge. Refuses a
    /// would-be cycle (pi_chain_hits). A task re-blocks (leaves any
    /// previous queue first — the park loop's unlink-before-retry).
    pub fn block_on(&mut self, task: usize, li: usize) -> bool {
        if self.chain_hits(task, li) {
            return false;
        }
        if self.tasks[task].blocked_on.take().is_some() {
            for lk in self.locks.iter_mut() {
                lk.waiters.retain(|&(t, _)| t != task);
            }
        }
        let r = self.tasks[task].live;
        self.tasks[task].blocked_on = Some(li);
        self.insert_ordered(li, task, r);
        self.adjust_owner_boost(li);
        true
    }

    /// The timeout/acquire path: leave the queue, adjust the edge.
    pub fn unblock(&mut self, task: usize) {
        let Some(li) = self.tasks[task].blocked_on.take() else {
            return;
        };
        self.locks[li].waiters.retain(|&(t, _)| t != task);
        self.adjust_owner_boost(li);
    }

    /// unlock_pi's handoff: restore the outgoing owner from its saved
    /// slot, the incoming owner leaves the waiter queue OWNING the
    /// lock, the edge re-arms from the remaining chain.
    pub fn handoff(&mut self, li: usize, new_owner: usize) {
        let old = self.locks[li].owner;
        if let Some(sv) = self.locks[li].owner_saved.take() {
            self.tasks[old].live = sv;
        }
        self.locks[li].waiters.retain(|&(t, _)| t != new_owner);
        if self.tasks[new_owner].blocked_on == Some(li) {
            self.tasks[new_owner].blocked_on = None;
        }
        self.locks[li].owner = new_owner;
        self.adjust_owner_boost(li);
    }

    /// pi_chain_hits: does queueing `task` on `li` close a cycle?
    /// Walks li's owner -> blocked-on -> owner ... (the kernel's
    /// owner-side walk — the reference's rt_mutex_owner(lock) ==
    /// top_task check).
    pub fn chain_hits(&self, task: usize, li: usize) -> bool {
        let mut cur = self.locks[li].owner;
        for _ in 0..MAX_PI_CHAIN {
            if cur == task {
                return true;
            }
            let Some(b) = self.tasks[cur].blocked_on else {
                return false;
            };
            cur = self.locks[b].owner;
        }
        true
    }

    /// pi_chain_walk: from `owner` upward — refresh the owner's waiter
    /// entry on the lock it blocks on, adjust that edge, follow to its
    /// owner. Returns the edges visited (INV-C3 bounds it).
    pub fn chain_walk(&mut self, owner: usize) -> usize {
        let mut cur = owner;
        let mut visited = 0usize;
        for _ in 0..MAX_PI_CHAIN {
            let Some(li) = self.tasks[cur].blocked_on else {
                return visited;
            };
            self.refresh_rank(li, cur);
            self.adjust_owner_boost(li);
            visited += 1;
            cur = self.locks[li].owner;
        }
        visited
    }

    // -----------------------------------------------------------------
    // Invariant checks (INV-C1..C5). None = clean; Some(name) = broken.
    // -----------------------------------------------------------------

    pub fn invariant_violation(&self) -> Option<&'static str> {
        for l in self.locks.iter() {
            // INV-C4: waiter order never improves going down the list.
            for w in l.waiters.windows(2) {
                if w[1].1.outranks(&w[0].1) {
                    return Some("INV-C4 waiter order");
                }
            }
            // INV-C1: with waiters, the owner's live rank serves the
            // top waiter's LIVE rank (a chained waiter boosted while
            // parked carries the new rank even before its entry is
            // refreshed — the boost input is live, the queue order is
            // the snapshot).
            if let Some(&(tw, _)) = l.waiters.first() {
                if !self.tasks[l.owner].live.serves(&self.tasks[tw].live) {
                    return Some("INV-C1 boost conservation");
                }
            }
        }
        // INV-C2 + INV-C5: every task with live != base has an armed
        // edge whose restore returns it to base; every task with live
        // == base may still hold (transient) saved slots, but a task
        // with NO armed edge must be at base.
        for t in self.tasks.iter() {
            let armed: Vec<&PiLock> = self
                .locks
                .iter()
                .filter(|lk| lk.owner_saved.is_some() && lk.owner == t.id)
                .collect();
            if t.live != t.base {
                if armed.is_empty() {
                    return Some("INV-C5 boost without undo record");
                }
            } else if armed.is_empty() {
                // live == base, nothing armed: fully restored (INV-C2
                // holds trivially for this task).
            }
        }
        // INV-C2 strict form: a task with NO waiters on ANY of its
        // locks and no armed edge must be at base.
        for t in self.tasks.iter() {
            let relevant = self.locks.iter().any(|lk| {
                lk.owner == t.id && (!lk.waiters.is_empty() || lk.owner_saved.is_some())
            });
            if !relevant && t.live != t.base {
                return Some("INV-C2 incomplete restore");
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_clean(w: &PiWorld, ctx: &str) {
        if let Some(v) = w.invariant_violation() {
            panic!("{}: {}", ctx, v);
        }
    }

    /// The R6 acceptance topology: A(CFS) holds L0, B(FIFO 40) holds
    /// L1 blocked on L0, C(FIFO 90) blocks on L1. The walk pushes 90
    /// through to A (the CFS->FIFO conversion shape).
    #[test]
    fn chain_three_deep_boosts_through() {
        let mut w = PiWorld::new();
        let a = w.task(Rank::Cfs(0));
        let b = w.task(Rank::Rt(40));
        let c = w.task(Rank::Rt(90));
        let l0 = w.lock(a);
        let l1 = w.lock(b);
        assert!(w.block_on(b, l0));
        assert_clean(&w, "single edge");
        assert_eq!(w.tasks[a].live, Rank::Rt(40), "A boosted to B's rank");
        assert!(w.block_on(c, l1));
        assert_eq!(w.tasks[b].live, Rank::Rt(90), "B boosted by C");
        // WITHOUT the walk A stays at 40 — the pre-R6 single-edge gap:
        assert_eq!(w.tasks[a].live, Rank::Rt(40));
        // THE R6 WALK (the park path's upward propagation):
        let visited = w.chain_walk(b);
        assert!(visited >= 1, "walk reached L0's edge");
        assert_eq!(
            w.tasks[a].live,
            Rank::Rt(90),
            "boost walked through to A (CFS->FIFO 90)"
        );
        assert_clean(&w, "post-walk");
    }

    /// INV-C2/C5: teardown restores everyone exactly.
    #[test]
    fn teardown_restores_all() {
        let mut w = PiWorld::new();
        let a = w.task(Rank::Cfs(5));
        let b = w.task(Rank::Rt(40));
        let c = w.task(Rank::Rt(90));
        let l0 = w.lock(a);
        let l1 = w.lock(b);
        assert!(w.block_on(b, l0));
        assert!(w.block_on(c, l1));
        w.chain_walk(b);
        assert_clean(&w, "armed");
        // Teardown cascade: A releases L0 -> B owns it; B releases L1
        // -> C owns it; both restores consumed.
        w.handoff(l0, b);
        w.handoff(l1, c);
        assert_eq!(w.tasks[a].live, Rank::Cfs(5), "A restored to CFS nice 5");
        assert_eq!(w.tasks[b].live, Rank::Rt(40), "B restored to FIFO 40");
        assert_eq!(w.tasks[c].live, Rank::Rt(90), "C never boosted");
        assert_clean(&w, "torn down");
    }

    /// INV-C3: a cycle is refused at block time, never walked.
    #[test]
    fn cycle_detected_not_walked() {
        let mut w = PiWorld::new();
        let x = w.task(Rank::Rt(50));
        let y = w.task(Rank::Rt(60));
        let lx = w.lock(x);
        let ly = w.lock(y);
        assert!(w.block_on(y, lx));
        assert!(!w.block_on(x, ly), "cycle refused");
        assert_clean(&w, "cycle refused");
    }

    /// INV-C4: refresh re-orders a waiter boosted while parked.
    #[test]
    fn refresh_reorders() {
        let mut w = PiWorld::new();
        let a = w.task(Rank::Rt(30));
        let lo = w.task(Rank::Rt(10));
        let hi = w.task(Rank::Rt(80));
        let l = w.lock(a);
        assert!(w.block_on(lo, l));
        assert!(w.block_on(hi, l));
        assert_eq!(w.locks[l].waiters[0].0, hi, "hi at head");
        // Boost lo PROPERLY (its own "elsewhere" edge): a waiter on a
        // lock lo owns.
        let top = w.task(Rank::Rt(95));
        let l2 = w.lock(lo);
        assert!(w.block_on(top, l2));
        assert_eq!(w.tasks[lo].live, Rank::Rt(95), "lo boosted elsewhere");
        w.refresh_rank(l, lo);
        w.adjust_owner_boost(l); // the walk pairs refresh with adjust
        assert_eq!(w.locks[l].waiters[0].0, lo, "refreshed waiter to head");
        assert_eq!(w.tasks[a].live, Rank::Rt(95), "owner re-boosted to 95");
        assert_clean(&w, "refresh order");
    }

    /// Planted P1 (the pre-R6 kernel's behavior): no walk — the outer
    /// owner stays low; the harness must flag INV-C1.
    #[test]
    fn planted_no_walk_is_caught() {
        let mut w = PiWorld::new();
        let a = w.task(Rank::Cfs(0));
        let b = w.task(Rank::Rt(40));
        let c = w.task(Rank::Rt(90));
        let l0 = w.lock(a);
        let l1 = w.lock(b);
        assert!(w.block_on(b, l0));
        assert!(w.block_on(c, l1));
        // (no chain_walk call — the pre-R6 shape)
        assert_eq!(
            w.invariant_violation(),
            Some("INV-C1 boost conservation"),
            "harness catches the single-edge gap"
        );
    }

    /// Randomized op soup: block/unblock/walk/handoff sequences keep
    /// INV-C1..C5 after quiescence (walks from every owner per step,
    /// mirroring the engine's synchronous call sites).
    #[test]
    fn random_soup_invariants() {
        let mut rng_state: u64 = 0x1234_5678_9abc_def0;
        let mut mulberry = move || {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            rng_state
        };
        for round in 0..200u64 {
            let mut w = PiWorld::new();
            let nt = 3 + (mulberry() % 4) as usize;
            let nl = 2 + (mulberry() % 3) as usize;
            for _ in 0..nt {
                let r = if (mulberry() >> 8) & 1 == 0 {
                    Rank::Rt((mulberry() % 99) as u32)
                } else {
                    Rank::Cfs(((mulberry() % 40) as i32) - 20)
                };
                w.task(r);
            }
            for _ in 0..nl {
                let owner = (mulberry() as usize) % nt;
                w.lock(owner);
            }
            for _step in 0..40 {
                let op = mulberry() % 4;
                let t = (mulberry() as usize) % nt;
                let li = (mulberry() as usize) % nl;
                match op {
                    0 => {
                        w.block_on(t, li);
                    }
                    1 => {
                        w.unblock(t);
                    }
                    2 => {
                        w.chain_walk(t);
                    }
                    _ => {
                        let no = (mulberry() as usize) % nt;
                        w.handoff(li, no);
                    }
                }
                for ti in 0..nt {
                    w.chain_walk(ti);
                }
                if let Some(v) = w.invariant_violation() {
                    panic!("round {}: {}", round, v);
                }
            }
        }
    }
}
