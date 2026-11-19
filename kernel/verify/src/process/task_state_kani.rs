//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for TaskState bitmap operations.
//!
//! Types copied from: kernel/src/process/task.rs

#![cfg(kani)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskState(u32);

impl TaskState {
    pub const RUNNING: u32 = 0x00000000;
    pub const INTERRUPTIBLE: u32 = 0x00000001;
    pub const UNINTERRUPTIBLE: u32 = 0x00000002;
    pub const STOPPED: u32 = 0x00000004;
    pub const ZOMBIE: u32 = 0x00000010;
    pub const DEAD: u32 = 0x00000020;

    pub const fn new(bits: u32) -> Self { TaskState(bits) }
    pub fn bits(&self) -> u32 { self.0 }
    pub fn contains(&self, flag: u32) -> bool { (self.0 & flag) != 0 }
    pub fn is_running(&self) -> bool { self.0 == Self::RUNNING }
    pub fn is_sleeping(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE) || self.contains(Self::UNINTERRUPTIBLE)
    }
    pub fn is_dead(&self) -> bool {
        self.contains(Self::ZOMBIE) || self.contains(Self::DEAD)
    }
}

/// INV-TASKSTATE-K1: constants (except RUNNING) are distinct powers of 2.
#[kani::proof]
fn verify_constants_distinct() {
    let consts = [
        TaskState::INTERRUPTIBLE, TaskState::UNINTERRUPTIBLE,
        TaskState::STOPPED, TaskState::ZOMBIE, TaskState::DEAD,
    ];
    let mut seen = 0u32;
    for &c in &consts {
        assert!(c > 0 && (c & (c - 1)) == 0, "not power of 2: {:#x}", c);
        assert_eq!(seen & c, 0, "duplicate flag: {:#x}", c);
        seen |= c;
    }
}

/// INV-TASKSTATE-K2: new(bits).bits() == bits roundtrip.
#[kani::proof]
fn verify_bits_roundtrip() {
    let bits: u32 = kani::any();
    let state = TaskState::new(bits);
    assert_eq!(state.bits(), bits);
}

/// INV-TASKSTATE-K3: contains(flag) is correct after setting and clearing.
#[kani::proof]
fn verify_contains() {
    let bits: u32 = kani::any();
    let flag: u32 = kani::any();
    kani::assume(flag > 0 && flag <= 0x40);

    let with = TaskState::new(bits | flag);
    assert!(with.contains(flag));

    let without = TaskState::new(bits & !flag);
    assert!(!without.contains(flag));
}

/// INV-TASKSTATE-K4: is_running only true when bits == 0.
#[kani::proof]
fn verify_is_running() {
    let bits: u32 = kani::any();
    kani::assume(bits <= 0x100);
    let state = TaskState::new(bits);
    assert_eq!(state.is_running(), bits == 0);
}

/// INV-TASKSTATE-K5: is_sleeping iff INTERRUPTIBLE or UNINTERRUPTIBLE set.
#[kani::proof]
fn verify_is_sleeping() {
    let bits: u32 = kani::any();
    kani::assume(bits <= 0x100);
    let state = TaskState::new(bits);
    let has_int = (bits & TaskState::INTERRUPTIBLE) != 0;
    let has_unint = (bits & TaskState::UNINTERRUPTIBLE) != 0;
    assert_eq!(state.is_sleeping(), has_int || has_unint);
}
