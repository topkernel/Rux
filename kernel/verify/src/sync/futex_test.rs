//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Futex key, hash, flags, and bitset invariant tests.
//!
//! Types copied from: kernel/src/sync/futex.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/sync/futex.rs
// ============================================================================

pub const FUTEX_WAIT: i32 = 0;
pub const FUTEX_WAKE: i32 = 1;
pub const FUTEX_FD: i32 = 2;
pub const FUTEX_REQUEUE: i32 = 3;
pub const FUTEX_CMP_REQUEUE: i32 = 4;
pub const FUTEX_WAKE_OP: i32 = 5;
pub const FUTEX_LOCK_PI: i32 = 6;
pub const FUTEX_UNLOCK_PI: i32 = 7;
pub const FUTEX_TRYLOCK_PI: i32 = 8;
pub const FUTEX_WAIT_BITSET: i32 = 9;
pub const FUTEX_WAKE_BITSET: i32 = 10;
pub const FUTEX_WAIT_REQUEUE_PI: i32 = 11;
pub const FUTEX_CMP_REQUEUE_PI: i32 = 12;
pub const FUTEX_LOCK_PI2: i32 = 13;

pub const FUTEX_PRIVATE_FLAG: i32 = 128;
pub const FUTEX_CLOCK_REALTIME: i32 = 256;
pub const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

pub const FUTEX_BITSET_MATCH_ANY: u32 = 0xffffffff;

pub const FLAGS_SHARED: u32 = 0x0010;
pub const FLAGS_CLOCKRT: u32 = 0x0020;

pub const HASH_SIZE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FutexKey {
    pub uaddr: usize,
    pub pid: u32,
    pub flags: u32,
}

impl FutexKey {
    pub fn new(uaddr: usize, pid: u32, flags: u32) -> Self {
        Self { uaddr, pid, flags }
    }

    pub fn matches(&self, other: &FutexKey) -> bool {
        if !(self.flags & FLAGS_SHARED != 0) {
            self.uaddr == other.uaddr && self.pid == other.pid
        } else {
            self.uaddr == other.uaddr
        }
    }
}

pub fn futex_hash(key: &FutexKey) -> usize {
    let hash = key.uaddr.wrapping_add(key.pid as usize);
    hash % HASH_SIZE
}

pub fn futex_to_flags(op: u32) -> u32 {
    let mut flags = 0u32;
    if (op & FUTEX_PRIVATE_FLAG as u32) == 0 {
        flags |= FLAGS_SHARED;
    }
    if (op & FUTEX_CLOCK_REALTIME as u32) != 0 {
        flags |= FLAGS_CLOCKRT;
    }
    flags
}

pub fn bitset_matches(waiter_bitset: u32, wake_bitset: u32) -> bool {
    (waiter_bitset & wake_bitset) != 0
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-FX-1: Private key matches only when uaddr and pid match
    #[test]
    fn test_private_key_match(
        uaddr in 0usize..0xFFFFusize,
        pid in 1u32..1000u32,
    ) {
        let k1 = FutexKey::new(uaddr, pid, 0); // no FLAGS_SHARED
        let k2 = FutexKey::new(uaddr, pid, 0);
        prop_assert!(k1.matches(&k2));
    }

    /// INV-FX-2: Private key rejects different pid
    #[test]
    fn test_private_key_pid_mismatch(
        uaddr in 0usize..0xFFFFusize,
        pid1 in 1u32..500u32,
        pid2 in 501u32..1000u32,
    ) {
        let k1 = FutexKey::new(uaddr, pid1, 0);
        let k2 = FutexKey::new(uaddr, pid2, 0);
        prop_assert!(!k1.matches(&k2));
    }

    /// INV-FX-3: Private key rejects different uaddr
    #[test]
    fn test_private_key_uaddr_mismatch(
        uaddr1 in 0usize..0x7FFFusize,
        uaddr2 in 0x8000usize..0xFFFFusize,
        pid in 1u32..1000u32,
    ) {
        let k1 = FutexKey::new(uaddr1, pid, 0);
        let k2 = FutexKey::new(uaddr2, pid, 0);
        prop_assert!(!k1.matches(&k2));
    }

    /// INV-FX-4: Shared key matches on uaddr only (ignores pid)
    #[test]
    fn test_shared_key_match(
        uaddr in 0usize..0xFFFFusize,
        pid1 in 1u32..500u32,
        pid2 in 501u32..1000u32,
    ) {
        let k1 = FutexKey::new(uaddr, pid1, FLAGS_SHARED);
        let k2 = FutexKey::new(uaddr, pid2, FLAGS_SHARED);
        prop_assert!(k1.matches(&k2));
    }

    /// INV-FX-5: futex_hash is within [0, HASH_SIZE)
    #[test]
    fn test_hash_in_range(
        uaddr in 0usize..usize::MAX,
        pid in 0u32..u32::MAX,
    ) {
        let key = FutexKey::new(uaddr, pid, 0);
        let h = futex_hash(&key);
        prop_assert!(h < HASH_SIZE);
    }

    /// INV-FX-6: Same uaddr with pid difference 1 gives different hashes
    #[test]
    fn test_hash_different_pids(
        uaddr in 0usize..10000usize,
        pid in 0u32..1000u32,
    ) {
        let k1 = FutexKey::new(uaddr, pid, 0);
        let k2 = FutexKey::new(uaddr, pid + 1, 0);
        let h1 = futex_hash(&k1);
        let h2 = futex_hash(&k2);
        // (uaddr + pid) % 64 vs (uaddr + pid + 1) % 64 — always differ by 1 mod 64
        prop_assert_ne!(h1, h2);
    }

    /// INV-FX-7: futex_to_flags sets SHARED when PRIVATE not set
    #[test]
    fn test_to_flags_default_shared(
        op in 0u32..15u32,
    ) {
        let flags = futex_to_flags(op);
        prop_assert!(flags & FLAGS_SHARED != 0);
    }

    /// INV-FX-8: futex_to_flags clears SHARED when PRIVATE is set
    #[test]
    fn test_to_flags_private(base_op in 0u32..15u32) {
        let op = base_op | (FUTEX_PRIVATE_FLAG as u32);
        let flags = futex_to_flags(op);
        prop_assert!(flags & FLAGS_SHARED == 0);
    }

    /// INV-FX-9: futex_to_flags sets CLOCKRT when CLOCK_REALTIME is set
    #[test]
    fn test_to_flags_clockrt(base_op in 0u32..15u32) {
        let op = base_op | (FUTEX_CLOCK_REALTIME as u32);
        let flags = futex_to_flags(op);
        prop_assert!(flags & FLAGS_CLOCKRT != 0);
    }

    /// INV-FX-10: FUTEX_CMD_MASK strips PRIVATE and CLOCK_REALTIME
    #[test]
    fn test_cmd_mask(op in 0i32..i32::MAX) {
        let masked = op & FUTEX_CMD_MASK;
        prop_assert!(masked & FUTEX_PRIVATE_FLAG == 0);
        prop_assert!(masked & FUTEX_CLOCK_REALTIME == 0);
    }

    /// INV-FX-11: bitset_matches with MATCH_ANY always returns true
    #[test]
    fn test_bitset_match_any(bitset in 0u32..u32::MAX) {
        prop_assert!(bitset_matches(bitset, FUTEX_BITSET_MATCH_ANY));
    }

    /// INV-FX-12: bitset_matches with 0 always returns false
    #[test]
    fn test_bitset_zero(bitset in 0u32..u32::MAX) {
        prop_assert!(!bitset_matches(bitset, 0));
    }

    /// INV-FX-13: bitset_matches is commutative
    #[test]
    fn test_bitset_commutative(
        a in 0u32..u32::MAX,
        b in 0u32..u32::MAX,
    ) {
        prop_assert_eq!(bitset_matches(a, b), bitset_matches(b, a));
    }

    /// INV-FX-14: All FUTEX_* opcode constants are distinct
    #[test]
    fn test_opcodes_distinct(_v in 0u8..1u8) {
        let ops = [
            FUTEX_WAIT, FUTEX_WAKE, FUTEX_FD, FUTEX_REQUEUE,
            FUTEX_CMP_REQUEUE, FUTEX_WAKE_OP, FUTEX_LOCK_PI,
            FUTEX_UNLOCK_PI, FUTEX_TRYLOCK_PI, FUTEX_WAIT_BITSET,
            FUTEX_WAKE_BITSET, FUTEX_WAIT_REQUEUE_PI,
            FUTEX_CMP_REQUEUE_PI, FUTEX_LOCK_PI2,
        ];
        let mut seen = std::collections::HashSet::new();
        for &op in &ops {
            prop_assert!(seen.insert(op), "duplicate opcode: {}", op);
        }
    }

    /// INV-FX-15: Reflexivity: key matches itself
    #[test]
    fn test_key_reflexive(
        uaddr in 0usize..usize::MAX,
        pid in 0u32..u32::MAX,
        flags in 0u32..0xFFFFu32,
    ) {
        let k = FutexKey::new(uaddr, pid, flags);
        prop_assert!(k.matches(&k));
    }

    /// INV-FX-16: Symmetry: k1.matches(k2) == k2.matches(k1)
    #[test]
    fn test_key_symmetric(
        uaddr1 in 0usize..usize::MAX,
        uaddr2 in 0usize..usize::MAX,
        pid1 in 0u32..u32::MAX,
        pid2 in 0u32..u32::MAX,
        flags in 0u32..0xFFFFu32,
    ) {
        let k1 = FutexKey::new(uaddr1, pid1, flags);
        let k2 = FutexKey::new(uaddr2, pid2, flags);
        prop_assert_eq!(k1.matches(&k2), k2.matches(&k1));
    }
}
