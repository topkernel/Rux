//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for IPC ID encoding/decoding and permission bitfield.
//!
//! Types copied from: kernel/src/ipc/util.rs

#![cfg(kani)]

pub fn ipc_build_id(index: usize, seq: u32) -> i32 {
    (((index as u32) << 16) | (seq & 0xFFFF)) as i32
}

pub fn ipc_id_to_index(id: i32) -> usize { ((id as u32) >> 16) as usize }

pub fn ipc_id_seq(id: i32) -> u32 { (id as u32) & 0xFFFF }

pub fn ipc_update_mode(old_mode: u16, new_mode: u16) -> u16 {
    (new_mode & 0o777) | (old_mode & !0o777)
}

pub fn owner_bits(mode: u16) -> u16 { (mode >> 6) & 0o7 }
pub fn group_bits(mode: u16) -> u16 { (mode >> 3) & 0o7 }
pub fn other_bits(mode: u16) -> u16 { mode & 0o7 }

/// INV-IPC-K1: IPC ID round-trip: build → extract == original.
#[kani::proof]
fn verify_ipc_id_roundtrip() {
    let index: usize = kani::any();
    let seq: u32 = kani::any();
    kani::assume(index < 65536);
    kani::assume(seq < 65536);
    let id = ipc_build_id(index, seq);
    assert_eq!(ipc_id_to_index(id), index);
    assert_eq!(ipc_id_seq(id), seq);
}

/// INV-IPC-K2: IPC ID seq truncates to 16 bits.
#[kani::proof]
fn verify_ipc_id_seq_truncates() {
    let seq: u32 = kani::any();
    let id = ipc_build_id(0, seq);
    assert_eq!(ipc_id_seq(id), seq & 0xFFFF);
}

/// INV-IPC-K3: high index produces negative i32.
#[kani::proof]
fn verify_ipc_id_negative() {
    let index: usize = kani::any();
    let seq: u32 = kani::any();
    kani::assume(index >= 32768 && index < 65536);
    kani::assume(seq < 65536);
    let id = ipc_build_id(index, seq);
    assert!(id < 0);
    assert_eq!(ipc_id_to_index(id), index);
}

/// INV-IPC-K4: update_mode preserves non-permission bits from old, permission from new.
#[kani::proof]
fn verify_update_mode() {
    let old_mode: u16 = kani::any();
    let new_mode: u16 = kani::any();
    kani::assume(new_mode < 0o7777);
    let result = ipc_update_mode(old_mode, new_mode);
    assert_eq!(result & 0o777, new_mode & 0o777);
    assert_eq!(result & !0o777, old_mode & !0o777);
}

/// INV-IPC-K5: permission bits extraction and reassembly.
#[kani::proof]
fn verify_perm_bits_extraction() {
    let mode: u16 = kani::any();
    kani::assume(mode < 0o777);
    let ow = owner_bits(mode);
    let gr = group_bits(mode);
    let ot = other_bits(mode);
    assert!(ow <= 0o7 && gr <= 0o7 && ot <= 0o7);
    assert_eq!((ow << 6) | (gr << 3) | ot, mode);
}
