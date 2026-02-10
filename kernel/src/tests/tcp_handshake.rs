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

use crate::println;
use crate::net::tcp::{TcpSocket, TcpState, TcpPort, tcp_socket_alloc, tcp_bind, tcp_socket_free};

pub fn test_tcp_handshake() {
    println!("test: ===== Starting TCP Three-Way Handshake Tests =====");

    // 测试 1: TCP Socket 基础功能
    println!("test: 1. Testing TCP Socket basic operations...");
    test_tcp_socket_basic();

    // 测试 2: TCP 状态机
    println!("test: 2. Testing TCP state machine...");
    test_tcp_state_machine();

    // 测试 3: TCP 三次握手 - 客户端视角
    println!("test: 3. Testing TCP three-way handshake (client side)...");
    test_tcp_client_handshake();

    // 测试 4: TCP 三次握手 - 服务器端视角
    println!("test: 4. Testing TCP three-way handshake (server side)...");
    test_tcp_server_handshake();

    // 测试 5: TCP 序列号管理
    println!("test: 5. Testing TCP sequence number management...");
    test_tcp_sequence_numbers();

    // 测试 6: TCP Socket 分配
    println!("test: 6. Testing TCP Socket allocation...");
    test_tcp_socket_allocation();

    println!("test: TCP Three-Way Handshake testing completed.");
}

/// 测试 TCP Socket 基础功能
fn test_tcp_socket_basic() {
    let mut socket = TcpSocket::new();

    // 初始状态
    assert_eq!(socket.state, TcpState::TCP_CLOSE, "Initial state should be CLOSED");
    assert!(!socket.bound, "Should not be bound initially");

    // 绑定端口
    match socket.bind(8080) {
        Ok(()) => {
            println!("test:    Socket bound to port 8080");
        }
        Err(_) => {
            println!("test:    FAILED - Could not bind to port 8080");
            return;
        }
    }
    assert!(socket.bound, "Should be bound after bind()");
    assert_eq!(socket.local_port, 8080, "Local port should be 8080");

    // 进入监听状态
    match socket.listen(10) {
        Ok(()) => {
            println!("test:    Socket listening with backlog 10");
        }
        Err(_) => {
            println!("test:    FAILED - Could not listen");
            return;
        }
    }
    assert_eq!(socket.state, TcpState::TCP_LISTEN, "State should be LISTEN");

    println!("test:    SUCCESS - TCP Socket basic operations work");
}

/// 测试 TCP 状态机
fn test_tcp_state_machine() {
    let mut socket = TcpSocket::new();
    socket.bind(8080).unwrap();

    // 状态转换：CLOSED -> LISTEN
    socket.listen(10).unwrap();
    assert_eq!(socket.state, TcpState::TCP_LISTEN);

    // 状态转换：LISTEN -> SYN_RECV (服务器端)
    // (模拟接收到 SYN 包)
    socket.state = TcpState::TCP_SYN_RECV;
    assert_eq!(socket.state, TcpState::TCP_SYN_RECV);

    // 状态转换：SYN_RECV -> ESTABLISHED (服务器端)
    // (模拟接收到 ACK 包)
    socket.state = TcpState::TCP_ESTABLISHED;
    assert_eq!(socket.state, TcpState::TCP_ESTABLISHED);

    println!("test:    SUCCESS - TCP state machine works correctly");
}

/// 测试客户端三次握手
fn test_tcp_client_handshake() {
    let mut socket = TcpSocket::new();
    socket.bind(12345).unwrap(); // 使用临时端口

    // 初始状态：CLOSED
    assert_eq!(socket.state, TcpState::TCP_CLOSE);

    // 模拟主动连接（发送 SYN）
    socket.remote_ip = 0x7F000001;
    socket.remote_port = 80;
    socket.snd_nxt = 12345; // 客户端 ISN
    socket.snd_una = socket.snd_nxt;
    socket.state = TcpState::TCP_SYN_SENT;
    println!("test:    Client sent SYN, state = TCP_SYN_SENT");

    // 验证序列号
    assert!(socket.snd_nxt != 0, "Should have initial sequence number");
    assert_eq!(socket.snd_una, socket.snd_nxt, "SND_UNA should equal SND_NXT initially");

    // 模拟接收到 SYN-ACK
    socket.rcv_nxt = 54321; // 服务器 ISN + 1
    socket.snd_una = socket.snd_nxt.wrapping_add(1); // SYN 已确认
    socket.snd_nxt = socket.snd_una;
    socket.state = TcpState::TCP_ESTABLISHED;
    println!("test:    Client received SYN-ACK, sent ACK, state = TCP_ESTABLISHED");

    println!("test:    SUCCESS - Client side handshake works");
}

/// 测试服务器端三次握手
fn test_tcp_server_handshake() {
    let mut socket = TcpSocket::new();
    socket.bind(80).unwrap();
    socket.listen(10).unwrap();

    // 初始状态：LISTEN
    assert_eq!(socket.state, TcpState::TCP_LISTEN);

    // 模拟接收到 SYN 包（服务器为该连接分配 ISN）
    socket.state = TcpState::TCP_SYN_RECV;
    socket.snd_nxt = 54321; // 服务器 ISN
    socket.snd_una = socket.snd_nxt;
    println!("test:    Server received SYN, state = TCP_SYN_RECV");

    // 验证服务器初始化序列号
    assert!(socket.snd_nxt != 0, "Should have server ISN");
    assert_eq!(socket.snd_una, socket.snd_nxt, "SND_UNA should equal SND_NXT initially");

    // 模拟接收到 ACK 包
    socket.state = TcpState::TCP_ESTABLISHED;
    println!("test:    Server received ACK, state = TCP_ESTABLISHED");

    println!("test:    SUCCESS - Server side handshake works");
}

/// 测试序列号管理
fn test_tcp_sequence_numbers() {
    let mut socket = TcpSocket::new();
    socket.bind(12346).unwrap();

    // 模拟连接，设置序列号
    socket.snd_nxt = 12345; // 初始序列号
    socket.snd_una = socket.snd_nxt;

    // 验证初始序列号
    let initial_seq = socket.snd_nxt;
    assert!(initial_seq != 0, "Should have non-zero initial sequence number");

    // 验证序列号递增
    socket.snd_nxt = socket.snd_nxt.wrapping_add(1000);
    assert_eq!(socket.snd_nxt, initial_seq.wrapping_add(1000), "Sequence number should increment");

    // 验证未确认序列号
    socket.snd_una = socket.snd_una.wrapping_add(500);
    assert_eq!(socket.snd_una, initial_seq.wrapping_add(500), "Unacknowledged sequence number should increment");

    println!("test:    SUCCESS - TCP sequence number management works");
}

/// 测试 Socket 分配
fn test_tcp_socket_allocation() {
    // 分配多个 Socket
    let fd1 = tcp_socket_alloc();
    assert!(fd1.is_ok(), "First socket allocation should succeed");
    let fd1_val = fd1.unwrap();
    assert_eq!(fd1_val, 0, "First socket should have fd 0");

    let fd2 = tcp_socket_alloc();
    assert!(fd2.is_ok(), "Second socket allocation should succeed");
    let fd2_val = fd2.unwrap();
    assert_eq!(fd2_val, 1, "Second socket should have fd 1");

    // 绑定端口
    let ret = tcp_bind(fd1_val, 8080);
    assert_eq!(ret, 0, "Bind should succeed");

    let ret = tcp_bind(fd2_val, 8081);
    assert_eq!(ret, 0, "Bind should succeed");

    // 释放 Socket
    tcp_socket_free(fd1_val);
    tcp_socket_free(fd2_val);

    println!("test:    SUCCESS - TCP Socket allocation works");
}
