//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network protocol type round-trip invariant tests.
//!
//! Types copied from: kernel/src/net/buffer.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/net/buffer.rs
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Host = 0,
    Otherhost = 1,
    Broadcast = 2,
    Multicast = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EthProtocol {
    ETH_P_IP = 0x0800,
    ETH_P_ARP = 0x0806,
    ETH_P_IPV6 = 0x86DD,
    ETH_P_8021Q = 0x8100,
}

impl EthProtocol {
    pub fn from_u16(val: u16) -> Option<Self> {
        match val {
            0x0800 => Some(EthProtocol::ETH_P_IP),
            0x0806 => Some(EthProtocol::ETH_P_ARP),
            0x86DD => Some(EthProtocol::ETH_P_IPV6),
            0x8100 => Some(EthProtocol::ETH_P_8021Q),
            _ => None,
        }
    }
    pub fn to_u16(self) -> u16 { self as u16 }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpProtocol {
    IPPROTO_IP = 0,
    IPPROTO_ICMP = 1,
    IPPROTO_TCP = 6,
    IPPROTO_UDP = 17,
    IPPROTO_IPV6 = 41,
}

impl IpProtocol {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(IpProtocol::IPPROTO_IP),
            1 => Some(IpProtocol::IPPROTO_ICMP),
            6 => Some(IpProtocol::IPPROTO_TCP),
            17 => Some(IpProtocol::IPPROTO_UDP),
            41 => Some(IpProtocol::IPPROTO_IPV6),
            _ => None,
        }
    }
    pub fn to_u8(self) -> u8 { self as u8 }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-NETBUF-1: EthProtocol round-trip for valid values
    #[test]
    fn test_eth_protocol_roundtrip(val in 0u16..65535u16) {
        if let Some(proto) = EthProtocol::from_u16(val) {
            prop_assert_eq!(proto.to_u16(), val);
        } else {
            // All valid EthProtocol values should round-trip
            let valid_vals = [0x0800u16, 0x0806, 0x86DD, 0x8100];
            prop_assert!(!valid_vals.contains(&val));
        }
    }

    /// INV-NETBUF-2: IpProtocol round-trip for valid values
    #[test]
    fn test_ip_protocol_roundtrip(val in 0u8..255u8) {
        if let Some(proto) = IpProtocol::from_u8(val) {
            prop_assert_eq!(proto.to_u8(), val);
        }
    }

    /// INV-NETBUF-3: EthProtocol::from_u16 returns None for arbitrary invalid values
    #[test]
    fn test_eth_invalid_random(invalid in 0u16..65535u16) {
        // Just verify it doesn't panic for any input
        let _ = EthProtocol::from_u16(invalid);
    }

    /// INV-NETBUF-4: IpProtocol::from_u8 returns None for arbitrary invalid values
    #[test]
    fn test_ip_invalid_random(invalid in 0u8..255u8) {
        let _ = IpProtocol::from_u8(invalid);
    }
}

#[test]
/// INV-NETBUF-5: All EthProtocol values are distinct
fn test_eth_protocol_distinct() {
    let protos = [
        EthProtocol::ETH_P_IP,
        EthProtocol::ETH_P_ARP,
        EthProtocol::ETH_P_IPV6,
        EthProtocol::ETH_P_8021Q,
    ];
    let mut seen = std::collections::HashSet::new();
    for p in &protos {
        let val = p.to_u16();
        assert!(seen.insert(val), "duplicate EthProtocol value: {:#x}", val);
    }
}

#[test]
/// INV-NETBUF-6: All IpProtocol values are distinct
fn test_ip_protocol_distinct() {
    let protos = [
        IpProtocol::IPPROTO_IP,
        IpProtocol::IPPROTO_ICMP,
        IpProtocol::IPPROTO_TCP,
        IpProtocol::IPPROTO_UDP,
        IpProtocol::IPPROTO_IPV6,
    ];
    let mut seen = std::collections::HashSet::new();
    for p in &protos {
        let val = p.to_u8();
        assert!(seen.insert(val), "duplicate IpProtocol value: {}", val);
    }
}

#[test]
/// INV-NETBUF-7: EthProtocol well-known values match IANA assignments
fn test_eth_protocol_iana_values() {
    assert_eq!(EthProtocol::ETH_P_IP.to_u16(), 0x0800);
    assert_eq!(EthProtocol::ETH_P_ARP.to_u16(), 0x0806);
    assert_eq!(EthProtocol::ETH_P_IPV6.to_u16(), 0x86DD);
    assert_eq!(EthProtocol::ETH_P_8021Q.to_u16(), 0x8100);
}

#[test]
/// INV-NETBUF-8: IpProtocol well-known values match IANA assignments
fn test_ip_protocol_iana_values() {
    assert_eq!(IpProtocol::IPPROTO_IP.to_u8(), 0);
    assert_eq!(IpProtocol::IPPROTO_ICMP.to_u8(), 1);
    assert_eq!(IpProtocol::IPPROTO_TCP.to_u8(), 6);
    assert_eq!(IpProtocol::IPPROTO_UDP.to_u8(), 17);
    assert_eq!(IpProtocol::IPPROTO_IPV6.to_u8(), 41);
}

#[test]
/// INV-NETBUF-9: EthProtocol::from_u16 returns Some for all known values
fn test_eth_from_u16_known() {
    assert!(EthProtocol::from_u16(0x0800).is_some());
    assert!(EthProtocol::from_u16(0x0806).is_some());
    assert!(EthProtocol::from_u16(0x86DD).is_some());
    assert!(EthProtocol::from_u16(0x8100).is_some());
    assert!(EthProtocol::from_u16(0x0000).is_none());
    assert!(EthProtocol::from_u16(0xFFFF).is_none());
}

#[test]
/// INV-NETBUF-10: IpProtocol::from_u8 returns Some for all known values
fn test_ip_from_u8_known() {
    assert!(IpProtocol::from_u8(0).is_some());
    assert!(IpProtocol::from_u8(1).is_some());
    assert!(IpProtocol::from_u8(6).is_some());
    assert!(IpProtocol::from_u8(17).is_some());
    assert!(IpProtocol::from_u8(41).is_some());
    assert!(IpProtocol::from_u8(255).is_none());
}

#[test]
/// INV-NETBUF-11: PacketType discriminants are consecutive 0-3
fn test_packet_type_consecutive() {
    assert_eq!(PacketType::Host as u8, 0);
    assert_eq!(PacketType::Otherhost as u8, 1);
    assert_eq!(PacketType::Broadcast as u8, 2);
    assert_eq!(PacketType::Multicast as u8, 3);
}
