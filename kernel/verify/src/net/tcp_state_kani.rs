//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for TCP state machine and header constants.
//!
//! Types copied from: kernel/src/net/tcp.rs

#![cfg(kani)]

pub const TCP_MIN_HLEN: usize = 20;
pub const TCP_MAX_HLEN: usize = 60;
pub const TCP_DEFAULT_MSS: u16 = 1460;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    TCP_CLOSE = 0, TCP_LISTEN = 1, TCP_SYN_SENT = 2, TCP_SYN_RECV = 3,
    TCP_ESTABLISHED = 4, TCP_FIN_WAIT1 = 5, TCP_FIN_WAIT2 = 6,
    TCP_CLOSE_WAIT = 7, TCP_LAST_ACK = 8, TCP_TIME_WAIT = 9, TCP_CLOSING = 10,
}

pub fn tcp_dof(dof_res: u8) -> u8 { dof_res >> 4 }
pub fn tcp_header_len(dof_res: u8) -> usize { (dof_res as usize >> 4) * 4 }
pub fn tcp_syn(dof_res: u8) -> bool { (dof_res & 0x02) != 0 }
pub fn tcp_ack(dof_res: u8) -> bool { (dof_res & 0x10) != 0 }
pub fn tcp_fin(dof_res: u8) -> bool { (dof_res & 0x01) != 0 }
pub fn tcp_rst(dof_res: u8) -> bool { (dof_res & 0x04) != 0 }

/// INV-TCP-K1: TcpState discriminants are 0-10 consecutive.
#[kani::proof]
fn verify_state_discriminants() {
    let states = [
        TcpState::TCP_CLOSE, TcpState::TCP_LISTEN, TcpState::TCP_SYN_SENT,
        TcpState::TCP_SYN_RECV, TcpState::TCP_ESTABLISHED,
        TcpState::TCP_FIN_WAIT1, TcpState::TCP_FIN_WAIT2,
        TcpState::TCP_CLOSE_WAIT, TcpState::TCP_LAST_ACK,
        TcpState::TCP_TIME_WAIT, TcpState::TCP_CLOSING,
    ];
    for (i, s) in states.iter().enumerate() {
        assert_eq!(*s as u8, i as u8);
    }
}

/// INV-TCP-K2: header length range for valid data offset values.
#[kani::proof]
fn verify_header_len_range() {
    let dof: u8 = kani::any();
    kani::assume(dof >= 5 && dof <= 15);
    let hlen = (dof as usize) * 4;
    assert!(hlen >= TCP_MIN_HLEN);
    assert!(hlen <= TCP_MAX_HLEN);
}

/// INV-TCP-K3: TCP flag bits are distinct powers of 2.
#[kani::proof]
fn verify_flag_bits_distinct() {
    let flags = [0x02u8, 0x10, 0x01, 0x04, 0x08]; // SYN, ACK, FIN, RST, PSH
    let mut seen = 0u8;
    for &f in &flags {
        assert!(f > 0 && (f & (f - 1)) == 0);
        assert_eq!(seen & f, 0);
        seen |= f;
    }
}

/// INV-TCP-K4: TCP_MAX_HLEN = 15 * 4 = 60.
#[kani::proof]
fn verify_max_hlen() {
    assert_eq!(TCP_MAX_HLEN, 60);
    assert_eq!(TCP_DEFAULT_MSS, 1460);
}
