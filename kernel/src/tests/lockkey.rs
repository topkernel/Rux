
// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]
// Typed lock-key shape tests (rust-design-headroom §3, the GRQ typed
// lock-key slice). The mechanics: permitted nestings are trait impls
// (CanNestInside), so the reverse or same-level acquisition cannot
// compile — the negative half is a compile-time fact and cannot be run
// as a test; the positive half (the allowed nesting locks and unlocks
// correctly, guards manage the raw protocol, the claim/unlock pair
// used by the GRQ pattern behaves) is exercised here against REAL
// keyed locks, and the GRQ pilot itself rides the whole suite (every
// schedule/pick/wake path takes the keyed lock).
use crate::sync::lockkey::{
    FutexBucketKey, GrqKey, KeyedRawSpinlock, KeyedSpinlock, NestKey, NestProof, NetQueueKey,
    NetStatsKey, WaiterSlotKey,
};
use super::{test_fail, test_group_start, test_pass};

pub fn test_lockkey() {
    test_group_start("lockkey");

    // Keys carry their invariant names (deadlock-watchdog cross-check).
    if GrqKey::NAME == "grq"
        && FutexBucketKey::NAME == "futex-bucket"
        && NetStatsKey::NAME == "net-stats"
        && NetQueueKey::NAME == "net-queue"
    {
        test_pass("key names");
    } else {
        test_fail("key names", "a key NAME drifted");
    }

    // Top-level acquire + guard drop unlocks: a fresh lock acquires
    // after the previous guard dropped.
    let grq: KeyedRawSpinlock<GrqKey> = KeyedRawSpinlock::new();
    {
        let _g = grq.lock();
        // Re-claim while held must fail (the lock is held).
        if grq.try_lock().is_some() {
            test_fail("guard holds", "try_lock succeeded while guard live");
            return;
        }
    }
    if grq.try_lock().is_none() {
        test_fail("guard drop", "lock not free after guard drop");
        return;
    } else {
        drop(grq.try_lock());
        test_pass("guard holds/drops");
    }

    // The compile-checked nesting (INV-LOCK-5 forward): grq acquired
    // while the bucket guard is live, via lock_nested — the call only
    // compiles because `impl CanNestInside<FutexBucketKey> for GrqKey`
    // exists. Inner must unlock before outer (the inner guard's drop
    // while the outer is still held exercises nothing special, but the
    // ORDER here is the invariant's runtime half).
    let bucket: KeyedRawSpinlock<FutexBucketKey> = KeyedRawSpinlock::new();
    {
        let bg = bucket.lock();
        let gg = grq.lock_nested(&bg.proof()); // INV-LOCK-5 forward
        drop(gg);
        drop(bg);
    }
    // The DATA-CARRYING keyed lock (the HASH_HEADS shape) mints the
    // same proof class: the mini end-to-end mechanics of the real
    // wake chain (bucket guard -> proof -> nested GRQ acquire), plus
    // the guard's Deref/DerefMut to the data.
    let heads: KeyedSpinlock<FutexBucketKey, Option<usize>> = KeyedSpinlock::new(None);
    {
        let mut head = heads.lock_irqsave();
        assert!(head.is_none(), "fresh bucket head must read None");
        *head = Some(7);
        let gg = grq.lock_nested(&head.proof()); // same compile check
        drop(gg);
        assert_eq!(*head, Some(7), "bucket data corrupted across nested section");
    }
    // The data persists (a lock guard does not restore data); the lock
    // being free again is proven by this acquire succeeding.
    match *heads.lock_irqsave() {
        Some(7) => test_pass("data guard proof chain"),
        v => {
            test_fail("data guard proof chain", &alloc::format!("head value lost: {:?}", v));
            return;
        }
    }
    // Same lock classes must be free again.
    if bucket.try_lock().is_none() || grq.try_lock().is_none() {
        test_fail("nested drop", "a keyed lock stayed held after nested section");
        return;
    }
    test_pass("nested INV-LOCK-5 forward");

    // D-11 slice 3: a waiter SLOT inside the bucket (the shape of
    // every chain walk) — the data-guard lock_nested twin.
    let slots: KeyedSpinlock<WaiterSlotKey, Option<usize>> = KeyedSpinlock::new(None);
    {
        let h = heads.lock_irqsave();
        let s = slots.lock_nested(&h.proof()); // slot inside bucket
        drop(s);
        drop(h);
    }
    test_pass("nested slot-in-bucket");

    // INV-LOCK-6 forward: stats inside a net-queue lock.
    let nq: KeyedRawSpinlock<NetQueueKey> = KeyedRawSpinlock::new();
    let stats: KeyedRawSpinlock<NetStatsKey> = KeyedRawSpinlock::new();
    {
        let qg = nq.lock();
        let sg = stats.lock_nested(&qg.proof());
        drop(sg);
        drop(qg);
    }
    test_pass("nested INV-LOCK-6 forward");

    // The GRQ pattern's claim/unlock pair (guard-less claim; the
    // subsystem's own guard wraps the irqsave/preempt protocol).
    let grq2: KeyedRawSpinlock<GrqKey> = KeyedRawSpinlock::new();
    if grq2.try_lock_claim() {
        grq2.unlock();
        if grq2.try_lock_claim() {
            grq2.unlock();
            test_pass("claim/unlock pair");
        } else {
            test_fail("claim/unlock pair", "unlock did not free the lock");
        }
    } else {
        test_fail("claim/unlock pair", "fresh lock claim failed");
    }

    // Documentation of the negative half (compile-time, so asserted
    // here only as a comment the shape group pins): the following MUST
    // NOT compile —
    //   let _p: NestProof<'_, FutexBucketKey> = unsafe { core::mem::zeroed() };
    //       (no public constructor — only a live guard mints proofs)
    //   let gg = grq.lock();
    //   let bg = bucket.lock_nested(&gg);   // no impl CanNestInside<GrqKey>
    //                                       // for FutexBucketKey (INV-LOCK-5
    //                                       // reverse: the deadlock order)
    //   let gg2 = grq.lock_nested(&gg);     // no same-class impl either
    // The verify crate (host cargo test) can grow a trybuild compile-fail
    // leg for these two snippets when one is wanted; the trait surface
    // is the enforcement either way.

}
