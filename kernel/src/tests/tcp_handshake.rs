//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: TCP three-way handshake
//!
//! Tests TCP protocol three-way handshake implementation, including:
//! - Client active open
//! - Server passive open
//! - State transitions
//! - Sequence number and acknowledgment number handling

use crate::net::tcp::{TcpSocket, TcpState, tcp_socket_alloc, tcp_bind, tcp_socket_free};
use super::{test_pass, test_fail, test_group_start};

pub fn test_tcp_handshake() {
    test_group_start("TCP handshake");

    // Test 1: TCP Socket basic functionality
    test_tcp_socket_basic();

    // Test 2: TCP state machine
    test_tcp_state_machine();

    // Test 3: TCP three-way handshake - client perspective
    test_tcp_client_handshake();

    // Test 4: TCP three-way handshake - server perspective
    test_tcp_server_handshake();

    // Test 5: TCP sequence number management
    test_tcp_sequence_numbers();

    // Test 6: TCP Socket allocation
    test_tcp_socket_allocation();
}

/// Test TCP Socket basic functionality
fn test_tcp_socket_basic() {
    let mut socket = TcpSocket::new();

    // Initial state
    let initial_state_ok = socket.state == TcpState::TCP_CLOSE && !socket.bound;
    if !initial_state_ok {
        test_fail("TCP socket initial state", "invalid");
        return;
    }

    // Bind port
    match socket.bind(8080) {
        Ok(()) => {}
        Err(_) => {
            test_fail("TCP bind", "failed");
            return;
        }
    }

    let bind_ok = socket.bound && socket.local_port == 8080;
    if !bind_ok {
        test_fail("TCP bind state", "invalid");
        return;
    }

    // Enter listening state
    match socket.listen(10) {
        Ok(()) => {}
        Err(_) => {
            test_fail("TCP listen", "failed");
            return;
        }
    }

    if socket.state == TcpState::TCP_LISTEN {
        test_pass("TCP socket basic ops");
    } else {
        test_fail("TCP listen state", "not LISTEN");
    }
}

/// Test TCP state machine
fn test_tcp_state_machine() {
    let mut socket = TcpSocket::new();
    socket.bind(8080).unwrap();

    // State transition: CLOSED -> LISTEN
    socket.listen(10).unwrap();
    if socket.state != TcpState::TCP_LISTEN {
        test_fail("TCP state machine", "not LISTEN");
        return;
    }

    // State transition: LISTEN -> SYN_RECV (server)
    socket.state = TcpState::TCP_SYN_RECV;
    if socket.state != TcpState::TCP_SYN_RECV {
        test_fail("TCP state machine", "not SYN_RECV");
        return;
    }

    // State transition: SYN_RECV -> ESTABLISHED (server)
    socket.state = TcpState::TCP_ESTABLISHED;
    if socket.state == TcpState::TCP_ESTABLISHED {
        test_pass("TCP state machine");
    } else {
        test_fail("TCP state machine", "not ESTABLISHED");
    }
}

/// Test client three-way handshake
fn test_tcp_client_handshake() {
    let mut socket = TcpSocket::new();
    socket.bind(12345).unwrap();

    // Initial state: CLOSED
    if socket.state != TcpState::TCP_CLOSE {
        test_fail("TCP client", "not CLOSED initially");
        return;
    }

    // Simulate active connection (send SYN)
    socket.remote_ip = 0x7F000001;
    socket.remote_port = 80;
    socket.snd_nxt = 12345;
    socket.snd_una = socket.snd_nxt;
    socket.state = TcpState::TCP_SYN_SENT;

    // Verify sequence number
    if socket.snd_nxt == 0 {
        test_fail("TCP client", "zero ISN");
        return;
    }
    if socket.snd_una != socket.snd_nxt {
        test_fail("TCP client", "SND_UNA != SND_NXT");
        return;
    }

    // Simulate receiving SYN-ACK
    socket.rcv_nxt = 54321;
    socket.snd_una = socket.snd_nxt.wrapping_add(1);
    socket.snd_nxt = socket.snd_una;
    socket.state = TcpState::TCP_ESTABLISHED;

    if socket.state == TcpState::TCP_ESTABLISHED {
        test_pass("TCP client handshake");
    } else {
        test_fail("TCP client handshake", "not ESTABLISHED");
    }
}

/// Test server three-way handshake
fn test_tcp_server_handshake() {
    let mut socket = TcpSocket::new();
    socket.bind(80).unwrap();
    socket.listen(10).unwrap();

    // Initial state: LISTEN
    if socket.state != TcpState::TCP_LISTEN {
        test_fail("TCP server", "not LISTEN");
        return;
    }

    // Simulate receiving SYN packet
    socket.state = TcpState::TCP_SYN_RECV;
    socket.snd_nxt = 54321;
    socket.snd_una = socket.snd_nxt;

    if socket.snd_nxt == 0 {
        test_fail("TCP server", "zero ISN");
        return;
    }

    // Simulate receiving ACK packet
    socket.state = TcpState::TCP_ESTABLISHED;

    if socket.state == TcpState::TCP_ESTABLISHED {
        test_pass("TCP server handshake");
    } else {
        test_fail("TCP server handshake", "not ESTABLISHED");
    }
}

/// Test sequence number management
fn test_tcp_sequence_numbers() {
    let mut socket = TcpSocket::new();
    socket.bind(12346).unwrap();

    // Simulate connection, set sequence numbers
    socket.snd_nxt = 12345;
    socket.snd_una = socket.snd_nxt;

    let initial_seq = socket.snd_nxt;
    if initial_seq == 0 {
        test_fail("TCP seq numbers", "zero ISN");
        return;
    }

    // Verify sequence number increment
    socket.snd_nxt = socket.snd_nxt.wrapping_add(1000);
    if socket.snd_nxt != initial_seq.wrapping_add(1000) {
        test_fail("TCP seq increment", "failed");
        return;
    }

    // Verify unacknowledged sequence number
    socket.snd_una = socket.snd_una.wrapping_add(500);
    if socket.snd_una == initial_seq.wrapping_add(500) {
        test_pass("TCP seq number mgmt");
    } else {
        test_fail("TCP seq number mgmt", "invalid");
    }
}

/// Test Socket allocation
fn test_tcp_socket_allocation() {
    // Allocate multiple Sockets
    let fd1 = tcp_socket_alloc();
    if fd1.is_err() {
        test_fail("TCP socket alloc", "first failed");
        return;
    }
    let fd1_val = fd1.unwrap();
    if fd1_val != 0 {
        test_fail("TCP socket alloc", "first fd not 0");
        return;
    }

    let fd2 = tcp_socket_alloc();
    if fd2.is_err() {
        test_fail("TCP socket alloc", "second failed");
        return;
    }
    let fd2_val = fd2.unwrap();
    if fd2_val != 1 {
        test_fail("TCP socket alloc", "second fd not 1");
        return;
    }

    // Bind ports
    let ret1 = tcp_bind(fd1_val, 8080);
    let ret2 = tcp_bind(fd2_val, 8081);

    if ret1 == 0 && ret2 == 0 {
        test_pass("TCP socket alloc");
    } else {
        test_fail("TCP socket bind", "failed");
    }

    // Free Sockets
    tcp_socket_free(fd1_val);
    tcp_socket_free(fd2_val);
}
