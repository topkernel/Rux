//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for socket address and protocol constants.
//! Copied from: kernel/src/net/socket.rs

use proptest::prelude::*;

// Copied socket constants
pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

// Copied SockAddrIn
#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

impl SockAddrIn {
    pub fn port(&self) -> u16 { u16::from_be(self.sin_port) }
    pub fn addr(&self) -> u32 { u32::from_be(self.sin_addr) }
}

proptest! {
    #[test]
    fn test_sockaddr_in_size(_v in 0u8..1u8) {
        // SockAddrIn: 2 + 2 + 4 + 8 = 16 bytes
        assert_eq!(core::mem::size_of::<SockAddrIn>(), 16);
    }

    #[test]
    fn test_port_roundtrip(port in 0u16..65535u16) {
        let sa = SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: 0,
            sin_zero: [0; 8],
        };
        prop_assert_eq!(sa.port(), port);
    }

    #[test]
    fn test_addr_roundtrip(addr in 0u32..0xFFFFFFFFu32) {
        let sa = SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: 0,
            sin_addr: addr.to_be(),
            sin_zero: [0; 8],
        };
        prop_assert_eq!(sa.addr(), addr);
    }

    #[test]
    fn test_protocol_constants_distinct(_v in 0u8..1u8) {
        // Address family constants
        let af = [AF_INET];
        // Socket type constants
        let sock_types = [SOCK_STREAM, SOCK_DGRAM];
        // IP protocol constants
        let ip_protos = [IPPROTO_TCP, IPPROTO_UDP];
        for i in 0..af.len() {
            for j in (i+1)..af.len() {
                assert_ne!(af[i], af[j]);
            }
        }
        for i in 0..sock_types.len() {
            for j in (i+1)..sock_types.len() {
                assert_ne!(sock_types[i], sock_types[j]);
            }
        }
        for i in 0..ip_protos.len() {
            for j in (i+1)..ip_protos.len() {
                assert_ne!(ip_protos[i], ip_protos[j]);
            }
        }
    }

    #[test]
    fn test_sock_type_values(_v in 0u8..1u8) {
        assert_eq!(SOCK_STREAM, 1);
        assert_eq!(SOCK_DGRAM, 2);
    }

    #[test]
    fn test_protocol_numbers(_v in 0u8..1u8) {
        assert_eq!(IPPROTO_TCP, 6);
        assert_eq!(IPPROTO_UDP, 17);
    }

    #[test]
    fn test_af_inet_value(_v in 0u8..1u8) {
        assert_eq!(AF_INET, 2);
    }

    #[test]
    fn test_loopback_addr(port in 0u16..65535u16) {
        // 127.0.0.1
        let sa = SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: 0x7F000001u32.to_be(),
            sin_zero: [0; 8],
        };
        assert_eq!(sa.addr(), 0x7F000001);
    }
}
