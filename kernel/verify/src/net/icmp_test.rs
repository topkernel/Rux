//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ICMP header constant invariant tests.
//!
//! Types copied from: kernel/src/net/icmp.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/net/icmp.rs
// ============================================================================

pub const ICMP_HDR_LEN: usize = 8;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IcmpHdr {
    pub icmp_type: u8,
    pub icmp_code: u8,
    pub icmp_checksum: u16,
    pub icmp_id: u16,
    pub icmp_seq: u16,
}

/// ICMP type constants
pub mod icmp_type {
    pub const ECHO_REPLY: u8 = 0;
    pub const DEST_UNREACH: u8 = 3;
    pub const ECHO_REQUEST: u8 = 8;
    pub const TIME_EXCEEDED: u8 = 11;
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-ICMP-1: IcmpHdr is exactly 8 bytes
    #[test]
    fn test_icmphdr_size(_v in 0u8..1u8) {
        prop_assert_eq!(core::mem::size_of::<IcmpHdr>(), ICMP_HDR_LEN);
    }

    /// INV-ICMP-2: ICMP header fields are at expected offsets
    #[test]
    fn test_field_offsets(_v in 0u8..1u8) {
        let hdr = IcmpHdr::default();
        let base = &hdr as *const IcmpHdr as usize;
        prop_assert_eq!(
            (&hdr.icmp_type as *const _ as usize) - base, 0
        );
        prop_assert_eq!(
            (&hdr.icmp_code as *const _ as usize) - base, 1
        );
        prop_assert_eq!(
            (&hdr.icmp_checksum as *const _ as usize) - base, 2
        );
        prop_assert_eq!(
            (&hdr.icmp_id as *const _ as usize) - base, 4
        );
        prop_assert_eq!(
            (&hdr.icmp_seq as *const _ as usize) - base, 6
        );
    }
}

#[test]
/// INV-ICMP-3: ICMP type constants match IANA assignments
fn test_type_constants() {
    assert_eq!(icmp_type::ECHO_REPLY, 0);
    assert_eq!(icmp_type::DEST_UNREACH, 3);
    assert_eq!(icmp_type::ECHO_REQUEST, 8);
    assert_eq!(icmp_type::TIME_EXCEEDED, 11);
}

#[test]
/// INV-ICMP-4: ICMP header length is 8 bytes
fn test_header_length() {
    assert_eq!(ICMP_HDR_LEN, 8);
}

#[test]
/// INV-ICMP-5: ICMP type constants are all distinct
fn test_types_distinct() {
    let types = [
        icmp_type::ECHO_REPLY,
        icmp_type::DEST_UNREACH,
        icmp_type::ECHO_REQUEST,
        icmp_type::TIME_EXCEEDED,
    ];
    let mut seen = std::collections::HashSet::new();
    for &t in &types {
        assert!(seen.insert(t), "duplicate ICMP type: {}", t);
    }
}

#[test]
/// INV-ICMP-6: Default IcmpHdr is all zeros
fn test_default_zeroed() {
    let hdr = IcmpHdr::default();
    unsafe {
        let bytes = core::mem::transmute::<IcmpHdr, [u8; 8]>(hdr);
        assert_eq!(bytes, [0u8; 8]);
    }
}
