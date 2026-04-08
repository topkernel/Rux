//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Scheduling class ID and flags invariant tests.
//!
//! Types copied from: kernel/src/sched/class.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/sched/class.rs
// ============================================================================

pub const ENQUEUE_WAKEUP: i32 = 0x0001;
pub const ENQUEUE_RESTORE: i32 = 0x0002;
pub const ENQUEUE_MOVE: i32 = 0x0004;
pub const ENQUEUE_NOCLOCK: i32 = 0x0008;
pub const ENQUEUE_MIGRATING: i32 = 0x0010;
pub const ENQUEUE_HEAD: i32 = 0x00010000;
pub const ENQUEUE_REPLENISH: i32 = 0x00020000;
pub const ENQUEUE_MIGRATED: i32 = 0x00040000;

pub const DEQUEUE_SLEEP: i32 = 0x0001;
pub const DEQUEUE_SAVE: i32 = 0x0002;
pub const DEQUEUE_MOVE: i32 = 0x0004;
pub const DEQUEUE_NOCLOCK: i32 = 0x0008;
pub const DEQUEUE_MIGRATING: i32 = 0x0010;

pub const WF_EXEC: i32 = 0x02;
pub const WF_FORK: i32 = 0x04;
pub const WF_TTWU: i32 = 0x08;
pub const WF_SYNC: i32 = 0x10;
pub const WF_MIGRATED: i32 = 0x20;
pub const WF_CURRENT_CPU: i32 = 0x40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SchedClassId {
    Stop = 0,
    Deadline = 1,
    Rt = 2,
    Fair = 3,
    Idle = 4,
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SCHEDCLASS-1: SchedClassId ordering is Stop < Deadline < Rt < Fair < Idle
    #[test]
    fn test_ordering(
        a in 0u8..5u8,
        b in 0u8..5u8,
    ) {
        let ids = [
            SchedClassId::Stop,
            SchedClassId::Deadline,
            SchedClassId::Rt,
            SchedClassId::Fair,
            SchedClassId::Idle,
        ];
        let id_a = ids[a as usize];
        let id_b = ids[b as usize];
        // a < b implies id_a < id_b
        prop_assert_eq!(a < b, id_a < id_b);
    }
}

#[test]
/// INV-SCHEDCLASS-2: SchedClassId discriminants are 0-4
fn test_discriminants() {
    assert_eq!(SchedClassId::Stop as u8, 0);
    assert_eq!(SchedClassId::Deadline as u8, 1);
    assert_eq!(SchedClassId::Rt as u8, 2);
    assert_eq!(SchedClassId::Fair as u8, 3);
    assert_eq!(SchedClassId::Idle as u8, 4);
}

#[test]
/// INV-SCHEDCLASS-3: There are exactly 5 scheduling classes
fn test_class_count() {
    let count = [
        SchedClassId::Stop,
        SchedClassId::Deadline,
        SchedClassId::Rt,
        SchedClassId::Fair,
        SchedClassId::Idle,
    ].len();
    assert_eq!(count, 5);
}

#[test]
/// INV-SCHEDCLASS-4: ENQUEUE_WAKEUP and DEQUEUE_SLEEP share value 0x0001
fn test_enqueue_dequeue_overlap() {
    // This is intentional: they represent wake/sleep operations
    assert_eq!(ENQUEUE_WAKEUP, DEQUEUE_SLEEP);
}

#[test]
/// INV-SCHEDCLASS-5: ENQUEUE flags are distinct powers of two
fn test_enqueue_flags_distinct() {
    let flags = [
        ENQUEUE_WAKEUP, ENQUEUE_RESTORE, ENQUEUE_MOVE,
        ENQUEUE_NOCLOCK, ENQUEUE_MIGRATING,
        ENQUEUE_HEAD, ENQUEUE_REPLENISH, ENQUEUE_MIGRATED,
    ];
    let mut seen = std::collections::HashSet::new();
    for &f in &flags {
        assert!(f > 0 && (f as u32 & (f as u32 - 1)) == 0,
            "ENQUEUE flag {:#x} is not a power of two", f);
        assert!(seen.insert(f), "duplicate ENQUEUE flag {:#x}", f);
    }
}

#[test]
/// INV-SCHEDCLASS-6: DEQUEUE flags are distinct powers of two
fn test_dequeue_flags_distinct() {
    let flags = [
        DEQUEUE_SLEEP, DEQUEUE_SAVE, DEQUEUE_MOVE,
        DEQUEUE_NOCLOCK, DEQUEUE_MIGRATING,
    ];
    let mut seen = std::collections::HashSet::new();
    for &f in &flags {
        assert!(f > 0 && (f as u32 & (f as u32 - 1)) == 0,
            "DEQUEUE flag {:#x} is not a power of two", f);
        assert!(seen.insert(f), "duplicate DEQUEUE flag {:#x}", f);
    }
}

#[test]
/// INV-SCHEDCLASS-7: WF (wake) flags are distinct powers of two
fn test_wake_flags_distinct() {
    let flags = [
        WF_EXEC, WF_FORK, WF_TTWU, WF_SYNC,
        WF_MIGRATED, WF_CURRENT_CPU,
    ];
    let mut seen = std::collections::HashSet::new();
    for &f in &flags {
        assert!(f > 0 && (f as u32 & (f as u32 - 1)) == 0,
            "WF flag {:#x} is not a power of two", f);
        assert!(seen.insert(f), "duplicate WF flag {:#x}", f);
    }
}

#[test]
/// INV-SCHEDCLASS-8: ENQUEUE high flags (bits 16+) don't overlap WF flags (bits 0-7)
fn test_enqueue_high_flags_no_wf_overlap() {
    let enq_high = ENQUEUE_HEAD | ENQUEUE_REPLENISH | ENQUEUE_MIGRATED;
    let wf_all = WF_EXEC | WF_FORK | WF_TTWU | WF_SYNC | WF_MIGRATED | WF_CURRENT_CPU;
    assert_eq!(enq_high & wf_all, 0);
}

#[test]
/// INV-SCHEDCLASS-9: PartialOrd chain Stop < Deadline < Rt < Fair < Idle
fn test_partial_ord_chain() {
    assert!(SchedClassId::Stop < SchedClassId::Deadline);
    assert!(SchedClassId::Deadline < SchedClassId::Rt);
    assert!(SchedClassId::Rt < SchedClassId::Fair);
    assert!(SchedClassId::Fair < SchedClassId::Idle);
    assert!(SchedClassId::Stop < SchedClassId::Idle);
}
