//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for SoftirqIndex discriminant invariants.
//!
//! Types copied from: kernel/src/interrupt/softirq.rs

#![cfg(kani)]

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftirqIndex {
    Hi = 0, Timer = 1, NetTx = 2, NetRx = 3, Block = 4,
    IrqPoll = 5, Tasklet = 6, Sched = 7, Hrtimer = 8, Rcu = 9,
}

pub const NR_SOFTIRQS: usize = 10;

/// INV-SOFTIRQ-K1: discriminants are consecutive 0-9.
#[kani::proof]
fn verify_consecutive() {
    let indices = [
        SoftirqIndex::Hi, SoftirqIndex::Timer, SoftirqIndex::NetTx,
        SoftirqIndex::NetRx, SoftirqIndex::Block, SoftirqIndex::IrqPoll,
        SoftirqIndex::Tasklet, SoftirqIndex::Sched, SoftirqIndex::Hrtimer,
        SoftirqIndex::Rcu,
    ];
    for (i, idx) in indices.iter().enumerate() {
        assert_eq!(*idx as usize, i);
    }
}

/// INV-SOFTIRQ-K2: NR_SOFTIRQS matches variant count.
#[kani::proof]
fn verify_nr_softirqs() {
    assert_eq!(NR_SOFTIRQS, 10);
    assert_eq!(SoftirqIndex::Rcu as usize + 1, NR_SOFTIRQS);
}

/// INV-SOFTIRQ-K3: all indices in range 0..NR_SOFTIRQS.
#[kani::proof]
fn verify_indices_in_range() {
    let indices = [
        SoftirqIndex::Hi, SoftirqIndex::Timer, SoftirqIndex::NetTx,
        SoftirqIndex::NetRx, SoftirqIndex::Block, SoftirqIndex::IrqPoll,
        SoftirqIndex::Tasklet, SoftirqIndex::Sched, SoftirqIndex::Hrtimer,
        SoftirqIndex::Rcu,
    ];
    for &idx in &indices {
        assert!((idx as usize) < NR_SOFTIRQS);
    }
}

/// INV-SOFTIRQ-K4: specific assignments match kernel convention.
#[kani::proof]
fn verify_specific_assignments() {
    assert_eq!(SoftirqIndex::Hi as usize, 0);
    assert_eq!(SoftirqIndex::Timer as usize, 1);
    assert_eq!(SoftirqIndex::NetTx as usize, 2);
    assert_eq!(SoftirqIndex::NetRx as usize, 3);
    assert_eq!(SoftirqIndex::Block as usize, 4);
    assert_eq!(SoftirqIndex::Tasklet as usize, 6);
    assert_eq!(SoftirqIndex::Rcu as usize, 9);
}
