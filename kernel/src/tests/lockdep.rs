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
