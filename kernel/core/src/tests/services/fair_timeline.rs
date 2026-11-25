//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! grq-10ns: intrusive red-black CFS timeline unit tests (the
//! entity-embedded rbtree that replaced the BTreeMap). Originally
//! kernel/core/src/tests/fair_timeline.rs; moved with the S1b flip. This
//! crate is #![forbid(unsafe_code)], so the old raw-RbNode unsafe
//! introspection (walk/audit/container-of) is gone: the framework's
//! audited-unsafe zone exports SAFE debug_* helpers (sched/unsafe.rs:
//! debug_insert/erase/erase_leftmost/leftmost_key/walk_keys/
//! black_height) and this driver uses only those.
//!
//! Runs against a LOCAL RbTimeline with heap-allocated SchedEntity
//! arrays (entities live in the linear map, so the poison-discipline
//! range checks pass); the global timeline is untouched. The sentinel
//! (RB_NIL) is read-only after init (every write in the tree ops is
//! guarded by a != nil check), so concurrent scheduling cannot perturb
//! these tests.
// (INV-TESTHARNESS-1: the in-kernel test-harness family — the timeline
// driver constructs adversarial shared state deliberately (the thing
// under test); runs only under the unit-test feature.)
use rux::sched::r#unsafe::{RbTimeline, VruntimeKey};
use rux::sched::types::SchedEntity;
use alloc::vec::Vec;
use rux::tests::{test_fail, test_group_start, test_pass};

/// Deterministic xorshift shuffle so the suite is reproducible.
fn shuffle(seed: &mut u64, n: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = ((*seed >> 33) as usize) % (i + 1);
        v.swap(i, j);
    }
    v
}

pub fn test_fair_timeline() {
    test_group_start("fair_timeline");

    // ---- Test 1: sorted order + leftmost cache over shuffled inserts ----
    {
        let mut tl = RbTimeline::new();
        let mut entities: Vec<SchedEntity> = (0..64).map(|_| SchedEntity::new()).collect();
        let order = shuffle(&mut 0xC0FFEE, 64);
        for &i in &order {
            let vr = (i as u64) * 1_000_000 + (i as u64 % 7) * 137;
            let key = VruntimeKey::new(vr, i as u64);
            entities[i].set_timeline_key(Some(key));
            if !tl.debug_insert(key, &mut entities[i]) {
                test_fail("sorted_insert", "insert refused (range check)");
                return;
            }
        }
        let keys = tl.debug_walk_keys();
        if keys.len() != 64 {
            test_fail("sorted_insert", "walk length wrong");
            return;
        }
        for w in keys.windows(2) {
            if w[0] >= w[1] {
                test_fail("sorted_insert", "walk not strictly increasing");
                return;
            }
        }
        match tl.debug_black_height() {
            Ok(bh) => test_pass(&alloc::format!("sorted_insert (black-height {})", bh)),
            Err(e) => {
                test_fail("sorted_insert", e);
                return;
            }
        }
    }

    // ---- Test 2: shuffled erase keeps order + balance; empty at end ----
    {
        let mut tl = RbTimeline::new();
        let mut entities: Vec<SchedEntity> = (0..64).map(|_| SchedEntity::new()).collect();
        for i in 0..64 {
            let vr = (i as u64) * 777_777;
            let key = VruntimeKey::new(vr, i as u64);
            entities[i].set_timeline_key(Some(key));
            tl.debug_insert(key, &mut entities[i]);
        }
        let order = shuffle(&mut 0xBADF00D, 64);
        for (step, &i) in order.iter().enumerate() {
            if !tl.debug_erase(&mut entities[i]) {
                test_fail("shuffled_erase", "erase refused");
                return;
            }
            entities[i].set_timeline_key(None);
            if step % 7 == 0 || step + 1 == order.len() {
                let remaining = 64 - step - 1;
                let keys = tl.debug_walk_keys();
                if keys.len() != remaining {
                    test_fail("shuffled_erase", "walk length wrong after erase");
                    return;
                }
                for w in keys.windows(2) {
                    if w[0] >= w[1] {
                        test_fail("shuffled_erase", "walk order broken mid-erase");
                        return;
                    }
                }
                if let Err(e) = tl.debug_black_height() {
                    test_fail("shuffled_erase", e);
                    return;
                }
            }
        }
        if !tl.is_empty() {
            test_fail("shuffled_erase", "tree not empty after erasing all");
            return;
        }
        if tl.leftmost != tl.nil() {
            test_fail("shuffled_erase", "leftmost not nil on empty tree");
            return;
        }
        test_pass("shuffled_erase (order + balance + empty)");
    }

    // ---- Test 3: leftmost-cache discipline ----
    {
        let mut tl = RbTimeline::new();
        let mut entities: Vec<SchedEntity> = (0..16).map(|_| SchedEntity::new()).collect();
        // Insert in DESCENDING key order — every insert must lower the
        // leftmost cache.
        for i in 0..16u64 {
            let vr = (15 - i) * 100;
            let key = VruntimeKey::new(vr, i);
            entities[i as usize].set_timeline_key(Some(key));
            tl.debug_insert(key, &mut entities[i as usize]);
        }
        // The leftmost's stored key (the pick peek shape).
        let lkey = tl.debug_leftmost_key().unwrap();
        if lkey.vruntime != 0 {
            test_fail("leftmost_cache", "descending inserts did not lower the cache");
            return;
        }
        // Erase the leftmost repeatedly — the cache must follow.
        for expect in 0u64..15 {
            if !tl.debug_erase_leftmost() {
                test_fail("leftmost_cache", "leftmost erase refused");
                return;
            }
            let k = tl.debug_leftmost_key().unwrap();
            if k.vruntime != (expect + 1) * 100 {
                test_fail("leftmost_cache", "cache did not advance to successor");
                return;
            }
        }
        if let Err(e) = tl.debug_black_height() {
            test_fail("leftmost_cache", e);
            return;
        }
        test_pass("leftmost_cache (descending inserts + successor advance)");
    }

    // ---- Test 4: duplicate vruntime keys tie-break by task_id ----
    {
        let mut tl = RbTimeline::new();
        let mut entities: Vec<SchedEntity> = (0..8).map(|_| SchedEntity::new()).collect();
        for i in 0..8u64 {
            let key = VruntimeKey::new(5_000, i);
            entities[i as usize].set_timeline_key(Some(key));
            tl.debug_insert(key, &mut entities[i as usize]);
        }
        let keys = tl.debug_walk_keys();
        let ids: Vec<u64> = keys.iter().map(|k| {
            if k.vruntime != 5_000 {
                test_fail("tie_keys", "vruntime corrupted");
                return u64::MAX;
            }
            k.task_id
        }).collect();
        for w in ids.windows(2) {
            if w[0] >= w[1] {
                test_fail("tie_keys", "task_id tie-break broken");
                return;
            }
        }
        if let Err(e) = tl.debug_black_height() {
            test_fail("tie_keys", e);
            return;
        }
        test_pass("tie_keys (task_id ordering under equal vruntime)");
    }

    // ---- Test 5: re-insert after erase (the switch-out requeue shape) ----
    {
        let mut tl = RbTimeline::new();
        let mut entities: Vec<SchedEntity> = (0..4).map(|_| SchedEntity::new()).collect();
        for round in 0..8u64 {
            for i in 0..4usize {
                let vr = ((i as u64) * 250 + round * 7) % 1_000;
                let key = VruntimeKey::new(vr, (round * 4 + i as u64));
                entities[i].set_timeline_key(Some(key));
                tl.debug_insert(key, &mut entities[i]);
            }
            let keys = tl.debug_walk_keys();
            if keys.len() != 4 {
                test_fail("reinsert_cycle", "walk length wrong");
                return;
            }
            for w in keys.windows(2) {
                if w[0] >= w[1] {
                    test_fail("reinsert_cycle", "order broken");
                    return;
                }
            }
            if let Err(e) = tl.debug_black_height() {
                test_fail("reinsert_cycle", e);
                return;
            }
            for i in 0..4usize {
                if !tl.debug_erase(&mut entities[i]) {
                    test_fail("reinsert_cycle", "erase refused");
                    return;
                }
                entities[i].set_timeline_key(None);
            }
        }
        if !tl.is_empty() {
            test_fail("reinsert_cycle", "not empty at end of rounds");
            return;
        }
        test_pass("reinsert_cycle (pick/requeue pair x8 rounds)");
    }
}
