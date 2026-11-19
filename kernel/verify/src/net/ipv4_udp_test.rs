//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 header and UDP header invariant tests.
//!
//! Types copied from: kernel/src/net/ipv4/mod.rs, kernel/src/net/udp.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/net/ipv4/mod.rs
// ============================================================================

pub const IPHDR_LEN: usize = 20;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IpHdr {
    pub version_ihl: u8,
    pub tos: u8,
    pub tot_len: u16,
    pub id: u16,
    pub frag_off: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub check: u16,
    pub saddr: u32,
    pub daddr: u32,
}

impl IpHdr {
    pub fn version(&self) -> u8 {
        self.version_ihl >> 4
    }

    pub fn ihl(&self) -> u8 {
        self.version_ihl & 0x0F
    }

    pub fn header_len(&self) -> usize {
        (self.ihl() as usize) * 4
    }
}

// ============================================================================
// Copied types from kernel/src/net/udp.rs
// ============================================================================

pub type UdpPort = u16;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UdpHdr {
    pub source: UdpPort,
    pub dest: UdpPort,
    pub len: u16,
    pub check: u16,
}

impl UdpHdr {
    pub fn source(&self) -> UdpPort {
        u16::from_be(self.source)
    }

    pub fn dest(&self) -> UdpPort {
        u16::from_be(self.dest)
    }

    pub fn len(&self) -> u16 {
        u16::from_be(self.len)
    }

    pub fn check(&self) -> u16 {
        u16::from_be(self.check)
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-IP-1: Version is always 4 (for valid IPv4 header)
    #[test]
    fn test_ip_version(ihl in 5u8..15u8) {
        let hdr = IpHdr {
            version_ihl: (4 << 4) | ihl,
            ..Default::default()
        };
        prop_assert_eq!(hdr.version(), 4);
    }

    /// INV-IP-2: IHL >= 5 (minimum 20 bytes)
    #[test]
    fn test_ip_ihl(ihl in 5u8..15u8) {
        let hdr = IpHdr {
            version_ihl: (4 << 4) | ihl,
            ..Default::default()
        };
        prop_assert!(hdr.ihl() >= 5);
        prop_assert_eq!(hdr.header_len(), (ihl as usize) * 4);
    }

    /// INV-IP-3: Header length from IHL matches IPHDR_LEN for IHL=5
    #[test]
    fn test_ip_header_len_default(_v in 0u8..1u8) {
        let hdr = IpHdr {
            version_ihl: (4 << 4) | 5,
            ..Default::default()
        };
        prop_assert_eq!(hdr.header_len(), IPHDR_LEN);
    }

    /// INV-UDP-1: Big-endian field accessors roundtrip
    #[test]
    fn test_udp_port_roundtrip(
        src in 0u16..65535u16,
        dst in 0u16..65535u16,
        length in 8u16..65535u16,
    ) {
        let hdr = UdpHdr {
            source: u16::from_be(src),
            dest: u16::from_be(dst),
            len: u16::from_be(length),
            check: 0,
        };
        prop_assert_eq!(hdr.source(), src);
        prop_assert_eq!(hdr.dest(), dst);
        prop_assert_eq!(hdr.len(), length);
    }

    /// INV-UDP-2: Zero port roundtrip
    #[test]
    fn test_udp_zero_port(_v in 0u8..1u8) {
        let hdr = UdpHdr::default();
        prop_assert_eq!(hdr.source(), 0);
        prop_assert_eq!(hdr.dest(), 0);
        prop_assert_eq!(hdr.len(), 0);
    }

    /// INV-UDP-3: Default UDP header has all zeros
    #[test]
    fn test_udp_default(_v in 0u8..1u8) {
        let hdr = UdpHdr::default();
        prop_assert_eq!(hdr.source(), 0);
        prop_assert_eq!(hdr.dest(), 0);
        prop_assert_eq!(hdr.len(), 0);
        prop_assert_eq!(hdr.check(), 0);
    }

    /// INV-IP-4: Protocol field
    #[test]
    fn test_ip_protocol(proto in 0u8..255u8) {
        let hdr = IpHdr {
            protocol: proto,
            ..Default::default()
        };
        prop_assert_eq!(hdr.protocol, proto);
    }

    /// INV-IP-5: Source/dest address
    #[test]
    fn test_ip_addrs(
        saddr in 0u32..u32::MAX,
        daddr in 0u32..u32::MAX,
    ) {
        let hdr = IpHdr {
            saddr,
            daddr,
            ..Default::default()
        };
        prop_assert_eq!(hdr.saddr, saddr);
        prop_assert_eq!(hdr.daddr, daddr);
    }

    /// INV-IP-6: TTL field
    #[test]
    fn test_ip_ttl(ttl in 0u8..255u8) {
        let hdr = IpHdr {
            ttl,
            ..Default::default()
        };
        prop_assert_eq!(hdr.ttl, ttl);
    }
}
