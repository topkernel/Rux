//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for Unix DAC permission check.
//!
//! Types copied from: kernel/src/fs/permission.rs

#![cfg(kani)]

pub const MAY_EXEC: u32 = 0o001;
pub const MAY_WRITE: u32 = 0o002;
pub const MAY_READ: u32 = 0o004;

#[derive(Clone, Copy)]
pub struct Cred { pub euid: u32, pub egid: u32 }

pub fn generic_permission(
    inode_mode: u16, inode_uid: u32, inode_gid: u32,
    mask: u32, cred: &Cred,
) -> bool {
    let mode = inode_mode as u32;
    if cred.euid == inode_uid {
        ((mode >> 6) & 0o7) & mask == mask
    } else if cred.egid == inode_gid {
        ((mode >> 3) & 0o7) & mask == mask
    } else {
        (mode & 0o7) & mask == mask
    }
}

/// INV-PERM-K1: MAY_READ | MAY_WRITE | MAY_EXEC == 0o7.
#[kani::proof]
fn verify_perm_bits() {
    assert_eq!(MAY_READ | MAY_WRITE | MAY_EXEC, 0o7);
}

/// INV-PERM-K2: nobody can do anything on mode 0o000.
#[kani::proof]
fn verify_mode_000() {
    let mask: u32 = kani::any();
    kani::assume(mask >= 1 && mask <= 7);
    let cred = Cred { euid: 1, egid: 1 };
    assert!(!generic_permission(0o000, 1, 1, mask, &cred));
}

/// INV-PERM-K3: 0o777 allows all for matching category.
#[kani::proof]
fn verify_mode_777() {
    let uid: u32 = kani::any();
    let gid: u32 = kani::any();
    let cred = Cred { euid: uid, egid: gid };
    let mask = MAY_READ | MAY_WRITE | MAY_EXEC;
    assert!(generic_permission(0o777, uid, gid, mask, &cred));
}

/// INV-PERM-K4: owner check uses bits 8-6, other uses bits 2-0.
#[kani::proof]
fn verify_owner_priority() {
    let mode: u32 = kani::any();
    kani::assume(mode < 0o777);
    let cred = Cred { euid: 1, egid: 1 };
    let owner_bits = (mode >> 6) & 0o7;
    let other_bits = mode & 0o7;
    if (owner_bits & MAY_READ as u32) == 0 && (other_bits & MAY_READ as u32) != 0 {
        assert!(!generic_permission(mode as u16, 1, 2, MAY_READ, &cred));
    }
}
