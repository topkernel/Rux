//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! grq-10ns: intrusive red-black CFS timeline unit tests (the
//! entity-embedded rbtree that replaced the BTreeMap — see
//! kernel/src/sched/fair.rs). Runs against a LOCAL RbTimeline with
//! heap-allocated SchedEntity arrays (entities live in the linear map,
//! so the poison-discipline range checks pass); the global timeline is
//! untouched. The sentinel (RB_NIL) is read-only after init (every
//! write in the tree ops is guarded by a != nil check), so concurrent
//! scheduling cannot perturb these tests.
// (INV-TESTHARNESS-1: the in-kernel test-harness family — fair_timeline/
// syscall_time/small_alloc tests construct adversarial shared state
// with raw statics deliberately (the thing under test); runs only
// under the unit-test feature.
use crate::sched::fair::{RbTimeline, SchedEntity};
use alloc::vec::Vec;
use super::{test_pass, test_fail, test_group_start};

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

fn node_at(entities: &mut [SchedEntity], i: usize) -> *mut crate::sched::fair::RbNode {
    core::ptr::addr_of_mut!(entities[i].run_node)
}

/// Walk the successor chain from the leftmost, collecting the stored
/// keys. Stops at nil or the first poison bail-out (none expected: all
/// entities are heap addresses).
fn walk_keys(tl: &mut RbTimeline, entities: &mut [SchedEntity]) -> Vec<(u64, u64)> {
    let nil = tl.nil();
    let mut node = tl.leftmost;
    let mut out = Vec::new();
    while node != nil && !node.is_null() {
        // SAFETY: test-owned, linked nodes.
        let k = unsafe {
            let se = (node as *mut u8)
                .sub(core::mem::offset_of!(SchedEntity, run_node))
                as *mut SchedEntity;
            (*se).timeline_key().unwrap_or(crate::sched::fair::VruntimeKey::new(u64::MAX, u64::MAX))
        };
        out.push((k.vruntime, k.task_id));
        // SAFETY: as above.
        node = unsafe { tl.next(node) };
    }
    out
}

/// Black-height / balance audit: from every node down to the leaves,
/// the count of black nodes (excluding the node) must be equal on all
/// paths, and no red node has a red child. Returns Err(description).
fn audit(tl: &mut RbTimeline) -> Result<u64, &'static str> {
    let nil = tl.nil();
    // SAFETY: audit reads only link/color fields of linked nodes.
    unsafe fn bh(tl: &RbTimeline, node: *mut crate::sched::fair::RbNode, nil: *mut crate::sched::fair::RbNode) -> Result<u64, &'static str> {
        if node == nil {
            return Ok(0);
        }
        // SAFETY: linked node.
        let (l, r, red) = unsafe { ((*node).left, (*node).right, (*node).red) };
        if red {
            // SAFETY: children.
            unsafe {
                if l != nil && (*l).red {
                    return Err("red node with red left child");
                }
                if r != nil && (*r).red {
                    return Err("red node with red right child");
                }
            }
        }
        // SAFETY: recursion on children.
        let bl = unsafe { bh(tl, l, nil)? };
        let br = unsafe { bh(tl, r, nil)? };
        if bl != br {
            return Err("black-height mismatch");
        }
        Ok(bl + if red { 0 } else { 1 })
    }
    // SAFETY: root is linked or nil.
    unsafe {
        if tl.root != nil && (*tl.root).red {
            return Err("root is red");
        }
        bh(tl, tl.root, nil)
    }
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
            // SAFETY: test-owned entity.
            unsafe {
                (*core::ptr::addr_of_mut!(entities[i].timeline_key)).replace(Some(
                    crate::sched::fair::VruntimeKey::new(vr, i as u64),
                ));
            }
            // SAFETY: heap entity in the linear map.
            let ok = unsafe { tl.insert(crate::sched::fair::VruntimeKey::new(vr, i as u64), node_at(&mut entities, i)) };
            if !ok {
                test_fail("sorted_insert", "insert refused (range check)");
                return;
            }
        }
        let keys = walk_keys(&mut tl, &mut entities);
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
        match audit(&mut tl) {
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
            // SAFETY: test-owned entity.
            unsafe {
                (*core::ptr::addr_of_mut!(entities[i].timeline_key)).replace(Some(
                    crate::sched::fair::VruntimeKey::new(vr, i as u64),
                ));
                // SAFETY: heap entity in the linear map.
                tl.insert(crate::sched::fair::VruntimeKey::new(vr, i as u64), node_at(&mut entities, i));
            }
        }
        let order = shuffle(&mut 0xBADF00D, 64);
        for (step, &i) in order.iter().enumerate() {
            // SAFETY: node is linked.
            if !unsafe { tl.erase(node_at(&mut entities, i)) } {
                test_fail("shuffled_erase", "erase refused");
                return;
            }
            // SAFETY: entity field write.
            unsafe {
                (*core::ptr::addr_of_mut!(entities[i].timeline_key)).get_mut().take();
            }
            if step % 7 == 0 || step + 1 == order.len() {
                let remaining = 64 - step - 1;
                let keys = walk_keys(&mut tl, &mut entities);
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
                if let Err(e) = audit(&mut tl) {
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
            // SAFETY: test-owned entity.
            unsafe {
                (*core::ptr::addr_of_mut!(entities[i as usize].timeline_key)).replace(Some(
                    crate::sched::fair::VruntimeKey::new(vr, i),
                ));
                // SAFETY: heap entity.
                tl.insert(crate::sched::fair::VruntimeKey::new(vr, i), node_at(&mut entities, i as usize));
            }
        }
        // SAFETY: leftmost is linked.
        let lkey = unsafe {
            let n = tl.leftmost;
            let se = (n as *mut u8).sub(core::mem::offset_of!(SchedEntity, run_node)) as *mut SchedEntity;
            (*se).timeline_key().unwrap()
        };
        if lkey.vruntime != 0 {
            test_fail("leftmost_cache", "descending inserts did not lower the cache");
            return;
        }
        // Erase the leftmost repeatedly — the cache must follow.
        for expect in 0u64..15 {
            // SAFETY: linked leftmost.
            if !unsafe { tl.erase(tl.leftmost) } {
                test_fail("leftmost_cache", "leftmost erase refused");
                return;
            }
            // SAFETY: as above.
            let k = unsafe {
                let n = tl.leftmost;
                let se = (n as *mut u8).sub(core::mem::offset_of!(SchedEntity, run_node)) as *mut SchedEntity;
                (*se).timeline_key().unwrap()
            };
            if k.vruntime != (expect + 1) * 100 {
                test_fail("leftmost_cache", "cache did not advance to successor");
                return;
            }
        }
        if let Err(e) = audit(&mut tl) {
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
            // SAFETY: test-owned entity.
            unsafe {
                (*core::ptr::addr_of_mut!(entities[i as usize].timeline_key)).replace(Some(
                    crate::sched::fair::VruntimeKey::new(5_000, i),
                ));
                // SAFETY: heap entity.
                tl.insert(crate::sched::fair::VruntimeKey::new(5_000, i), node_at(&mut entities, i as usize));
            }
        }
        let keys = walk_keys(&mut tl, &mut entities);
        let ids: Vec<u64> = keys.iter().map(|(vr, id)| {
            if *vr != 5_000 {
                test_fail("tie_keys", "vruntime corrupted");
                return u64::MAX;
            }
            *id
        }).collect();
        for w in ids.windows(2) {
            if w[0] >= w[1] {
                test_fail("tie_keys", "task_id tie-break broken");
                return;
            }
        }
        if let Err(e) = audit(&mut tl) {
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
                // SAFETY: test-owned entity.
                unsafe {
                    (*core::ptr::addr_of_mut!(entities[i].timeline_key)).replace(Some(
                        crate::sched::fair::VruntimeKey::new(vr, (round * 4 + i as u64)),
                    ));
                    // SAFETY: heap entity.
                    tl.insert(crate::sched::fair::VruntimeKey::new(vr, (round * 4 + i as u64)), node_at(&mut entities, i));
                }
            }
            let keys = walk_keys(&mut tl, &mut entities);
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
            if let Err(e) = audit(&mut tl) {
                test_fail("reinsert_cycle", e);
                return;
            }
            for i in 0..4usize {
                // SAFETY: linked node.
                if !unsafe { tl.erase(node_at(&mut entities, i)) } {
                    test_fail("reinsert_cycle", "erase refused");
                    return;
                }
                // SAFETY: entity field write.
                unsafe {
                    (*core::ptr::addr_of_mut!(entities[i].timeline_key)).get_mut().take();
                }
            }
        }
        if !tl.is_empty() {
            test_fail("reinsert_cycle", "not empty at end of rounds");
            return;
        }
        test_pass("reinsert_cycle (pick/requeue pair x8 rounds)");
    }
}
