//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! POSIX.1e Capability bitmask invariant tests.
//!
//! Types copied from: kernel/src/security/capability.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/security/capability.rs
// ============================================================================

/// Capability bitmask — 41 capabilities fit in a single u64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap(pub u64);

/// Bits 0..=40 are valid (41 capabilities).
pub const CAP_VALID_MASK: u64 = (1u64 << 41) - 1;

pub const CAP_CHOWN: u32 = 0;
pub const CAP_DAC_OVERRIDE: u32 = 1;
pub const CAP_DAC_READ_SEARCH: u32 = 2;
pub const CAP_FOWNER: u32 = 3;
pub const CAP_FSETID: u32 = 4;
pub const CAP_KILL: u32 = 5;
pub const CAP_SETGID: u32 = 6;
pub const CAP_SETUID: u32 = 7;
pub const CAP_SETPCAP: u32 = 8;
pub const CAP_LINUX_IMMUTABLE: u32 = 9;
pub const CAP_NET_BIND_SERVICE: u32 = 10;
pub const CAP_NET_BROADCAST: u32 = 11;
pub const CAP_NET_ADMIN: u32 = 12;
pub const CAP_NET_RAW: u32 = 13;
pub const CAP_IPC_LOCK: u32 = 14;
pub const CAP_IPC_OWNER: u32 = 15;
pub const CAP_SYS_MODULE: u32 = 16;
pub const CAP_SYS_RAWIO: u32 = 17;
pub const CAP_SYS_CHROOT: u32 = 18;
pub const CAP_SYS_PTRACE: u32 = 19;
pub const CAP_SYS_PACCT: u32 = 20;
pub const CAP_SYS_ADMIN: u32 = 21;
pub const CAP_SYS_BOOT: u32 = 22;
pub const CAP_SYS_NICE: u32 = 23;
pub const CAP_SYS_RESOURCE: u32 = 24;
pub const CAP_SYS_TIME: u32 = 25;
pub const CAP_SYS_TTY_CONFIG: u32 = 26;
pub const CAP_MKNOD: u32 = 27;
pub const CAP_LEASE: u32 = 28;
pub const CAP_AUDIT_WRITE: u32 = 29;
pub const CAP_AUDIT_CONTROL: u32 = 30;
pub const CAP_SETFCAP: u32 = 31;
pub const CAP_MAC_OVERRIDE: u32 = 32;
pub const CAP_MAC_ADMIN: u32 = 33;
pub const CAP_SYSLOG: u32 = 34;
pub const CAP_WAKE_ALARM: u32 = 35;
pub const CAP_BLOCK_SUSPEND: u32 = 36;
pub const CAP_AUDIT_READ: u32 = 37;
pub const CAP_PERFMON: u32 = 38;
pub const CAP_BPF: u32 = 39;
pub const CAP_CHECKPOINT_RESTORE: u32 = 40;

pub const CAP_LAST_CAP: u32 = CAP_CHECKPOINT_RESTORE;

impl Cap {
    pub const EMPTY: Cap = Cap(0);
    pub const FULL: Cap = Cap(CAP_VALID_MASK);

    pub fn new(mask: u64) -> Self {
        Cap(mask & CAP_VALID_MASK)
    }

    pub fn has(&self, cap: u32) -> bool {
        if cap > CAP_LAST_CAP {
            return false;
        }
        (self.0 >> cap) & 1 == 1
    }

    pub fn set(&mut self, cap: u32) {
        if cap <= CAP_LAST_CAP {
            self.0 |= 1u64 << cap;
        }
    }

    pub fn clear(&mut self, cap: u32) {
        if cap <= CAP_LAST_CAP {
            self.0 &= !(1u64 << cap);
        }
    }

    pub fn intersect(&self, other: Cap) -> Cap {
        Cap(self.0 & other.0)
    }

    pub fn union(&self, other: Cap) -> Cap {
        Cap(self.0 | other.0)
    }

    pub fn xor(&self, other: Cap) -> Cap {
        Cap(self.0 ^ other.0)
    }

    pub fn complement(&self) -> Cap {
        Cap(CAP_VALID_MASK & !self.0)
    }

    pub fn is_subset_of(&self, other: Cap) -> bool {
        (self.0 & !other.0) == 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }

    pub fn lo(&self) -> u32 {
        self.0 as u32
    }

    pub fn hi(&self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub fn from_halves(lo: u32, hi: u32) -> Self {
        Cap::new(((hi as u64) << 32) | (lo as u64))
    }
}

/// All CAP_* constants for constant validation.
const ALL_CAPS: &[u32] = &[
    CAP_CHOWN, CAP_DAC_OVERRIDE, CAP_DAC_READ_SEARCH, CAP_FOWNER, CAP_FSETID,
    CAP_KILL, CAP_SETGID, CAP_SETUID, CAP_SETPCAP, CAP_LINUX_IMMUTABLE,
    CAP_NET_BIND_SERVICE, CAP_NET_BROADCAST, CAP_NET_ADMIN, CAP_NET_RAW,
    CAP_IPC_LOCK, CAP_IPC_OWNER, CAP_SYS_MODULE, CAP_SYS_RAWIO, CAP_SYS_CHROOT,
    CAP_SYS_PTRACE, CAP_SYS_PACCT, CAP_SYS_ADMIN, CAP_SYS_BOOT, CAP_SYS_NICE,
    CAP_SYS_RESOURCE, CAP_SYS_TIME, CAP_SYS_TTY_CONFIG, CAP_MKNOD, CAP_LEASE,
    CAP_AUDIT_WRITE, CAP_AUDIT_CONTROL, CAP_SETFCAP, CAP_MAC_OVERRIDE,
    CAP_MAC_ADMIN, CAP_SYSLOG, CAP_WAKE_ALARM, CAP_BLOCK_SUSPEND,
    CAP_AUDIT_READ, CAP_PERFMON, CAP_BPF, CAP_CHECKPOINT_RESTORE,
];

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-CAP-1: new(mask) masks to valid 41 bits
    #[test]
    fn test_new_masks(mask in 0u64..u64::MAX) {
        let c = Cap::new(mask);
        prop_assert_eq!(c.bits(), mask & CAP_VALID_MASK);
        prop_assert!(c.bits() <= CAP_VALID_MASK);
    }

    /// INV-CAP-2: set(x); has(x) for valid cap 0..=40
    #[test]
    fn test_set_has(cap in 0u32..41u32) {
        let mut c = Cap::EMPTY;
        c.set(cap);
        prop_assert!(c.has(cap));
    }

    /// INV-CAP-3: set(x); clear(x); !has(x) roundtrip
    #[test]
    fn test_set_clear_roundtrip(cap in 0u32..41u32) {
        let mut c = Cap::EMPTY;
        c.set(cap);
        prop_assert!(c.has(cap));
        c.clear(cap);
        prop_assert!(!c.has(cap));
    }

    /// INV-CAP-4: intersect = bitwise AND
    #[test]
    fn test_intersect(
        a in 0u64..CAP_VALID_MASK,
        b in 0u64..CAP_VALID_MASK,
    ) {
        let ca = Cap::new(a);
        let cb = Cap::new(b);
        prop_assert_eq!(ca.intersect(cb).bits(), a & b);
    }

    /// INV-CAP-5: union = bitwise OR
    #[test]
    fn test_union(
        a in 0u64..CAP_VALID_MASK,
        b in 0u64..CAP_VALID_MASK,
    ) {
        let ca = Cap::new(a);
        let cb = Cap::new(b);
        prop_assert_eq!(ca.union(cb).bits(), a | b);
    }

    /// INV-CAP-6: xor = bitwise XOR
    #[test]
    fn test_xor(
        a in 0u64..CAP_VALID_MASK,
        b in 0u64..CAP_VALID_MASK,
    ) {
        let ca = Cap::new(a);
        let cb = Cap::new(b);
        prop_assert_eq!(ca.xor(cb).bits(), a ^ b);
    }

    /// INV-CAP-7: complement(complement(c)) == c (within valid mask)
    #[test]
    fn test_complement_involution(mask in 0u64..CAP_VALID_MASK) {
        let c = Cap::new(mask);
        prop_assert_eq!(c.complement().complement(), c);
    }

    /// INV-CAP-8: FULL is subset of FULL, EMPTY is subset of any
    #[test]
    fn test_subset_trivial(mask in 0u64..CAP_VALID_MASK) {
        let full = Cap::FULL;
        let empty = Cap::EMPTY;
        let c = Cap::new(mask);
        prop_assert!(full.is_subset_of(full));
        prop_assert!(empty.is_subset_of(c));
    }

    /// INV-CAP-9: is_subset_of is reflexive
    #[test]
    fn test_subset_reflexive(mask in 0u64..CAP_VALID_MASK) {
        let c = Cap::new(mask);
        prop_assert!(c.is_subset_of(c));
    }

    /// INV-CAP-10: has(cap > 40) always false
    #[test]
    fn test_has_invalid_cap(cap in 41u32..100u32) {
        let mut c = Cap::EMPTY;
        c.set(cap); // should be no-op
        let full = Cap::FULL;
        prop_assert!(!c.has(cap));
        prop_assert!(!full.has(cap));
    }

    /// INV-CAP-11: De Morgan's law
    #[test]
    fn test_de_morgan(
        a in 0u64..CAP_VALID_MASK,
        b in 0u64..CAP_VALID_MASK,
    ) {
        let ca = Cap::new(a);
        let cb = Cap::new(b);
        let lhs = ca.intersect(cb).complement();
        let rhs = ca.complement().union(cb.complement());
        prop_assert_eq!(lhs, rhs);
    }

    /// INV-CAP-12: lo/hi/from_halves roundtrip
    #[test]
    fn test_halves_roundtrip(lo in 0u32..u32::MAX, hi in 0u32..u32::MAX) {
        let c = Cap::from_halves(lo, hi);
        // hi is masked to 9 bits (41-32=9 valid high bits)
        prop_assert_eq!(c.lo(), lo);
        prop_assert_eq!(c.hi(), hi & ((1u32 << 9) - 1));
    }

    /// INV-CAP-13: is_empty consistent with bits
    #[test]
    fn test_is_empty(mask in 0u64..CAP_VALID_MASK) {
        let c = Cap::new(mask);
        prop_assert_eq!(c.is_empty(), mask == 0);
    }

    /// INV-CAP-14: EMPTY and FULL constants
    #[test]
    fn test_empty_full(_v in 0u8..1u8) {
        prop_assert!(Cap::EMPTY.is_empty());
        prop_assert!(!Cap::FULL.is_empty());
        prop_assert_eq!(Cap::FULL.bits(), CAP_VALID_MASK);
    }

    /// INV-CAP-15: complement of EMPTY is FULL
    #[test]
    fn test_complement_empty_full(_v in 0u8..1u8) {
        prop_assert_eq!(Cap::EMPTY.complement(), Cap::FULL);
        prop_assert_eq!(Cap::FULL.complement(), Cap::EMPTY);
    }

    /// INV-CAP-16: from_halves(new(lo, hi).lo(), new(lo, hi).hi()) roundtrip
    #[test]
    fn test_from_halves_new_roundtrip(lo in 0u32..u32::MAX, hi in 0u32..u32::MAX) {
        let c = Cap::from_halves(lo, hi);
        let c2 = Cap::new(c.bits());
        prop_assert_eq!(c, c2);
    }
}

#[test]
/// INV-CAP-17: All 41 CAP_* constants are distinct
fn test_all_caps_distinct() {
    let mut seen = std::collections::HashSet::new();
    for &cap in ALL_CAPS {
        assert!(seen.insert(cap), "duplicate CAP_ constant: {}", cap);
    }
    assert_eq!(ALL_CAPS.len(), 41);
}

#[test]
/// INV-CAP-18: All CAP_* constants in range 0..=40
fn test_all_caps_in_range() {
    for &cap in ALL_CAPS {
        assert!(cap <= 40, "CAP_ constant out of range: {}", cap);
    }
}
