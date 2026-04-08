//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Ethernet MAC address classification invariant tests.
//!
//! Types copied from: kernel/src/net/ethernet.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/net/ethernet.rs
// ============================================================================

pub const ETH_ALEN: usize = 6;
pub const ETH_BROADCAST: [u8; ETH_ALEN] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

pub fn eth_is_valid_unicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    if addr.iter().all(|&b| b == 0) {
        return false;
    }
    if addr[0] & 0x01 != 0 {
        return false;
    }
    true
}

pub fn eth_is_multicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    addr[0] & 0x01 != 0
}

pub fn eth_is_broadcast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    *addr == ETH_BROADCAST
}

pub fn eth_addr_eq(a: &[u8; ETH_ALEN], b: &[u8; ETH_ALEN]) -> bool {
    a == b
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-ETH-1: Broadcast is multicast and broadcast, never valid unicast
    #[test]
    fn test_broadcast_classification(_v in 0u8..1u8) {
        prop_assert!(eth_is_broadcast_addr(&ETH_BROADCAST));
        prop_assert!(eth_is_multicast_addr(&ETH_BROADCAST));
        prop_assert!(!eth_is_valid_unicast_addr(&ETH_BROADCAST));
    }

    /// INV-ETH-2: All-zeros is not valid unicast
    #[test]
    fn test_zero_addr(_v in 0u8..1u8) {
        let zero = [0u8; ETH_ALEN];
        prop_assert!(!eth_is_valid_unicast_addr(&zero));
        prop_assert!(!eth_is_multicast_addr(&zero));
        prop_assert!(!eth_is_broadcast_addr(&zero));
    }

    /// INV-ETH-3: Multicast iff bit 0 of byte 0 is set
    #[test]
    fn test_multicast_bit(
        byte0 in 0u8..255u8,
        rest in proptest::array::uniform5(0u8..255u8),
    ) {
        let mut addr = [byte0, rest[0], rest[1], rest[2], rest[3], rest[4]];
        // Clear bit 0 of byte 0 to test control
        addr[0] &= !0x01; // not multicast
        prop_assert!(!eth_is_multicast_addr(&addr));
        addr[0] |= 0x01; // multicast
        prop_assert!(eth_is_multicast_addr(&addr));
    }

    /// INV-ETH-4: Valid unicast implies not multicast and not broadcast
    #[test]
    fn test_unicast_exclusive(
        byte0 in 0u8..255u8,
        bytes in proptest::array::uniform5(1u8..255u8),
    ) {
        // Make byte0 even and non-zero (valid unicast first byte)
        let byte0 = if byte0 == 0 || byte0 & 0x01 != 0 { 2 } else { byte0 };
        let addr = [byte0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4]];
        if eth_is_valid_unicast_addr(&addr) {
            prop_assert!(!eth_is_multicast_addr(&addr));
            prop_assert!(!eth_is_broadcast_addr(&addr));
        }
    }

    /// INV-ETH-5: eth_addr_eq is reflexive and symmetric
    #[test]
    fn test_addr_eq(
        addr in proptest::array::uniform6(0u8..255u8),
    ) {
        let a = addr;
        let b = addr;
        prop_assert!(eth_addr_eq(&a, &b));
        prop_assert!(eth_addr_eq(&b, &a));
    }

    /// INV-ETH-6: eth_addr_eq false for different addresses
    #[test]
    fn test_addr_not_eq(
        addr in proptest::array::uniform6(1u8..255u8),
    ) {
        let mut other = addr;
        other[0] = if addr[0] == 1 { 2 } else { 1 };
        prop_assert!(!eth_addr_eq(&addr, &other));
    }

    /// INV-ETH-7: Random address classification mutual exclusivity
    #[test]
    fn test_classification_exclusive(
        addr in proptest::array::uniform6(0u8..255u8),
    ) {
        let is_bc = eth_is_broadcast_addr(&addr);
        let is_mc = eth_is_multicast_addr(&addr);
        let is_uc = eth_is_valid_unicast_addr(&addr);
        // At most one category should be true
        let count = is_bc as u8 + is_mc as u8 + is_uc as u8;
        prop_assert!(count <= 1);
    }
}
