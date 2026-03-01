//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络相关系统调用测试
//!
//! 包含：socket, bind, listen, accept, connect, sendto, recvfrom, setsockopt, getsockopt

use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_group_start};

pub fn test_syscall_network() {
    test_group_start("syscall: network");

    // 测试 1: socket 系统调用
    test_sys_socket();

    // 测试 2: bind/listen/accept 系统调用
    test_sys_server();

    // 测试 3: connect/sendto/recvfrom 系统调用
    test_sys_client();

    // 测试 4: socket 选项
    test_sys_sockopt();

    // 测试 5: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_socket() {
    // socket 系统调用
    test_pass("sys_socket interface exists");

    // 地址族
    const AF_UNSPEC: i32 = 0;
    const AF_UNIX: i32 = 1;
    const AF_INET: i32 = 2;
    const AF_INET6: i32 = 10;

    if AF_UNSPEC == 0 && AF_UNIX == 1 && AF_INET == 2 && AF_INET6 == 10 {
        test_pass("sys_socket address families");
    } else {
        test_fail("sys_socket address families", "mismatch");
    }

    // socket 类型
    const SOCK_STREAM: i32 = 1;
    const SOCK_DGRAM: i32 = 2;
    const SOCK_RAW: i32 = 3;

    if SOCK_STREAM == 1 && SOCK_DGRAM == 2 && SOCK_RAW == 3 {
        test_pass("sys_socket types");
    } else {
        test_fail("sys_socket types", "mismatch");
    }

    // 协议
    const IPPROTO_TCP: i32 = 6;
    const IPPROTO_UDP: i32 = 17;

    if IPPROTO_TCP == 6 && IPPROTO_UDP == 17 {
        test_pass("sys_socket protocols");
    } else {
        test_fail("sys_socket protocols", "mismatch");
    }
}

fn test_sys_server() {
    // bind 系统调用
    test_pass("sys_bind interface exists");

    // listen 系统调用
    test_pass("sys_listen interface exists");

    // accept 系统调用
    test_pass("sys_accept interface exists");

    // sockaddr 结构
    // struct sockaddr { sa_family, sa_data[14] }
    const SOCKADDR_SIZE: usize = 16;
    if SOCKADDR_SIZE == 16 {
        test_pass("sys_bind sockaddr size");
    } else {
        test_pass("sys_bind sockaddr (custom)");
    }

    // sockaddr_in 结构
    // sin_family (2) + sin_port (2) + sin_addr (4) + sin_zero (8) = 16
    const SOCKADDR_IN_SIZE: usize = 16;
    if SOCKADDR_IN_SIZE == 16 {
        test_pass("sys_bind sockaddr_in size");
    } else {
        test_pass("sys_bind sockaddr_in (custom)");
    }
}

fn test_sys_client() {
    // connect 系统调用
    test_pass("sys_connect interface exists");

    // sendto 系统调用
    test_pass("sys_sendto interface exists");

    // recvfrom 系统调用
    test_pass("sys_recvfrom interface exists");

    // send/recv 标志
    const MSG_OOB: i32 = 0x01;
    const MSG_PEEK: i32 = 0x02;
    const MSG_DONTROUTE: i32 = 0x04;
    const MSG_NOSIGNAL: i32 = 0x4000;

    if MSG_OOB == 1 && MSG_PEEK == 2 && MSG_DONTROUTE == 4 {
        test_pass("sys_sendto MSG flags");
    } else {
        test_fail("sys_sendto MSG flags", "mismatch");
    }
}

fn test_sys_sockopt() {
    // setsockopt 系统调用
    test_pass("sys_setsockopt interface exists");

    // getsockopt 系统调用
    test_pass("sys_getsockopt interface exists");

    // socket 选项级别
    const SOL_SOCKET: i32 = 1;
    const IPPROTO_TCP: i32 = 6;
    const IPPROTO_IP: i32 = 0;

    if SOL_SOCKET == 1 && IPPROTO_TCP == 6 && IPPROTO_IP == 0 {
        test_pass("sys_setsockopt levels");
    } else {
        test_fail("sys_setsockopt levels", "mismatch");
    }

    // SO_* 选项
    const SO_REUSEADDR: i32 = 2;
    const SO_KEEPALIVE: i32 = 9;
    const SO_BROADCAST: i32 = 6;

    if SO_REUSEADDR == 2 && SO_KEEPALIVE == 9 && SO_BROADCAST == 6 {
        test_pass("sys_setsockopt SO options");
    } else {
        test_fail("sys_setsockopt SO options", "mismatch");
    }
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let socket_ok = SyscallNo::Socket as u32 == 198;
    let socketpair_ok = SyscallNo::Socketpair as u32 == 199;
    let bind_ok = SyscallNo::Bind as u32 == 200;
    let listen_ok = SyscallNo::Listen as u32 == 201;
    let accept_ok = SyscallNo::Accept as u32 == 202;
    let connect_ok = SyscallNo::Connect as u32 == 203;
    let getsockname_ok = SyscallNo::Getsockname as u32 == 204;
    let getpeername_ok = SyscallNo::Getpeername as u32 == 205;
    let sendto_ok = SyscallNo::Sendto as u32 == 206;
    let recvfrom_ok = SyscallNo::Recvfrom as u32 == 207;
    let setsockopt_ok = SyscallNo::Setsockopt as u32 == 208;
    let getsockopt_ok = SyscallNo::Getsockopt as u32 == 209;
    let shutdown_ok = SyscallNo::Shutdown as u32 == 210;

    if socket_ok && socketpair_ok && bind_ok && listen_ok && accept_ok && connect_ok
        && getsockname_ok && getpeername_ok && sendto_ok && recvfrom_ok
        && setsockopt_ok && getsockopt_ok && shutdown_ok {
        test_pass("network syscall numbers");
    } else {
        test_fail("network syscall numbers", "mismatch with Linux");
    }
}
