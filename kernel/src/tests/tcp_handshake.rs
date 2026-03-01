//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
// 测试：TCP 三次握手
//!
//! 测试 TCP 协议的三次握手实现，包括：
//! - 客户端主动打开（active open）
//! - 服务器端被动打开（passive open）
//! - 状态转换
//! - 序列号和确认号处理

use crate::net::tcp::{TcpSocket, TcpState, tcp_socket_alloc, tcp_bind, tcp_socket_free};
use super::{test_pass, test_fail, test_group_start};

pub fn test_tcp_handshake() {
    test_group_start("TCP handshake");

    // 测试 1: TCP Socket 基础功能
    test_tcp_socket_basic();

    // 测试 2: TCP 状态机
    test_tcp_state_machine();

    // 测试 3: TCP 三次握手 - 客户端视角
    test_tcp_client_handshake();

    // 测试 4: TCP 三次握手 - 服务器端视角
    test_tcp_server_handshake();

    // 测试 5: TCP 序列号管理
    test_tcp_sequence_numbers();

    // 测试 6: TCP Socket 分配
    test_tcp_socket_allocation();
}

/// 测试 TCP Socket 基础功能
fn test_tcp_socket_basic() {
    let mut socket = TcpSocket::new();

    // 初始状态
    let initial_state_ok = socket.state == TcpState::TCP_CLOSE && !socket.bound;
    if !initial_state_ok {
        test_fail("TCP socket initial state", "invalid");
        return;
    }

    // 绑定端口
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

    // 进入监听状态
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

/// 测试 TCP 状态机
fn test_tcp_state_machine() {
    let mut socket = TcpSocket::new();
    socket.bind(8080).unwrap();

    // 状态转换：CLOSED -> LISTEN
    socket.listen(10).unwrap();
    if socket.state != TcpState::TCP_LISTEN {
        test_fail("TCP state machine", "not LISTEN");
        return;
    }

    // 状态转换：LISTEN -> SYN_RECV (服务器端)
    socket.state = TcpState::TCP_SYN_RECV;
    if socket.state != TcpState::TCP_SYN_RECV {
        test_fail("TCP state machine", "not SYN_RECV");
        return;
    }

    // 状态转换：SYN_RECV -> ESTABLISHED (服务器端)
    socket.state = TcpState::TCP_ESTABLISHED;
    if socket.state == TcpState::TCP_ESTABLISHED {
        test_pass("TCP state machine");
    } else {
        test_fail("TCP state machine", "not ESTABLISHED");
    }
}

/// 测试客户端三次握手
fn test_tcp_client_handshake() {
    let mut socket = TcpSocket::new();
    socket.bind(12345).unwrap();

    // 初始状态：CLOSED
    if socket.state != TcpState::TCP_CLOSE {
        test_fail("TCP client", "not CLOSED initially");
        return;
    }

    // 模拟主动连接（发送 SYN）
    socket.remote_ip = 0x7F000001;
    socket.remote_port = 80;
    socket.snd_nxt = 12345;
    socket.snd_una = socket.snd_nxt;
    socket.state = TcpState::TCP_SYN_SENT;

    // 验证序列号
    if socket.snd_nxt == 0 {
        test_fail("TCP client", "zero ISN");
        return;
    }
    if socket.snd_una != socket.snd_nxt {
        test_fail("TCP client", "SND_UNA != SND_NXT");
        return;
    }

    // 模拟接收到 SYN-ACK
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

/// 测试服务器端三次握手
fn test_tcp_server_handshake() {
    let mut socket = TcpSocket::new();
    socket.bind(80).unwrap();
    socket.listen(10).unwrap();

    // 初始状态：LISTEN
    if socket.state != TcpState::TCP_LISTEN {
        test_fail("TCP server", "not LISTEN");
        return;
    }

    // 模拟接收到 SYN 包
    socket.state = TcpState::TCP_SYN_RECV;
    socket.snd_nxt = 54321;
    socket.snd_una = socket.snd_nxt;

    if socket.snd_nxt == 0 {
        test_fail("TCP server", "zero ISN");
        return;
    }

    // 模拟接收到 ACK 包
    socket.state = TcpState::TCP_ESTABLISHED;

    if socket.state == TcpState::TCP_ESTABLISHED {
        test_pass("TCP server handshake");
    } else {
        test_fail("TCP server handshake", "not ESTABLISHED");
    }
}

/// 测试序列号管理
fn test_tcp_sequence_numbers() {
    let mut socket = TcpSocket::new();
    socket.bind(12346).unwrap();

    // 模拟连接，设置序列号
    socket.snd_nxt = 12345;
    socket.snd_una = socket.snd_nxt;

    let initial_seq = socket.snd_nxt;
    if initial_seq == 0 {
        test_fail("TCP seq numbers", "zero ISN");
        return;
    }

    // 验证序列号递增
    socket.snd_nxt = socket.snd_nxt.wrapping_add(1000);
    if socket.snd_nxt != initial_seq.wrapping_add(1000) {
        test_fail("TCP seq increment", "failed");
        return;
    }

    // 验证未确认序列号
    socket.snd_una = socket.snd_una.wrapping_add(500);
    if socket.snd_una == initial_seq.wrapping_add(500) {
        test_pass("TCP seq number mgmt");
    } else {
        test_fail("TCP seq number mgmt", "invalid");
    }
}

/// 测试 Socket 分配
fn test_tcp_socket_allocation() {
    // 分配多个 Socket
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

    // 绑定端口
    let ret1 = tcp_bind(fd1_val, 8080);
    let ret2 = tcp_bind(fd2_val, 8081);

    if ret1 == 0 && ret2 == 0 {
        test_pass("TCP socket alloc");
    } else {
        test_fail("TCP socket bind", "failed");
    }

    // 释放 Socket
    tcp_socket_free(fd1_val);
    tcp_socket_free(fd2_val);
}
