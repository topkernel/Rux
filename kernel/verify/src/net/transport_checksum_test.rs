//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for UDP/ICMP/TCP transport checksums.
//! Copied from: kernel/src/net/udp.rs, kernel/src/net/icmp.rs, kernel/src/net/tcp.rs
//!
//! Uses byte-array approach for roundtrip verification to avoid
//! struct byte-order issues on little-endian hosts.

use proptest::prelude::*;

// ============================================================================
// Internet checksum (RFC 1071)
// ============================================================================

fn ip_checksum(data: &[u8]) -> u16 {
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

// ============================================================================
// Copied: ICMP checksum from kernel/src/net/icmp.rs
// ============================================================================

/// ICMP compute_checksum — operates on raw byte arrays (big-endian)
pub fn icmp_compute_checksum(hdr: &[u8], data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    let mut i = 0;
    while i + 1 < hdr.len() {
        let word = u16::from_be_bytes([hdr[i], hdr[i + 1]]) as u32;
        sum += word;
        i += 2;
    }
    if i < hdr.len() {
        sum += (hdr[i] as u32) << 8;
    }

    let mut i = 0;
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        sum += word;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

// ============================================================================
// UDP checksum via byte-array (pseudo header + header + data)
// ============================================================================

fn build_udp_packet(src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16, data: &[u8]) -> Vec<u8> {
    let udp_len = (8 + data.len()) as u16;
    let mut packet = Vec::new();
    // Pseudo header (12 bytes)
    packet.extend_from_slice(&src_ip.to_be_bytes());
    packet.extend_from_slice(&dst_ip.to_be_bytes());
    packet.push(0);                     // zero
    packet.push(17);                    // protocol = UDP
    packet.extend_from_slice(&udp_len.to_be_bytes());
    // UDP header (8 bytes)
    packet.extend_from_slice(&src_port.to_be_bytes());
    packet.extend_from_slice(&dst_port.to_be_bytes());
    packet.extend_from_slice(&udp_len.to_be_bytes());
    packet.extend_from_slice(&[0u8, 0]); // checksum = 0
    // Data
    packet.extend_from_slice(data);
    packet
}

const UDP_CHECKSUM_OFFSET: usize = 18; // 12 (pseudo) + 6 (before checksum)

fn udp_checksum_verify(src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16, data: &[u8]) -> u16 {
    let mut packet = build_udp_packet(src_ip, dst_ip, src_port, dst_port, data);
    let csum = ip_checksum(&packet);
    packet[UDP_CHECKSUM_OFFSET] = (csum >> 8) as u8;
    packet[UDP_CHECKSUM_OFFSET + 1] = (csum & 0xFF) as u8;
    ip_checksum(&packet)
}

// ============================================================================
// TCP checksum via byte-array
// ============================================================================

fn build_tcp_packet(src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16,
                     seq: u32, ack_seq: u32, flags: u8, window: u16, data: &[u8]) -> Vec<u8> {
    let header_len = 20u16;
    let tcp_len = header_len + data.len() as u16;
    let data_off_flags = 0x50u8 | flags; // data_off=5 (20 bytes), flags in low nibble

    let mut packet = Vec::new();
    // Pseudo header (12 bytes)
    packet.extend_from_slice(&src_ip.to_be_bytes());
    packet.extend_from_slice(&dst_ip.to_be_bytes());
    packet.push(0);                     // zero
    packet.push(6);                     // protocol = TCP
    packet.extend_from_slice(&tcp_len.to_be_bytes());
    // TCP header (20 bytes)
    packet.extend_from_slice(&src_port.to_be_bytes());
    packet.extend_from_slice(&dst_port.to_be_bytes());
    packet.extend_from_slice(&seq.to_be_bytes());
    packet.extend_from_slice(&ack_seq.to_be_bytes());
    packet.push(data_off_flags);
    packet.push(0);                     // reserved + flags upper bits
    packet.extend_from_slice(&window.to_be_bytes());
    packet.extend_from_slice(&[0u8, 0]); // checksum = 0
    packet.extend_from_slice(&[0u8, 0]); // urgent pointer = 0
    // Data
    packet.extend_from_slice(data);
    packet
}

const TCP_CHECKSUM_OFFSET: usize = 30; // 12 (pseudo) + 18 (before checksum in 20-byte header)

fn tcp_checksum_verify(src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16,
                       seq: u32, ack_seq: u32, flags: u8, window: u16, data: &[u8]) -> u16 {
    let mut packet = build_tcp_packet(src_ip, dst_ip, src_port, dst_port, seq, ack_seq, flags, window, data);
    let csum = ip_checksum(&packet);
    packet[TCP_CHECKSUM_OFFSET] = (csum >> 8) as u8;
    packet[TCP_CHECKSUM_OFFSET + 1] = (csum & 0xFF) as u8;
    ip_checksum(&packet)
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-TCSUM-1: UDP checksum verify (byte-array, big-endian)
    #[test]
    fn test_udp_checksum_verify(
        src_ip in 0u32..0xFFFF_FFFFu32,
        dst_ip in 0u32..0xFFFF_FFFFu32,
        src_port in 0u16..0xFFFFu16,
        dst_port in 0u16..0xFFFFu16,
        data_len in 0usize..100usize,
        seed in 0u32..1000u32,
    ) {
        let data: Vec<u8> = (0..data_len).map(|i| ((seed + i as u32) * 31) as u8).collect();
        prop_assert_eq!(udp_checksum_verify(src_ip, dst_ip, src_port, dst_port, &data), 0);
    }

    /// INV-TCSUM-2: ICMP checksum verify (byte arrays, big-endian)
    #[test]
    fn test_icmp_checksum_verify(
        data_len in 0usize..100usize,
        seed in 0u32..1000u32,
    ) {
        let mut hdr = [0u8; 8];
        hdr[0] = 8; hdr[1] = 0; // Echo request
        hdr[4] = 0x12; hdr[5] = 0x34;
        hdr[6] = 0x00; hdr[7] = 0x01;

        let data: Vec<u8> = (0..data_len).map(|i| ((seed + i as u32) * 37) as u8).collect();
        let csum = icmp_compute_checksum(&hdr, &data);

        hdr[2] = (csum >> 8) as u8;
        hdr[3] = (csum & 0xFF) as u8;
        let verify = icmp_compute_checksum(&hdr, &data);
        // With checksum embedded, recomputing should yield 0 (complement of 0xFFFF)
        prop_assert_eq!(verify, 0);
    }

    /// INV-TCSUM-3: TCP checksum verify (byte-array, big-endian)
    #[test]
    fn test_tcp_checksum_verify(
        src_ip in 0u32..0xFFFF_FFFFu32,
        dst_ip in 0u32..0xFFFF_FFFFu32,
        src_port in 0u16..0xFFFFu16,
        dst_port in 0u16..0xFFFFu16,
        seq in 0u32..0xFFFF_FFFFu32,
        data_len in 0usize..100usize,
        seed in 0u32..1000u32,
    ) {
        let data: Vec<u8> = (0..data_len).map(|i| ((seed + i as u32) * 41) as u8).collect();
        let ack_seq = seq.wrapping_add(data_len as u32);
        prop_assert_eq!(tcp_checksum_verify(src_ip, dst_ip, src_port, dst_port, seq, ack_seq, 0x18, 65535, &data), 0);
    }

    /// INV-TCSUM-4: UDP empty data
    #[test]
    fn test_udp_empty_data(
        src_ip in 0u32..0xFFFF_FFFFu32,
        dst_ip in 0u32..0xFFFF_FFFFu32,
        src_port in 1u16..0xFFFFu16,
        dst_port in 1u16..0xFFFFu16,
    ) {
        prop_assert_eq!(udp_checksum_verify(src_ip, dst_ip, src_port, dst_port, &[]), 0);
    }

    /// INV-TCSUM-5: ICMP empty data
    #[test]
    fn test_icmp_empty_data(seed in 0u32..1000u32) {
        let mut hdr = [0u8; 8];
        hdr[0] = 8; hdr[1] = 0;
        hdr[4] = ((seed >> 8) & 0xFF) as u8;
        hdr[5] = (seed & 0xFF) as u8;
        hdr[6] = 1; hdr[7] = 0;

        let csum = icmp_compute_checksum(&hdr, &[]);
        hdr[2] = (csum >> 8) as u8;
        hdr[3] = (csum & 0xFF) as u8;
        prop_assert_eq!(icmp_compute_checksum(&hdr, &[]), 0);
    }

    /// INV-TCSUM-6: TCP empty data
    #[test]
    fn test_tcp_empty_data(
        src_ip in 0u32..0xFFFF_FFFFu32,
        dst_ip in 0u32..0xFFFF_FFFFu32,
    ) {
        prop_assert_eq!(tcp_checksum_verify(src_ip, dst_ip, 12345, 80, 1000, 0, 0x02, 65535, &[]), 0);
    }

    /// INV-TCSUM-7: UDP odd-length data
    #[test]
    fn test_udp_odd_length(
        src_ip in 0x01000000u32..0xFEFFFFFFu32,
        dst_ip in 0x01000000u32..0xFEFFFFFFu32,
        odd_len in 1usize..50usize,
        seed in 0u32..1000u32,
    ) {
        let data_len = odd_len * 2 + 1;
        let data: Vec<u8> = (0..data_len).map(|i| ((seed + i as u32) * 23) as u8).collect();
        prop_assert_eq!(udp_checksum_verify(src_ip, dst_ip, 1000, 2000, &data), 0);
    }

    /// INV-TCSUM-8: ICMP odd-length data
    #[test]
    fn test_icmp_odd_length(
        odd_len in 1usize..50usize,
        seed in 0u32..1000u32,
    ) {
        let data_len = odd_len * 2 + 1;
        let data: Vec<u8> = (0..data_len).map(|i| ((seed + i as u32) * 53) as u8).collect();
        let mut hdr = [0u8; 8];
        hdr[0] = 3; hdr[1] = 0;
        hdr[4] = (seed & 0xFF) as u8;
        hdr[5] = ((seed >> 8) & 0xFF) as u8;
        hdr[6] = ((seed >> 16) & 0xFF) as u8;
        hdr[7] = ((seed >> 24) & 0xFF) as u8;
        let csum = icmp_compute_checksum(&hdr, &data);
        hdr[2] = (csum >> 8) as u8;
        hdr[3] = (csum & 0xFF) as u8;
        prop_assert_eq!(icmp_compute_checksum(&hdr, &data), 0);
    }

    /// INV-TCSUM-9: UDP checksum depends on source IP
    #[test]
    fn test_udp_checksum_depends_on_src_ip(
        src1 in 0x01000000u32..0x010000FFu32,
        src2 in 0x02000000u32..0x020000FFu32,
    ) {
        let dst = 0x03000000u32;
        let data = [1u8, 2, 3, 4];
        let pkt1 = build_udp_packet(src1, dst, 1000, 2000, &data);
        let pkt2 = build_udp_packet(src2, dst, 1000, 2000, &data);
        prop_assert_ne!(ip_checksum(&pkt1), ip_checksum(&pkt2));
    }

    /// INV-TCSUM-10: TCP vs UDP protocol difference
    #[test]
    fn test_tcp_vs_udp_protocol(
        src_ip in 0x01000000u32..0x01FFFFFFu32,
        dst_ip in 0x02000000u32..0x02FFFFFFu32,
    ) {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let udp_pkt = build_udp_packet(src_ip, dst_ip, 1000, 80, &data);
        let tcp_pkt = build_tcp_packet(src_ip, dst_ip, 1000, 80, 1, 0, 0x18, 65535, &data);
        prop_assert_ne!(ip_checksum(&udp_pkt), ip_checksum(&tcp_pkt));
    }
}
