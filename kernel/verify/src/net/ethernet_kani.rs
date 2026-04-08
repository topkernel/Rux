//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for Ethernet MAC address classification.
//!
//! Types copied from: kernel/src/net/ethernet.rs

#![cfg(kani)]

pub const ETH_ALEN: usize = 6;
pub const ETH_BROADCAST: [u8; ETH_ALEN] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

pub fn eth_is_valid_unicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    if addr.iter().all(|&b| b == 0) { return false; }
    if addr[0] & 0x01 != 0 { return false; }
    true
}

pub fn eth_is_multicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    addr[0] & 0x01 != 0
}

pub fn eth_is_broadcast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    *addr == ETH_BROADCAST
}

/// INV-ETH-K1: broadcast is multicast, not valid unicast.
#[kani::proof]
fn verify_broadcast_classification() {
    assert!(eth_is_broadcast_addr(&ETH_BROADCAST));
    assert!(eth_is_multicast_addr(&ETH_BROADCAST));
    assert!(!eth_is_valid_unicast_addr(&ETH_BROADCAST));
}

/// INV-ETH-K2: all-zeros is not unicast, multicast, or broadcast.
#[kani::proof]
fn verify_zero_addr() {
    let zero = [0u8; ETH_ALEN];
    assert!(!eth_is_valid_unicast_addr(&zero));
    assert!(!eth_is_multicast_addr(&zero));
    assert!(!eth_is_broadcast_addr(&zero));
}

/// INV-ETH-K3: multicast iff bit 0 of byte 0 is set.
#[kani::proof]
fn verify_multicast_bit() {
    let byte0: u8 = kani::any();
    let b1: u8 = kani::any();
    let b2: u8 = kani::any();
    let b3: u8 = kani::any();
    let b4: u8 = kani::any();
    let b5: u8 = kani::any();
    let mut addr = [byte0, b1, b2, b3, b4, b5];
    addr[0] &= !0x01;
    assert!(!eth_is_multicast_addr(&addr));
    addr[0] |= 0x01;
    assert!(eth_is_multicast_addr(&addr));
}

/// INV-ETH-K4: classification is mutually exclusive.
#[kani::proof]
fn verify_classification_exclusive() {
    let a0: u8 = kani::any();
    let a1: u8 = kani::any();
    let a2: u8 = kani::any();
    let a3: u8 = kani::any();
    let a4: u8 = kani::any();
    let a5: u8 = kani::any();
    let addr = [a0, a1, a2, a3, a4, a5];
    let is_bc = eth_is_broadcast_addr(&addr) as u8;
    let is_mc = eth_is_multicast_addr(&addr) as u8;
    let is_uc = eth_is_valid_unicast_addr(&addr) as u8;
    assert!(is_bc + is_mc + is_uc <= 1);
}
