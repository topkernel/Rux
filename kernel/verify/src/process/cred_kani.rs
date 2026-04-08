//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for Cred (process credentials) initialization.
//!
//! Types copied from: kernel/src/process/task.rs

#![cfg(kani)]

pub const CAP_VALID_MASK: u64 = (1u64 << 41) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap(u64);

impl Cap {
    pub const EMPTY: Cap = Cap(0);
    pub const FULL: Cap = Cap(CAP_VALID_MASK);
    pub fn is_empty(&self) -> bool { self.0 == 0 }
    pub fn is_full(&self) -> bool { self.0 == CAP_VALID_MASK }
}

pub struct Cred {
    pub uid: u32, pub gid: u32,
    pub euid: u32, pub egid: u32,
    pub cap_effective: Cap,
    pub cap_bounding: Cap,
}

impl Cred {
    pub fn new_init() -> Self {
        Self {
            uid: 0, gid: 0, euid: 0, egid: 0,
            cap_effective: Cap::FULL,
            cap_bounding: Cap::FULL,
        }
    }

    pub fn new_user(uid: u32, gid: u32) -> Self {
        Self {
            uid, gid, euid: uid, egid: gid,
            cap_effective: Cap::EMPTY,
            cap_bounding: Cap::FULL,
        }
    }
}

/// INV-CRED-K1: init has all-zero IDs.
#[kani::proof]
fn verify_init_zero_ids() {
    let c = Cred::new_init();
    assert_eq!(c.uid, 0);
    assert_eq!(c.gid, 0);
    assert_eq!(c.euid, 0);
    assert_eq!(c.egid, 0);
}

/// INV-CRED-K2: init has full capabilities.
#[kani::proof]
fn verify_init_full_caps() {
    let c = Cred::new_init();
    assert!(c.cap_effective.is_full());
    assert!(c.cap_bounding.is_full());
}

/// INV-CRED-K3: user credentials have matching IDs.
#[kani::proof]
fn verify_user_id_match() {
    let uid: u32 = kani::any();
    let gid: u32 = kani::any();
    let c = Cred::new_user(uid, gid);
    assert_eq!(c.uid, uid);
    assert_eq!(c.gid, gid);
    assert_eq!(c.euid, uid);
    assert_eq!(c.egid, gid);
}

/// INV-CRED-K4: user credentials have no effective caps but full bounding.
#[kani::proof]
fn verify_user_no_caps() {
    let uid: u32 = kani::any();
    let gid: u32 = kani::any();
    kani::assume(uid > 0);
    let c = Cred::new_user(uid, gid);
    assert!(c.cap_effective.is_empty());
    assert!(c.cap_bounding.is_full());
}
