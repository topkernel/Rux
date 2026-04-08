//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for SchedClassId ordering and enqueue flags.
//!
//! Types copied from: kernel/src/sched/class.rs

#![cfg(kani)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SchedClassId {
    Stop = 0, Deadline = 1, Rt = 2, Fair = 3, Idle = 4,
}

pub const ENQUEUE_WAKEUP: i32 = 0x0001;
pub const ENQUEUE_HEAD: i32 = 0x00010000;
pub const DEQUEUE_SLEEP: i32 = 0x0001;
pub const DEQUEUE_SAVE: i32 = 0x0002;

/// INV-SCHEDCLASS-K1: SchedClassId discriminants are 0-4.
#[kani::proof]
fn verify_discriminants() {
    assert_eq!(SchedClassId::Stop as u8, 0);
    assert_eq!(SchedClassId::Deadline as u8, 1);
    assert_eq!(SchedClassId::Rt as u8, 2);
    assert_eq!(SchedClassId::Fair as u8, 3);
    assert_eq!(SchedClassId::Idle as u8, 4);
}

/// INV-SCHEDCLASS-K2: ordering — Stop < Deadline < Rt < Fair < Idle.
#[kani::proof]
fn verify_ordering() {
    let a: u8 = kani::any();
    let b: u8 = kani::any();
    kani::assume(a < 5 && b < 5);
    let ids = [
        SchedClassId::Stop, SchedClassId::Deadline, SchedClassId::Rt,
        SchedClassId::Fair, SchedClassId::Idle,
    ];
    let id_a = ids[a as usize];
    let id_b = ids[b as usize];
    assert_eq!(a < b, id_a < id_b);
}

/// INV-SCHEDCLASS-K3: enqueue/dequeue flags are distinct powers of 2.
#[kani::proof]
fn verify_flags_distinct() {
    let flags = [ENQUEUE_WAKEUP, ENQUEUE_HEAD, DEQUEUE_SLEEP, DEQUEUE_SAVE];
    let mut seen = 0i32;
    for &f in &flags {
        assert!(f > 0 && (f & (f - 1)) == 0);
        assert_eq!(seen & f, 0);
        seen |= f;
    }
}
