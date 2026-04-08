//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 header and fragment flag invariant tests.
//!
//! Types copied from: kernel/src/net/ipv4/mod.rs

use proptest::prelude::*;

// ============================================================================
// Copied constants from kernel/src/net/ipv4/mod.rs
// ============================================================================

pub const IP_ALEN: usize = 4;
pub const IPHDR_LEN: usize = 20;
pub const IP_MIN_MTU: u16 = 68;
pub const IP_MAX_MTU: u16 = 65535;

pub mod ip_frag_flags {
    pub const RB: u16 = 0x8000;
    pub const DF: u16 = 0x4000;
    pub const MF: u16 = 0x2000;
    pub const OFFSET_MASK: u16 = 0x1FFF;
}

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

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-IP-1: IpHdr is exactly 20 bytes
    #[test]
    fn test_iphdr_size(_v in 0u8..1u8) {
        prop_assert_eq!(core::mem::size_of::<IpHdr>(), IPHDR_LEN);
    }

    /// INV-IP-2: version field (high nibble of version_ihl) is 4 for valid header
    #[test]
    fn test_version_ihl(version in 4u8..5u8, ihl in 5u8..16u8) {
        let hdr = IpHdr {
            version_ihl: (version << 4) | ihl,
            ..Default::default()
        };
        prop_assert_eq!(hdr.version_ihl >> 4, 4);
        prop_assert_eq!(hdr.version_ihl & 0x0F, ihl);
    }

    /// INV-IP-3: IHL * 4 == header length in bytes
    #[test]
    fn test_ihl_to_bytes(ihl in 5u8..16u8) {
        let header_bytes = (ihl as usize) * 4;
        prop_assert!(header_bytes >= IPHDR_LEN);
        prop_assert!(header_bytes <= 60); // max 60 bytes for IP header
    }

    /// INV-IP-4: Fragment flags are in high 3 bits of frag_off
    #[test]
    fn test_frag_flag_positions(flag_val in 0u16..8u16) {
        let frag_off = flag_val << 13;
        let rb_set = (frag_off & ip_frag_flags::RB) != 0;
        let df_set = (frag_off & ip_frag_flags::DF) != 0;
        let mf_set = (frag_off & ip_frag_flags::MF) != 0;
        // Bit 15 (RB), bit 14 (DF), bit 13 (MF)
        prop_assert_eq!(rb_set, (flag_val & 4) != 0);
        prop_assert_eq!(df_set, (flag_val & 2) != 0);
        prop_assert_eq!(mf_set, (flag_val & 1) != 0);
    }

    /// INV-IP-5: OFFSET_MASK extracts only lower 13 bits
    #[test]
    fn test_offset_mask(offset in 0u16..0x2000u16) {
        let frag_off = offset; // no flags set
        prop_assert_eq!(frag_off & ip_frag_flags::OFFSET_MASK, offset);
        prop_assert_eq!(frag_off & ip_frag_flags::OFFSET_MASK, frag_off);
    }

    /// INV-IP-6: Combining flags and offset preserves both
    #[test]
    fn test_flags_plus_offset(
        flags in 0u16..8u16,
        offset in 0u16..0x2000u16,
    ) {
        let frag_off = (flags << 13) | offset;
        let extracted_flags = (frag_off >> 13) & 0x7;
        let extracted_offset = frag_off & ip_frag_flags::OFFSET_MASK;
        prop_assert_eq!(extracted_flags, flags);
        prop_assert_eq!(extracted_offset, offset);
    }

    /// INV-IP-7: MTU is within valid range
    #[test]
    fn test_mtu_range(mtu in IP_MIN_MTU..=IP_MAX_MTU) {
        prop_assert!(mtu >= IP_MIN_MTU);
        prop_assert!(mtu <= IP_MAX_MTU);
    }

    /// INV-IP-8: IP address length is 4 bytes
    #[test]
    fn test_ip_addr_len(_v in 0u8..1u8) {
        prop_assert_eq!(IP_ALEN, 4);
        prop_assert_eq!(core::mem::size_of::<u32>(), IP_ALEN);
    }
}

#[test]
/// INV-IP-9: Fragment flag constants are distinct powers of two
fn test_frag_flags_powers_of_two() {
    let flags = [
        ("RB", ip_frag_flags::RB),
        ("DF", ip_frag_flags::DF),
        ("MF", ip_frag_flags::MF),
    ];
    let mut seen = std::collections::HashSet::new();
    for &(name, val) in &flags {
        assert!(val > 0 && (val & (val - 1)) == 0,
            "{} ({:#x}) is not a power of two", name, val);
        assert!(seen.insert(val), "{} is a duplicate flag", name);
    }
}

#[test]
/// INV-IP-10: OFFSET_MASK covers exactly the lower 13 bits
fn test_offset_mask_bits() {
    assert_eq!(ip_frag_flags::OFFSET_MASK, 0x1FFF);
    assert_eq!(ip_frag_flags::OFFSET_MASK.count_ones(), 13);
}

#[test]
/// INV-IP-11: Flag bits and offset mask are disjoint and cover all 16 bits
fn test_flag_offset_disjoint() {
    let all_flags = ip_frag_flags::RB | ip_frag_flags::DF | ip_frag_flags::MF;
    assert_eq!(all_flags & ip_frag_flags::OFFSET_MASK, 0, "flags and offset overlap");
    assert_eq!(all_flags | ip_frag_flags::OFFSET_MASK, 0xFFFF, "don't cover all 16 bits");
}

#[test]
/// INV-IP-12: IP_MIN_MTU and IP_MAX_MTU are sensible
fn test_mtu_constants() {
    assert!(IP_MIN_MTU <= IP_MAX_MTU);
    assert_eq!(IP_MIN_MTU, 68); // RFC 791 minimum reassembly buffer
    assert_eq!(IP_MAX_MTU, 65535); // max IP datagram size
}
