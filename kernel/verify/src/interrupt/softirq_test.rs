//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Softirq vector index invariant tests.
//!
//! Types copied from: kernel/src/interrupt/softirq.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/interrupt/softirq.rs
// ============================================================================

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftirqIndex {
    Hi         = 0,
    Timer      = 1,
    NetTx      = 2,
    NetRx      = 3,
    Block      = 4,
    IrqPoll    = 5,
    Tasklet    = 6,
    Sched      = 7,
    Hrtimer    = 8,
    Rcu        = 9,
}

pub const NR_SOFTIRQS: usize = 10;

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SOFTIRQ-1: SoftirqIndex discriminants are consecutive 0-9
    #[test]
    fn test_consecutive_discriminants(idx in 0usize..NR_SOFTIRQS) {
        let indices = [
            SoftirqIndex::Hi,
            SoftirqIndex::Timer,
            SoftirqIndex::NetTx,
            SoftirqIndex::NetRx,
            SoftirqIndex::Block,
            SoftirqIndex::IrqPoll,
            SoftirqIndex::Tasklet,
            SoftirqIndex::Sched,
            SoftirqIndex::Hrtimer,
            SoftirqIndex::Rcu,
        ];
        prop_assert_eq!(indices[idx] as usize, idx);
    }
}

#[test]
/// INV-SOFTIRQ-2: NR_SOFTIRQS matches number of SoftirqIndex variants
fn test_nr_softirqs() {
    assert_eq!(NR_SOFTIRQS, 10);
    assert_eq!(SoftirqIndex::Rcu as usize + 1, NR_SOFTIRQS);
}

#[test]
/// INV-SOFTIRQ-3: All SoftirqIndex variants have distinct discriminants
fn test_distinct_discriminants() {
    let indices = [
        SoftirqIndex::Hi as usize,
        SoftirqIndex::Timer as usize,
        SoftirqIndex::NetTx as usize,
        SoftirqIndex::NetRx as usize,
        SoftirqIndex::Block as usize,
        SoftirqIndex::IrqPoll as usize,
        SoftirqIndex::Tasklet as usize,
        SoftirqIndex::Sched as usize,
        SoftirqIndex::Hrtimer as usize,
        SoftirqIndex::Rcu as usize,
    ];
    let mut seen = std::collections::HashSet::new();
    for &idx in &indices {
        assert!(seen.insert(idx), "duplicate softirq index: {}", idx);
    }
}

#[test]
/// INV-SOFTIRQ-4: Specific softirq assignments match kernel convention
fn test_specific_assignments() {
    assert_eq!(SoftirqIndex::Hi as usize, 0);      // High priority
    assert_eq!(SoftirqIndex::Timer as usize, 1);   // Timer
    assert_eq!(SoftirqIndex::NetTx as usize, 2);   // Network TX
    assert_eq!(SoftirqIndex::NetRx as usize, 3);   // Network RX
    assert_eq!(SoftirqIndex::Block as usize, 4);   // Block IO
    assert_eq!(SoftirqIndex::IrqPoll as usize, 5); // IRQ poll
    assert_eq!(SoftirqIndex::Tasklet as usize, 6); // Tasklet
    assert_eq!(SoftirqIndex::Sched as usize, 7);   // Scheduler
    assert_eq!(SoftirqIndex::Hrtimer as usize, 8); // High-res timer
    assert_eq!(SoftirqIndex::Rcu as usize, 9);     // RCU
}

#[test]
/// INV-SOFTIRQ-5: All indices are in range 0..NR_SOFTIRQS
fn test_indices_in_range() {
    let indices = [
        SoftirqIndex::Hi, SoftirqIndex::Timer, SoftirqIndex::NetTx,
        SoftirqIndex::NetRx, SoftirqIndex::Block, SoftirqIndex::IrqPoll,
        SoftirqIndex::Tasklet, SoftirqIndex::Sched, SoftirqIndex::Hrtimer,
        SoftirqIndex::Rcu,
    ];
    for &idx in &indices {
        let val = idx as usize;
        assert!(val < NR_SOFTIRQS, "softirq index {} out of range", val);
    }
}

#[test]
/// INV-SOFTIRQ-6: SoftirqIndex enum has exactly NR_SOFTIRQS variants
fn test_variant_count() {
    let all = [
        SoftirqIndex::Hi, SoftirqIndex::Timer, SoftirqIndex::NetTx,
        SoftirqIndex::NetRx, SoftirqIndex::Block, SoftirqIndex::IrqPoll,
        SoftirqIndex::Tasklet, SoftirqIndex::Sched, SoftirqIndex::Hrtimer,
        SoftirqIndex::Rcu,
    ];
    assert_eq!(all.len(), NR_SOFTIRQS);
}
