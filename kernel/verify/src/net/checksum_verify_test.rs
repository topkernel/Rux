//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Extended checksum verification tests: carry chains, edge cases.
//! Builds on kernel/src/net/ipv4/checksum.rs

use proptest::prelude::*;

/// Internet checksum (RFC 1071) — copied from kernel/src/net/ipv4/checksum.rs
pub fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        if i + 1 == data.len() {
            sum += (data[i] as u32) << 8;
        } else {
            let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            sum += word;
        }
        i += 2;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
}

proptest! {
    /// INV-CSUM-EXT1: verify property for even-length data
    /// After appending checksum, recomputing yields 0
    #[test]
    fn test_verify_even_length(
        word0 in 0u16..=0xFFFFu16,
        word1 in 0u16..=0xFFFFu16,
        extra_words in 0usize..20usize,
        seed in 0u32..10_000u32,
    ) {
        let mut data = Vec::new();
        data.extend_from_slice(&word0.to_be_bytes());
        data.extend_from_slice(&word1.to_be_bytes());
        for i in 0..extra_words {
            let w = (seed.wrapping_add(i as u32).wrapping_mul(2654435761)) as u16;
            data.extend_from_slice(&w.to_be_bytes());
        }
        // data is always even-length (2 + 2 + 2*extra_words)
        assert!(data.len() % 2 == 0);

        let csum = ip_checksum(&data);
        data.extend_from_slice(&csum.to_be_bytes());
        prop_assert_eq!(ip_checksum(&data), 0);
    }

    /// INV-CSUM-EXT2: checksum of all-0xFF data (even length only)
    #[test]
    fn test_all_ff(half_len in 1usize..100usize) {
        let len = half_len * 2; // always even
        let data = vec![0xFFu8; len];
        let csum = ip_checksum(&data);
        let mut full = data;
        full.extend_from_slice(&csum.to_be_bytes());
        prop_assert_eq!(ip_checksum(&full), 0);
    }

    /// INV-CSUM-EXT3: data with single 1 bit set (even-length base)
    #[test]
    fn test_single_bit(
        byte_idx in 0usize..100usize,
        bit_idx in 0u8..8u8,
    ) {
        let mut data = vec![0u8; 100]; // even length
        data[byte_idx] = 1 << bit_idx;
        let csum = ip_checksum(&data);
        let mut full = data;
        full.extend_from_slice(&csum.to_be_bytes());
        prop_assert_eq!(ip_checksum(&full), 0);
    }

    /// INV-CSUM-EXT4: carry folding with many 0xFFFF words
    #[test]
    fn test_carry_chain(count in 0usize..100usize) {
        let mut data = Vec::new();
        for _ in 0..count {
            data.extend_from_slice(&0xFFFFu16.to_be_bytes());
        }
        let csum = ip_checksum(&data);
        let mut full = data;
        full.extend_from_slice(&csum.to_be_bytes());
        prop_assert_eq!(ip_checksum(&full), 0);
    }

    /// INV-CSUM-EXT5: word order roundtrip
    #[test]
    fn test_word_order_roundtrip(
        w0 in 0u16..=0xFFFFu16,
        w1 in 0u16..=0xFFFFu16,
    ) {
        let mut fwd = Vec::new();
        fwd.extend_from_slice(&w0.to_be_bytes());
        fwd.extend_from_slice(&w1.to_be_bytes());
        let csum_fwd = ip_checksum(&fwd);
        fwd.extend_from_slice(&csum_fwd.to_be_bytes());
        prop_assert_eq!(ip_checksum(&fwd), 0);

        let mut rev = Vec::new();
        rev.extend_from_slice(&w1.to_be_bytes());
        rev.extend_from_slice(&w0.to_be_bytes());
        let csum_rev = ip_checksum(&rev);
        rev.extend_from_slice(&csum_rev.to_be_bytes());
        prop_assert_eq!(ip_checksum(&rev), 0);
    }

    /// INV-CSUM-EXT6: single-byte data
    #[test]
    fn test_single_byte(val in 0u8..=0xFFu8) {
        let data = [val];
        let csum = ip_checksum(&data);
        let expected = !((val as u32) << 8) as u16;
        prop_assert_eq!(csum, expected);
    }

    /// INV-CSUM-EXT7: UDP-like pseudo-header checksum with embedded checksum
    #[test]
    fn test_udp_pseudo_header(
        src_ip in 0u32..0xFFFF_FFFFu32,
        dst_ip in 0u32..0xFFFF_FFFFu32,
        src_port in 1u16..0xFFFFu16,
        dst_port in 1u16..0xFFFFu16,
        data_words in 0usize..50usize,
        seed in 0u32..10_000u32,
    ) {
        // Build pseudo-header + UDP header + data as byte array (all big-endian)
        let data: Vec<u8> = (0..data_words)
            .flat_map(|i| {
                let w = (seed.wrapping_add(i as u32).wrapping_mul(37)) as u16;
                w.to_be_bytes()
            })
            .collect();
        let udp_len = (8 + data.len()) as u16;

        let mut packet = Vec::new();
        // Pseudo header (12 bytes)
        packet.extend_from_slice(&src_ip.to_be_bytes());
        packet.extend_from_slice(&dst_ip.to_be_bytes());
        packet.push(0);                    // zero
        packet.push(17);                   // protocol = UDP
        packet.extend_from_slice(&udp_len.to_be_bytes());
        // UDP header (8 bytes)
        packet.extend_from_slice(&src_port.to_be_bytes());
        packet.extend_from_slice(&dst_port.to_be_bytes());
        packet.extend_from_slice(&udp_len.to_be_bytes());
        packet.extend_from_slice(&[0u8, 0]); // checksum = 0
        // Data
        packet.extend_from_slice(&data);

        let csum = ip_checksum(&packet);
        // Embed checksum at offset 18 (12 pseudo + 6 UDP before checksum field)
        packet[18] = (csum >> 8) as u8;
        packet[19] = (csum & 0xFF) as u8;
        prop_assert_eq!(ip_checksum(&packet), 0);
    }
}
