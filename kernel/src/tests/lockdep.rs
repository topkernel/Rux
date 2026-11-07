//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! lockdep-lite P0 registry tests (design doc §10, P0 half): the
//! shipped registry validates clean, and the validator's negative
//! halves fire on synthetic tables (a cycle, a level violation, a
//! sleep-inside-spin edge, a duplicate name, a drifted lockkey
//! mirror). The negatives run on throwaway tables — the real registry
//! is never corrupted.

use super::{test_fail, test_group_start, test_pass};
use crate::debug::lockdep::{
    validate, validate_lockkey_mirror, DeclaredEdge, LockClassDesc, LockDomain, LockClassId,
    CLASSES, DECLARED_EDGES, KEYED_IMPL_MIRROR, ViolationKind,
};

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

/// Build one class row (test tables need compact construction).
fn cls(name: &str, domain: LockDomain, level: u8) -> LockClassDesc {
    LockClassDesc { name: leak(name), domain, level, doc: "test", key: None }
}

/// Tests build synthetic &'static str names; leak into 'static via
/// Box (the unit-test kernel never frees test fixtures).
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn edge(outer: u16, inner: u16) -> DeclaredEdge {
    DeclaredEdge { outer: LockClassId(outer), inner: LockClassId(inner), inv: "test" }
}

pub fn test_lockdep() {
    test_group_start("lockdep");

    // ---- The shipped registry validates clean (structural + mirror). ----
    let mut findings = validate(CLASSES, DECLARED_EDGES);
    findings.extend(validate_lockkey_mirror(CLASSES, DECLARED_EDGES));
    if findings.is_empty() {
        test_pass("registry validates clean");
    } else {
        for (kind, detail) in &findings {
            super::test_fail(
                "registry validates clean",
                &alloc::format!("{}: {}", kind.as_str(), detail),
            );
        }
    }

    // ---- Synthetic tables: each negative half fires exactly. ----
    // Cycle: a -> b -> a.
    {
        let classes = [cls("a", LockDomain::Spin, 0), cls("b", LockDomain::Spin, 1)];
        let edges = [edge(0, 1), edge(1, 0)];
        let v = validate(&classes, &edges);
        assert_kind(&v, ViolationKind::Cycle, "cycle detected");
    }
    // Level violation: outer deeper than inner.
    {
        let classes = [cls("outer", LockDomain::Spin, 3), cls("inner", LockDomain::Spin, 2)];
        let edges = [edge(0, 1)];
        let v = validate(&classes, &edges);
        assert_kind(&v, ViolationKind::LevelOrder, "level-order violation");
    }
    // Sleep-domain class nested inside a spin-family hold.
    {
        let classes = [cls("spinhold", LockDomain::Spin, 0), cls("sleeper", LockDomain::Sleep, 1)];
        let edges = [edge(0, 1)];
        let v = validate(&classes, &edges);
        assert_kind(&v, ViolationKind::SleepInsideSpin, "sleep-inside-spin");
    }
    // The reverse (spin inside sleep) is the sanctioned coarse-lock
    // pattern and must NOT be a violation.
    {
        let classes = [cls("sleepouter", LockDomain::Sleep, 0), cls("spininner", LockDomain::Spin, 1)];
        let edges = [edge(0, 1)];
        let v = validate(&classes, &edges);
        if v.is_empty() {
            test_pass("spin-inside-sleep stays legal");
        } else {
            test_fail("spin-inside-sleep stays legal", "false violation");
        }
    }
    // Duplicate class names.
    {
        let classes = [cls("dup", LockDomain::Spin, 0), cls("dup", LockDomain::Spin, 1)];
        let v = validate(&classes, &[]);
        assert_kind(&v, ViolationKind::DuplicateName, "duplicate name");
    }
    // Lockkey mirror drift: a mirror pair without a declared edge
    // (validate_lockkey_mirror on a partial table — the mirror is a
    // full-registry property, so the synthetic tables above never
    // trip it).
    {
        // Two real mirror pairs exist; build a table that declares
        // only the first pair's edge — the second must drift.
        let mut classes = Vec::new();
        for (o, i) in KEYED_IMPL_MIRROR.iter() {
            if !classes.iter().any(|c: &LockClassDesc| c.name == *o) {
                classes.push(cls(o, LockDomain::Spin, 0));
            }
            if !classes.iter().any(|c: &LockClassDesc| c.name == *i) {
                classes.push(cls(i, LockDomain::Spin, 1));
            }
        }
        // Declare only the FIRST mirror pair as an edge.
        let first = KEYED_IMPL_MIRROR[0];
        let oi = (
            classes.iter().position(|c: &LockClassDesc| c.name == first.0).unwrap(),
            classes.iter().position(|c: &LockClassDesc| c.name == first.1).unwrap(),
        );
        let edges = [edge(oi.0 as u16, oi.1 as u16)];
        let v = validate_lockkey_mirror(&classes, &edges);
        assert_kind(&v, ViolationKind::LockkeyMirrorDrift, "lockkey mirror drift");
    }

    // ---- Mirror pin vs sync/lockkey.rs (the anti-drift check). ----
    // The pinned set is exactly the three CanNestInside impls that
    // exist today; when a new impl lands, this test fails until the
    // mirror (and the registry edge) is added — drift is a defect.
    {
        let expected = [
            ("futex-bucket", "grq"),        // GrqKey inside FutexBucketKey
            ("net-queue", "net-stats"),     // NetStatsKey inside NetQueueKey
            ("futex-bucket", "futex-waiter-slot"), // WaiterSlotKey inside FutexBucketKey
        ];
        let ok = expected.len() == KEYED_IMPL_MIRROR.len()
            && expected.iter().zip(KEYED_IMPL_MIRROR.iter()).all(|(a, b)| a == b);
        if ok {
            test_pass("lockkey impl mirror pinned");
        } else {
            test_fail(
                "lockkey impl mirror pinned",
                "sync/lockkey.rs gained or lost a CanNestInside impl; update KEYED_IMPL_MIRROR",
            );
        }
    }

    // ---- /proc/lockdep renders every class row. ----
    {
        let text = crate::debug::lockdep::generate_proc_lockdep();
        let s = core::str::from_utf8(&text).unwrap_or("");
        let rows = s.lines().filter(|l| l.contains("....: ")).count();
        if rows == CLASSES.len() {
            test_pass("/proc/lockdep class rows");
        } else {
            test_fail("/proc/lockdep class rows", "row count mismatch");
        }
    }
}

/// Assert one finding of `kind` is present; report pass/fail by name.
fn assert_kind(v: &[(ViolationKind, alloc::string::String)], kind: ViolationKind, name: &str) {
    if v.iter().any(|(k, _)| *k == kind) {
        test_pass(name);
    } else {
        test_fail(name, "expected violation not reported");
    }
}

// ============================================================================
// P1 runtime tests (design doc §10, P1 half) — compiled only under the
// cargo feature `debug-lockdep`. Drive the check pipeline directly on
// local held stacks (no real wedging); each scenario resets the global
// arenas via reset_for_test so the suite's later boots start clean.
// ============================================================================

#[cfg(feature = "debug-lockdep")]
pub fn test_lockdep_runtime() {
    use crate::debug::lockdep::{
        check_acquire, counters, reset_for_test, HeldStack, Verdict, EXT4_BIG, FUTEX_BUCKET,
        GRQ, INODE, NET_QUEUE, NET_STATS, PROC_TREE, VFS_MUTATION, WAITER_SLOT,
    };

    // ---- s0: the key->registry mapping (drift is a defect). ----
    {
        use crate::sync::lockkey::{FutexBucketKey, GrqKey, NestKey, NetQueueKey, NetStatsKey, WaiterSlotKey};
        let ok = [
            (GrqKey::LOCK_CLASS, GrqKey::NAME, "grq"),
            (FutexBucketKey::LOCK_CLASS, FutexBucketKey::NAME, "futex-bucket"),
            (WaiterSlotKey::LOCK_CLASS, WaiterSlotKey::NAME, "futex-waiter-slot"),
            (NetQueueKey::LOCK_CLASS, NetQueueKey::NAME, "net-queue"),
            (NetStatsKey::LOCK_CLASS, NetStatsKey::NAME, "net-stats"),
        ]
        .iter()
        .all(|(class, key_name, table)| {
            CLASSES
                .get(class.0 as usize)
                .map(|c| c.name == *key_name && c.name == *table)
                .unwrap_or(false)
        });
        if ok {
            test_pass("p1 key<->registry mapping");
        } else {
            test_fail("p1 key<->registry mapping", "LOCK_CLASS drifted from its table row");
        }
    }

    // ---- s1: declared-order forward passes (every declared keyed
    // edge, one by one). ----
    {
        reset_for_test();
        let mut all_ok = true;
        // futex-bucket -> grq (INV-LOCK-5)
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, FUTEX_BUCKET, 0x1000, 0);
        all_ok &= matches!(
            check_acquire(&mut st, GRQ, 0x1004, 0),
            Verdict::Ok | Verdict::OkChainHit
        );
        // futex-bucket -> waiter-slot (INV-LOCK-5)
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, FUTEX_BUCKET, 0x1008, 0);
        all_ok &= matches!(
            check_acquire(&mut st, WAITER_SLOT, 0x100c, 0),
            Verdict::Ok | Verdict::OkChainHit
        );
        // net-queue -> net-stats (INV-LOCK-6)
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, NET_QUEUE, 0x1010, 0);
        all_ok &= matches!(
            check_acquire(&mut st, NET_STATS, 0x1014, 0),
            Verdict::Ok | Verdict::OkChainHit
        );
        // vfs-mutation -> ext4-big (INV-LOCK-8)
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, VFS_MUTATION, 0x1018, 0);
        all_ok &= matches!(
            check_acquire(&mut st, EXT4_BIG, 0x101c, 0),
            Verdict::Ok | Verdict::OkChainHit
        );
        if all_ok {
            test_pass("p1 declared forward orders pass");
        } else {
            test_fail("p1 declared forward orders pass", "a declared order was flagged");
        }
    }

    // ---- s2: forced reverse of INV-LOCK-5 = ORDER-VIOLATION. ----
    {
        reset_for_test();
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, GRQ, 0x2000, 0);
        match check_acquire(&mut st, FUTEX_BUCKET, 0x2004, 0) {
            Verdict::OrderViolation { held, acquiring, inv } => {
                if held == GRQ && acquiring == FUTEX_BUCKET && inv == "INV-LOCK-5" {
                    test_pass("p1 reverse of INV-LOCK-5 splats");
                } else {
                    test_fail("p1 reverse of INV-LOCK-5 splats", "wrong classes/inv in verdict");
                }
            }
            v => {
                test_fail("p1 reverse of INV-LOCK-5 splats", &alloc::format!("verdict {:?}", v));
            }
        }
    }

    // ---- s3: INV-LOCK-8 context negatives. ----
    {
        // Spin held + sleep acquire.
        reset_for_test();
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, GRQ, 0x3000, 0);
        match check_acquire(&mut st, VFS_MUTATION, 0x3004, 0) {
            Verdict::SleepInAtomic { class, held_spin, .. } => {
                if class == VFS_MUTATION && held_spin == Some(GRQ) {
                    test_pass("p1 sleep-under-spin splats");
                } else {
                    test_fail("p1 sleep-under-spin splats", "wrong fields");
                }
            }
            v => {
                test_fail("p1 sleep-under-spin splats", &alloc::format!("verdict {:?}", v));
            }
        }
        // preempt_count != 0, nothing held.
        reset_for_test();
        let mut st = HeldStack::new();
        match check_acquire(&mut st, EXT4_BIG, 0x3008, 0x10001) {
            Verdict::SleepInAtomic { preempt, held_spin, .. } => {
                if preempt == 0x10001 && held_spin.is_none() {
                    test_pass("p1 sleep-in-preempt-ctx splats");
                } else {
                    test_fail("p1 sleep-in-preempt-ctx splats", "wrong fields");
                }
            }
            v => {
                test_fail("p1 sleep-in-preempt-ctx splats", &alloc::format!("verdict {:?}", v));
            }
        }
        // The spin-under-sleep nesting stays LEGAL (the coarse-lock
        // pattern): only the sleep-domain acquire is checked.
        reset_for_test();
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, VFS_MUTATION, 0x300c, 0);
        match check_acquire(&mut st, GRQ, 0x3010, 0) {
            Verdict::Ok | Verdict::OkChainHit => {
                test_pass("p1 spin-inside-sleep stays legal");
            }
            v => {
                test_fail("p1 spin-inside-sleep stays legal", &alloc::format!("verdict {:?}", v));
            }
        }
    }

    // ---- s4: two-task observed cycle (INV-LOCK-4). ----
    {
        reset_for_test();
        let mut st1 = HeldStack::new();
        let _ = check_acquire(&mut st1, PROC_TREE, 0x4000, 0);
        let _ = check_acquire(&mut st1, INODE, 0x4004, 0);
        let mut st2 = HeldStack::new();
        let _ = check_acquire(&mut st2, INODE, 0x4008, 0);
        match check_acquire(&mut st2, PROC_TREE, 0x400c, 0) {
            Verdict::Cycle { .. } => test_pass("p1 observed cycle splats"),
            v => test_fail("p1 observed cycle splats", &alloc::format!("verdict {:?}", v)),
        }
    }

    // ---- s5: chain-key amortization: the second identical chain is
    // a hit, and the counter increments exactly once per unique
    // chain. ----
    {
        reset_for_test();
        let (_, hits0, chains0, _, bfs0, _, _) = counters();
        let mut a = HeldStack::new();
        let _ = check_acquire(&mut a, FUTEX_BUCKET, 0x5000, 0);
        let _ = check_acquire(&mut a, WAITER_SLOT, 0x5004, 0);
        let mut b = HeldStack::new();
        let _ = check_acquire(&mut b, FUTEX_BUCKET, 0x5008, 0);
        let v = check_acquire(&mut b, WAITER_SLOT, 0x500c, 0);
        let (_, hits1, chains1, _, bfs1, _, _) = counters();
        let amortized = matches!(v, Verdict::OkChainHit)
            && hits1 == hits0 + 1
            && chains1 >= chains0 + 1
            && bfs1 == bfs0 + 1; // the second chain ran NO new BFS
        if amortized {
            test_pass("p1 chain-key amortization");
        } else {
            test_fail(
                "p1 chain-key amortization",
                &alloc::format!(
                    "verdict {:?} hits {}->{} chains {}->{} bfs {}->{}",
                    v, hits0, hits1, chains0, chains1, bfs0, bfs1
                ),
            );
        }
    }

    // ---- s6: same-class re-entry is recursive-legal (no self-edge,
    // no checks). ----
    {
        reset_for_test();
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, VFS_MUTATION, 0x6000, 0);
        match check_acquire(&mut st, VFS_MUTATION, 0x6004, 0) {
            Verdict::OkRecursive => test_pass("p1 task-recursive re-entry legal"),
            v => test_fail(
                "p1 task-recursive re-entry legal",
                &alloc::format!("verdict {:?}", v),
            ),
        }
    }

    // ---- s7: /proc/lockdep live view + /proc/lockdep_chains. ----
    {
        reset_for_test();
        // Record one real chain, then render both files.
        let mut st = HeldStack::new();
        let _ = check_acquire(&mut st, FUTEX_BUCKET, 0x7000, 0);
        let _ = check_acquire(&mut st, WAITER_SLOT, 0x7004, 0);

        let text = crate::debug::lockdep::generate_proc_lockdep();
        let t = core::str::from_utf8(&text).unwrap_or("");
        let live_ok = t.contains("runtime ARMED")
            && t.contains("acquires=2")
            && t.contains("chains=2")
            && t.contains("edges=2")
            && t.contains("observed at");
        if live_ok {
            test_pass("p1 /proc/lockdep live view");
        } else {
            test_fail("p1 /proc/lockdep live view", "live counters/edges missing from render");
        }

        let chains = crate::debug::lockdep::generate_proc_lockdep_chains();
        let c = core::str::from_utf8(&chains).unwrap_or("");
        // The two acquires above recorded chains [futex-bucket] and
        // [futex-bucket, futex-waiter-slot].
        let chains_ok = c.contains("all lock chains:")
            && c.contains("irq_context: 0")
            && c.contains("[0x0002] futex-bucket")
            && c.contains("[0x0003] futex-waiter-slot");
        if chains_ok {
            test_pass("p1 /proc/lockdep_chains render");
        } else {
            test_fail("p1 /proc/lockdep_chains render", "chain rows missing");
        }
    }

    // Leave the checker armed + arenas clean for any later boot code.
    reset_for_test();
}
