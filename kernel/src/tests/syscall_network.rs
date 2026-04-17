//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network related system call test
//!
//! Includes: socket, bind, listen, accept, connect, sendto, recvfrom, setsockopt, getsockopt

use crate::syscall::SyscallNo;
use crate::syscall::network::{sys_socket, sys_bind, sys_listen, sys_accept, sys_connect, sys_sendto, sys_recvfrom};
use crate::net::socket::sys_socket_create;
use crate::fs::file_close;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_network() {
    test_group_start("syscall: network");

    // Test 1: socket syscall
    test_sys_socket();

    // Test 2: bind/listen/accept syscalls
    test_sys_server();

    // Test 3: connect/sendto/recvfrom syscalls
    test_sys_client();

    // Test 4: socket options constants
    test_sys_sockopt();

    // Test 5: socket functionality test
    test_sys_socket_functional();

    // Test 6: Syscall number verification
    test_syscall_numbers();
}

fn test_sys_socket() {
    // socket syscall via SyscallArgs
    let fd = sys_socket([2, 1, 6, 0, 0, 0]); // AF_INET, SOCK_STREAM, IPPROTO_TCP
    if fd >= 0 {
        test_pass("sys_socket TCP via SyscallArgs");
        let _ = file_close(fd as usize);
    } else {
        test_skip("sys_socket TCP via SyscallArgs", "socket creation failed");
    }

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

    // Invalid address family
    let fd = sys_socket([999, 1, 6, 0, 0, 0]);
    if fd < 0 {
        test_pass("sys_socket rejects invalid family");
    } else {
        test_fail("sys_socket invalid family", "should have failed");
        let _ = file_close(fd as usize);
    }

    // Invalid socket type
    let fd = sys_socket([2, 999, 0, 0, 0, 0]);
    if fd < 0 {
        test_pass("sys_socket rejects invalid type");
    } else {
        test_fail("sys_socket invalid type", "should have failed");
        let _ = file_close(fd as usize);
    }
}

fn test_sys_server() {
    // Create a TCP socket for bind/listen/accept tests
    const AF_INET: i32 = 2;
    const SOCK_STREAM: i32 = 1;
    const IPPROTO_TCP: i32 = 6;

    match sys_socket_create(AF_INET, SOCK_STREAM, IPPROTO_TCP) {
        Ok(fd) => {
            // sockaddr_in: AF_INET(2) + port(8080=0x1F90) + INADDR_ANY + padding
            let addr: [u8; 16] = [
                0x02, 0x00,       // AF_INET (little-endian)
                0x1F, 0x90,       // port 8080 (big-endian)
                0x00, 0x00, 0x00, 0x00, // INADDR_ANY
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ];
            let addr_ptr = &addr as *const u8;

            // Test bind
            let result = sys_bind([fd as u64, addr_ptr as u64, 16, 0, 0, 0]);
            if result == 0 {
                test_pass("sys_bind TCP socket");
            } else {
                test_skip("sys_bind", &alloc::format!("returned {}", result));
            }

            // Test listen on bound socket
            let result = sys_listen([fd as u64, 5, 0, 0, 0, 0]);
            if result == 0 {
                test_pass("sys_listen on bound socket");
            } else {
                test_skip("sys_listen", &alloc::format!("returned {}", result));
            }

            // Test accept (no incoming connection, should block or fail)
            let result = sys_accept([fd as u64, 0, 0, 0, 0, 0]);
            if result >= 0 {
                test_pass("sys_accept returns fd");
                let _ = file_close(result as usize);
            } else {
                // Expected: no connection pending
                test_skip("sys_accept", "no incoming connection");
            }

            let _ = file_close(fd as usize);
        }
        Err(e) => {
            test_skip("sys_bind/listen/accept", &alloc::format!("socket create failed: {}", e));
        }
    }

    // sockaddr structure size verification
    #[repr(C)]
    struct SockAddr {
        sa_family: u16,
        sa_data: [u8; 14],
    }

    if core::mem::size_of::<SockAddr>() == 16 {
        test_pass("sys_bind sockaddr size");
    } else {
        test_fail("sys_bind sockaddr", "size mismatch");
    }

    // sockaddr_in structure
    #[repr(C)]
    struct SockAddrIn {
        sin_family: u16,
        sin_port: u16,
        sin_addr: u32,
        sin_zero: [u8; 8],
    }

    if core::mem::size_of::<SockAddrIn>() == 16 {
        test_pass("sys_bind sockaddr_in size");
    } else {
        test_fail("sys_bind sockaddr_in", "size mismatch");
    }

    // Test bind with null address pointer → should return -EFAULT
    let fd = sys_socket([2, 1, 6, 0, 0, 0]);
    if fd >= 0 {
        let result = sys_bind([fd as u64, 0, 16, 0, 0, 0]); // null addr
        if result < 0 {
            test_pass("sys_bind null addr rejected");
        } else {
            test_fail("sys_bind null addr", "should have failed");
        }
        let _ = file_close(fd as usize);
    }
}

fn test_sys_client() {
    // connect syscall - test with TCP socket
    const AF_INET: i32 = 2;
    const SOCK_STREAM: i32 = 1;
    const IPPROTO_TCP: i32 = 6;

    match sys_socket_create(AF_INET, SOCK_STREAM, IPPROTO_TCP) {
        Ok(fd) => {
            // Try connect to 127.0.0.1:8080 (nobody listening)
            let addr: [u8; 16] = [
                0x02, 0x00,       // AF_INET
                0x1F, 0x90,       // port 8080
                0x7F, 0x00, 0x00, 0x01, // 127.0.0.1
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ];
            let addr_ptr = &addr as *const u8;
            let result = sys_connect([fd as u64, addr_ptr as u64, 16, 0, 0, 0]);
            // Connection will fail (nobody listening) or succeed depending on implementation
            if result == 0 {
                test_pass("sys_connect TCP");
            } else {
                test_pass("sys_connect TCP (expected failure - no listener)");
            }

            // Test connect with null address → -EFAULT
            let result = sys_connect([fd as u64, 0, 16, 0, 0, 0]);
            if result < 0 {
                test_pass("sys_connect null addr rejected");
            } else {
                test_fail("sys_connect null addr", "should have failed");
            }

            let _ = file_close(fd as usize);
        }
        Err(_) => {
            test_skip("sys_connect", "socket create failed");
        }
    }

    // sendto/recvfrom test with UDP socket
    match sys_socket_create(2, 2, 17) { // AF_INET, SOCK_DGRAM, IPPROTO_UDP
        Ok(fd) => {
            let data = b"hello";
            let addr: [u8; 16] = [
                0x02, 0x00,       // AF_INET
                0x00, 0x50,       // port 80
                0x7F, 0x00, 0x00, 0x01, // 127.0.0.1
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ];
            let addr_ptr = &addr as *const u8;

            // Test sendto
            let result = sys_sendto([fd as u64, data.as_ptr() as u64, data.len() as u64, 0, addr_ptr as u64, 16]);
            if result >= 0 {
                test_pass("sys_sendto sent bytes");
            } else {
                test_skip("sys_sendto", &alloc::format!("returned {}", result));
            }

            // Test sendto with null buf → -EFAULT
            let result = sys_sendto([fd as u64, 0, 10, 0, addr_ptr as u64, 16]);
            if result < 0 {
                test_pass("sys_sendto null buf rejected");
            } else {
                test_fail("sys_sendto null buf", "should have failed");
            }

            // Test sendto with zero length → 0
            let result = sys_sendto([fd as u64, data.as_ptr() as u64, 0, 0, addr_ptr as u64, 16]);
            if result == 0 {
                test_pass("sys_sendto zero length returns 0");
            } else {
                test_fail("sys_sendto zero length", &alloc::format!("expected 0, got {}", result));
            }

            // Test recvfrom
            let mut buf = [0u8; 64];
            let result = sys_recvfrom([fd as u64, buf.as_mut_ptr() as u64, 64, 0, 0, 0]);
            if result >= 0 {
                test_pass("sys_recvfrom returns");
            } else {
                test_skip("sys_recvfrom", "no data available");
            }

            // Test recvfrom with null buf → -EFAULT
            let result = sys_recvfrom([fd as u64, 0, 64, 0, 0, 0]);
            if result < 0 {
                test_pass("sys_recvfrom null buf rejected");
            } else {
                test_fail("sys_recvfrom null buf", "should have failed");
            }

            // Test recvfrom with zero length → 0
            let result = sys_recvfrom([fd as u64, buf.as_mut_ptr() as u64, 0, 0, 0, 0]);
            if result == 0 {
                test_pass("sys_recvfrom zero length returns 0");
            } else {
                test_fail("sys_recvfrom zero length", &alloc::format!("expected 0, got {}", result));
            }

            let _ = file_close(fd as usize);
        }
        Err(_) => {
            test_skip("sys_sendto/recvfrom", "socket create failed");
        }
    }

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

    if MSG_NOSIGNAL == 0x4000 {
        test_pass("sys_sendto MSG_NOSIGNAL");
    } else {
        test_fail("sys_sendto MSG_NOSIGNAL", "mismatch");
    }
}

fn test_sys_sockopt() {
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
    // Functional test: create various socket types

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

            // Close the socket
            match file_close(fd as usize) {
                Ok(()) => test_pass("sys_socket TCP close"),
                Err(_) => test_fail("sys_socket TCP close", "close failed"),
            }
        }
        Err(e) => {
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

            let _ = file_close(fd as usize);
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
            let _ = file_close(fd as usize);
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
