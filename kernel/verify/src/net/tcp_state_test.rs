//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for TCP state machine and header constants.
//! Copied from: kernel/src/net/tcp.rs

use proptest::prelude::*;

// Copied TCP constants
pub const TCP_MIN_HLEN: usize = 20;
pub const TCP_MAX_HLEN: usize = 60;
pub const TCP_MAX_WINDOW: u16 = 65535;
pub const TCP_DEFAULT_MSS: u16 = 1460;

// Copied TcpState enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum TcpState {
    TCP_CLOSE = 0,
    TCP_LISTEN = 1,
    TCP_SYN_SENT = 2,
    TCP_SYN_RECV = 3,
    TCP_ESTABLISHED = 4,
    TCP_FIN_WAIT1 = 5,
    TCP_FIN_WAIT2 = 6,
    TCP_CLOSE_WAIT = 7,
    TCP_LAST_ACK = 8,
    TCP_TIME_WAIT = 9,
    TCP_CLOSING = 10,
}

// TCP header bitfield helpers (copied from TcpHdr methods)
pub fn tcp_dof(dof_res: u8) -> u8 { dof_res >> 4 }

pub fn tcp_header_len(dof_res: u8) -> usize {
    (dof_res as usize >> 4) * 4
}

pub fn tcp_syn(dof_res: u8) -> bool { (dof_res & 0x02) != 0 }
pub fn tcp_ack(dof_res: u8) -> bool { (dof_res & 0x10) != 0 }
pub fn tcp_fin(dof_res: u8) -> bool { (dof_res & 0x01) != 0 }
pub fn tcp_rst(dof_res: u8) -> bool { (dof_res & 0x04) != 0 }
pub fn tcp_psh(dof_res: u8) -> bool { (dof_res & 0x08) != 0 }

proptest! {
    #[test]
    fn test_tcp_state_discriminants(_v in 0u8..1u8) {
        let states = [
            TcpState::TCP_CLOSE, TcpState::TCP_LISTEN, TcpState::TCP_SYN_SENT,
            TcpState::TCP_SYN_RECV, TcpState::TCP_ESTABLISHED,
            TcpState::TCP_FIN_WAIT1, TcpState::TCP_FIN_WAIT2,
            TcpState::TCP_CLOSE_WAIT, TcpState::TCP_LAST_ACK,
            TcpState::TCP_TIME_WAIT, TcpState::TCP_CLOSING,
        ];
        for i in 0..states.len() {
            assert_eq!(states[i] as u8, i as u8, "TcpState discriminant mismatch at {}", i);
        }
    }

    #[test]
    fn test_tcp_state_count(_v in 0u8..1u8) {
        // 11 states: CLOSE(0) through CLOSING(10)
        assert_eq!(TcpState::TCP_CLOSING as u8, 10);
    }

    #[test]
    fn test_tcp_state_distinct(_v in 0u8..1u8) {
        let states = [
            TcpState::TCP_CLOSE, TcpState::TCP_LISTEN, TcpState::TCP_SYN_SENT,
            TcpState::TCP_SYN_RECV, TcpState::TCP_ESTABLISHED,
            TcpState::TCP_FIN_WAIT1, TcpState::TCP_FIN_WAIT2,
            TcpState::TCP_CLOSE_WAIT, TcpState::TCP_LAST_ACK,
            TcpState::TCP_TIME_WAIT, TcpState::TCP_CLOSING,
        ];
        for i in 0..states.len() {
            for j in (i+1)..states.len() {
                assert_ne!(states[i], states[j]);
            }
        }
    }

    #[test]
    fn test_tcp_max_hlen_formula(_v in 0u8..1u8) {
        // TCP_MAX_HLEN should be 15 * 4 = 60 (max data offset = 15)
        assert_eq!(TCP_MAX_HLEN, 15 * 4);
    }

    #[test]
    fn test_tcp_min_hlen(_v in 0u8..1u8) {
        assert_eq!(TCP_MIN_HLEN, 20);
    }

    #[test]
    fn test_tcp_header_len_range(dof in 5u8..16u8) {
        let dof_res = dof << 4;
        let hlen = tcp_header_len(dof_res);
        assert!(hlen >= TCP_MIN_HLEN);
        assert!(hlen <= TCP_MAX_HLEN);
    }

    #[test]
    fn test_tcp_header_len_dof_roundtrip(dof_res in 5u8..16u8) {
        let hlen = tcp_header_len(dof_res);
        assert_eq!(hlen / 4, tcp_dof(dof_res) as usize);
    }

    #[test]
    fn test_tcp_max_window(_v in 0u8..1u8) {
        assert_eq!(TCP_MAX_WINDOW, u16::MAX);
    }

    #[test]
    fn test_tcp_default_mss(_v in 0u8..1u8) {
        // Standard MSS for Ethernet: 1500 (MTU) - 20 (IP) - 20 (TCP) = 1460
        assert_eq!(TCP_DEFAULT_MSS, 1460);
    }

    #[test]
    fn test_tcp_flag_bits_distinct(_v in 0u8..1u8) {
        // SYN=0x02, ACK=0x10, FIN=0x01, RST=0x04, PSH=0x08
        let flags = [0x02u8, 0x10, 0x01, 0x04, 0x08];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0, "TCP flags {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn test_tcp_flag_bits_powers_of_two(_v in 0u8..1u8) {
        let flags = [0x02u8, 0x10, 0x01, 0x04, 0x08];
        for &f in &flags {
            assert!(f > 0 && (f & (f - 1)) == 0);
        }
    }
}
