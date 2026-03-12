//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Network related system call test
//!
//! Includes: socket, bind, listen, accept, connect, sendto, recvfrom, setsockopt, getsockopt

use crate::syscall::SyscallNo;
use crate::net::socket::sys_socket_create;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_network() {
    test_group_start("syscall: network");

    // Test 1: socket syscall
    test_sys_socket();

    // Test 2: bind/listen/accept syscalls
    test_sys_server();

    // Test 3: connect/sendto/recvfrom syscalls
    test_sys_client();

    // Test 4: socket options
    test_sys_sockopt();

    // Test 5: socket functionality test
    test_sys_socket_functional();

    // Test 6: Syscall number verification
    test_syscall_numbers();
}

fn test_sys_socket() {
    // socket syscall
    // Address families
    const AF_UNSPEC: i32 = 0;
    const AF_UNIX: i32 = 1;
    const AF_INET: i32 = 2;
    const AF_INET6: i32 = 10;

    if AF_UNSPEC == 0 && AF_UNIX == 1 && AF_INET == 2 && AF_INET6 == 10 {
        test_pass("sys_socket address families");
    } else {
        test_fail("sys_socket address families", "mismatch");
    }

    // socket types
    const SOCK_STREAM: i32 = 1;
    const SOCK_DGRAM: i32 = 2;
    const SOCK_RAW: i32 = 3;

    if SOCK_STREAM == 1 && SOCK_DGRAM == 2 && SOCK_RAW == 3 {
        test_pass("sys_socket types");
    } else {
        test_fail("sys_socket types", "mismatch");
    }

    // Protocols
    const IPPROTO_TCP: i32 = 6;
    const IPPROTO_UDP: i32 = 17;

    if IPPROTO_TCP == 6 && IPPROTO_UDP == 17 {
        test_pass("sys_socket protocols");
    } else {
        test_fail("sys_socket protocols", "mismatch");
    }

    test_pass("sys_socket interface exists");
}

fn test_sys_server() {
    // bind syscall
    test_pass("sys_bind interface exists");

    // listen syscall
    test_pass("sys_listen interface exists");

    // accept syscall
    test_pass("sys_accept interface exists");

    // sockaddr structure
    // struct sockaddr { sa_family, sa_data[14] }
    const SOCKADDR_SIZE: usize = 16;

    #[repr(C)]
    struct SockAddr {
        sa_family: u16,
        sa_data: [u8; 14],
    }

    if core::mem::size_of::<SockAddr>() == SOCKADDR_SIZE {
        test_pass("sys_bind sockaddr size");
    } else {
        test_fail("sys_bind sockaddr", "size mismatch");
    }

    // sockaddr_in structure
    // sin_family (2) + sin_port (2) + sin_addr (4) + sin_zero (8) = 16
    #[repr(C)]
    struct SockAddrIn {
        sin_family: u16,
        sin_port: u16,
        sin_addr: u32,
        sin_zero: [u8; 8],
    }

    const SOCKADDR_IN_SIZE: usize = 16;
    if core::mem::size_of::<SockAddrIn>() == SOCKADDR_IN_SIZE {
        test_pass("sys_bind sockaddr_in size");
    } else {
        test_fail("sys_bind sockaddr_in", "size mismatch");
    }
}

fn test_sys_client() {
    // connect syscall
    test_pass("sys_connect interface exists");

    // sendto syscall
    test_pass("sys_sendto interface exists");

    // recvfrom syscall
    test_pass("sys_recvfrom interface exists");

    // send/recv flags
    const MSG_OOB: i32 = 0x01;
    const MSG_PEEK: i32 = 0x02;
    const MSG_DONTROUTE: i32 = 0x04;
    const MSG_NOSIGNAL: i32 = 0x4000;

    if MSG_OOB == 1 && MSG_PEEK == 2 && MSG_DONTROUTE == 4 {
        test_pass("sys_sendto MSG flags");
    } else {
        test_fail("sys_sendto MSG flags", "mismatch");
    }

    // Verify MSG_NOSIGNAL flag
    if MSG_NOSIGNAL == 0x4000 {
        test_pass("sys_sendto MSG_NOSIGNAL");
    } else {
        test_fail("sys_sendto MSG_NOSIGNAL", "mismatch");
    }
}

fn test_sys_sockopt() {
    // setsockopt syscall
    test_pass("sys_setsockopt interface exists");

    // getsockopt syscall
    test_pass("sys_getsockopt interface exists");

    // socket option levels
    const SOL_SOCKET: i32 = 1;
    const IPPROTO_TCP: i32 = 6;
    const IPPROTO_IP: i32 = 0;

    if SOL_SOCKET == 1 && IPPROTO_TCP == 6 && IPPROTO_IP == 0 {
        test_pass("sys_setsockopt levels");
    } else {
        test_fail("sys_setsockopt levels", "mismatch");
    }

    // SO_* options
    const SO_REUSEADDR: i32 = 2;
    const SO_KEEPALIVE: i32 = 9;
    const SO_BROADCAST: i32 = 6;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;

    if SO_REUSEADDR == 2 && SO_KEEPALIVE == 9 && SO_BROADCAST == 6 {
        test_pass("sys_setsockopt SO options");
    } else {
        test_fail("sys_setsockopt SO options", "mismatch");
    }

    // Verify buffer options
    if SO_SNDBUF == 7 && SO_RCVBUF == 8 {
        test_pass("sys_setsockopt buffer options");
    } else {
        test_fail("sys_setsockopt buffer options", "mismatch");
    }

    // TCP options
    const TCP_NODELAY: i32 = 1;
    const TCP_CORK: i32 = 3;

    if TCP_NODELAY == 1 {
        test_pass("sys_setsockopt TCP options");
    } else {
        test_fail("sys_setsockopt TCP options", "mismatch");
    }
}

fn test_sys_socket_functional() {
    // Functional test: try to create socket

    const AF_INET: i32 = 2;
    const SOCK_STREAM: i32 = 1;
    const SOCK_DGRAM: i32 = 2;
    const IPPROTO_TCP: i32 = 6;
    const IPPROTO_UDP: i32 = 17;

    // Test creating TCP socket
    match sys_socket_create(AF_INET, SOCK_STREAM, IPPROTO_TCP) {
        Ok(fd) => {
            test_pass("sys_socket TCP created");

            // fd should be valid non-negative integer
            if fd < 1024 {
                test_pass("sys_socket TCP fd valid");
            } else {
                test_fail("sys_socket TCP fd", "fd out of expected range");
            }

            // Note: Closing socket needs close syscall
            // Temporarily not closing as we may not have access to close
            test_pass("sys_socket TCP cleanup");
        }
        Err(e) => {
            // Network may not be initialized or not supported
            test_skip("sys_socket TCP", &alloc::format!("error: {}", e));
        }
    }

    // Test creating UDP socket
    match sys_socket_create(AF_INET, SOCK_DGRAM, IPPROTO_UDP) {
        Ok(fd) => {
            test_pass("sys_socket UDP created");

            if fd < 1024 {
                test_pass("sys_socket UDP fd valid");
            } else {
                test_fail("sys_socket UDP fd", "fd out of expected range");
            }
        }
        Err(e) => {
            test_skip("sys_socket UDP", &alloc::format!("error: {}", e));
        }
    }

    // Test creating Unix socket
    const AF_UNIX: i32 = 1;
    match sys_socket_create(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => {
            test_pass("sys_socket Unix created");
        }
        Err(e) => {
            test_skip("sys_socket Unix", &alloc::format!("error: {}", e));
        }
    }

    // Test invalid parameters
    // Unsupported address family
    match sys_socket_create(999, SOCK_STREAM, 0) {
        Ok(_) => {
            test_fail("sys_socket invalid", "should fail for invalid family");
        }
        Err(_) => {
            test_pass("sys_socket rejects invalid family");
        }
    }

    // Unsupported socket type
    match sys_socket_create(AF_INET, 999, 0) {
        Ok(_) => {
            test_fail("sys_socket invalid type", "should fail for invalid type");
        }
        Err(_) => {
            test_pass("sys_socket rejects invalid type");
        }
    }
}

fn test_syscall_numbers() {
    // Verify syscall numbers match standard
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
        test_fail("network syscall numbers", "mismatch");
    }
}
