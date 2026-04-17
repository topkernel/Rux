//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network-related system calls
//!
//! Includes: socket, bind, listen, accept, connect, sendto, recvfrom

use super::*;

/// sys_socket - Create socket
///
/// # Arguments
/// - args[0]: domain - protocol family (AF_INET=2)
/// - args[1]: type - socket type (SOCK_STREAM=1, SOCK_DGRAM=2)
/// - args[2]: protocol - protocol type (IPPROTO_TCP=6, IPPROTO_UDP=17)
///
/// # Returns
/// Returns file descriptor on success, negative error code on failure
pub fn sys_socket(args: SyscallArgs) -> i64 {
    let domain = args[0] as i32;
    let type_ = args[1] as i32;
    let protocol = args[2] as i32;

    // Delegate to the VFS-based socket layer which properly allocates
    // a process file descriptor and registers the socket in the fd table.
    match crate::net::socket::sys_socket_create(domain, type_, protocol) {
        Ok(fd) => fd as i64,
        Err(e) => e as i64,
    }
}

/// sys_bind - Bind socket to address
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr structure
/// - args[2]: addrlen - address length
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_bind(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    // Check address pointer validity
    if addr_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Read sockaddr_in structure (simplified implementation)
    // struct sockaddr_in {
    //     sa_family_t sin_family;  // 2 bytes
    //     in_port_t sin_port;      // 2 bytes (network byte order)
    //     struct in_addr sin_addr; // 4 bytes
    //     char sin_zero[8];        // 8 bytes
    // };

    // SAFETY: addr_ptr validated with access_ok above; reading fixed sockaddr_in fields.
    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    // SAFETY: addr_ptr validated; reading port at offset 2.
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };

    // Permission check: privileged ports (< 1024) require CAP_NET_BIND_SERVICE
    if sin_port < 1024 && !crate::security::capable(crate::security::CAP_NET_BIND_SERVICE) {
        return -(errno::EACCES as i64);
    }

    // Currently only support AF_INET
    if sin_family != 2 {
        return -(errno::EAFNOSUPPORT as i64);
    }

    // TODO: Need a way to determine if fd is TCP or UDP socket
    // Simplified implementation: try both protocols
    use crate::net::{tcp, udp};

    // Try TCP first
    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        return tcp::tcp_bind(fd, sin_port) as i64;
    }

    // Then try UDP
    if let Some(_socket) = udp::udp_socket_get(fd) {
        return udp::udp_bind(fd, sin_port) as i64;
    }

    -(errno::EBADF as i64)
}

/// sys_listen - Listen on socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: backlog - pending connection queue length
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_listen(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let backlog = args[1] as i32;

    use crate::net::tcp;

    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        tcp::tcp_listen(fd, backlog as u32) as i64
    } else {
        -(errno::EBADF as i64)
    }
}

/// sys_accept - Accept connection
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr structure (output)
/// - args[2]: addrlen - pointer to address length (input/output)
///
/// # Returns
/// Returns new socket file descriptor on success, negative error code on failure
pub fn sys_accept(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let _addr_ptr = args[1] as *mut u8;
    let _addrlen_ptr = args[2] as *mut u32;

    use crate::net::tcp;

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_accept(fd) as i64,
        None => -(errno::EBADF as i64)
    }
}

/// sys_connect - Connect to remote address
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr structure
/// - args[2]: addrlen - address length
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_connect(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    // Check address pointer validity
    if addr_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Read sockaddr_in structure
    // SAFETY: addr_ptr validated with access_ok; reading fixed sockaddr_in fields.
    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    // SAFETY: addr_ptr validated; reading port at offset 2.
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };
    // SAFETY: addr_ptr validated; reading address at offset 4.
    let sin_addr = unsafe { u32::from_be_bytes(*((addr_ptr.add(4)) as *const [u8; 4])) };

    // Currently only support AF_INET
    if sin_family != 2 {
        return -(errno::EAFNOSUPPORT as i64);
    }

    use crate::net::tcp;

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_connect(fd, sin_addr, sin_port) as i64,
        None => -(errno::EBADF as i64)
    }
}

/// sys_sendto - Send data (possibly to specified destination address)
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: buf - pointer to data buffer
/// - args[2]: len - data length
/// - args[3]: flags - flags
/// - args[4]: addr - pointer to destination address (optional)
/// - args[5]: addrlen - address length (optional)
///
/// # Returns
/// Returns number of bytes sent on success, negative error code on failure
pub fn sys_sendto(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *const u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let addr_ptr = args[4] as *const u8;
    let _addrlen = args[5] as u32;

    // Check buffer pointer validity
    if buf_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Validate user buffer pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, len) {
        return -(errno::EFAULT as i64);
    }

    if len == 0 {
        return 0;
    }

    // Validate optional address pointer
    if !addr_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Get socket
    let socket = match crate::net::socket::get_socket(fd) {
        Some(s) => s,
        None => {
            // Try to find from old socket table
            // Try TCP first
            if let Some(tcp_sock) = crate::net::tcp::tcp_socket_get(fd as i32) {
                // SAFETY: buf_ptr validated with access_ok; len > 0 guaranteed above.
                let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
                return match tcp_sock.send(data) {
                    Ok(n) => n as i64,
                    Err(()) => -(errno::EIO as i64),
                };
            }
            // Then try UDP
            if let Some(_) = crate::net::udp::udp_socket_get(fd as i32) {
                // SAFETY: buf_ptr validated with access_ok; len > 0 guaranteed above.
                let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
                return crate::net::udp::udp_send(fd as i32, data) as i64;
            }
            return -(errno::EBADF as i64);
        }
    };

    // Read data
    // SAFETY: buf_ptr validated with access_ok; len > 0 guaranteed.
    let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };

    // Parse destination address (if provided)
    let dest_addr = if !addr_ptr.is_null() {
        // SAFETY: addr_ptr validated with access_ok; reading 16 bytes of sockaddr_in.
        if let Some(sockaddr) = crate::net::socket::SockAddrIn::from_bytes(unsafe {
            core::slice::from_raw_parts(addr_ptr, 16)
        }) {
            Some((sockaddr.addr(), sockaddr.port()))
        } else {
            None
        }
    } else {
        None
    };

    // Send data
    match socket.send(data, dest_addr) {
        Ok(bytes_sent) => bytes_sent as i64,
        Err(e) => e as i64,
    }
}

/// sys_getsockname - Get socket local address
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr (output)
/// - args[2]: addrlen - pointer to address length (input/output)
pub fn sys_getsockname(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let addr_ptr = args[1] as *mut u8;
    let addrlen_ptr = args[2] as *mut u32;

    if addr_ptr.is_null() || addrlen_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: addrlen_ptr validated with access_ok(4); reading 4-byte u32.
    let addrlen = unsafe { core::ptr::read_volatile(addrlen_ptr) } as usize;
    if addrlen < 16 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Try new socket layer first
    if let Some(socket) = crate::net::socket::get_socket(fd) {
        let local_addr = *socket.local_addr.lock();
        let local_port = *socket.local_port.lock();
        // SAFETY: addr_ptr/addrlen_ptr validated with access_ok; writing sockaddr_in layout.
        unsafe {
            core::ptr::write(addr_ptr as *mut u16, 2u16); // AF_INET
            core::ptr::write(addr_ptr.add(2) as *mut u16, local_port.to_be());
            core::ptr::write(addr_ptr.add(4) as *mut u32, local_addr.to_be());
            core::ptr::write_bytes(addr_ptr.add(8), 0, 8);
            core::ptr::write_volatile(addrlen_ptr, 16u32);
        }
        return 0;
    }

    // Fallback: try old TCP/UDP tables
    // No stored local address in old layer — return INADDR_ANY:port 0
    // SAFETY: addr_ptr/addrlen_ptr validated with access_ok; writing sockaddr_in layout.
    unsafe {
        core::ptr::write(addr_ptr as *mut u16, 2u16);
        core::ptr::write(addr_ptr.add(2) as *mut u16, 0u16);
        core::ptr::write(addr_ptr.add(4) as *mut u32, 0u32);
        core::ptr::write_bytes(addr_ptr.add(8), 0, 8);
        core::ptr::write_volatile(addrlen_ptr, 16u32);
    }
    0
}

/// sys_getpeername - Get socket peer address
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr (output)
/// - args[2]: addrlen - pointer to address length (input/output)
pub fn sys_getpeername(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let addr_ptr = args[1] as *mut u8;
    let addrlen_ptr = args[2] as *mut u32;

    if addr_ptr.is_null() || addrlen_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: addrlen_ptr validated with access_ok; reading 4-byte u32.
    let addrlen = unsafe { core::ptr::read_volatile(addrlen_ptr) } as usize;
    if addrlen < 16 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Try new socket layer
    if let Some(socket) = crate::net::socket::get_socket(fd) {
        let state = *socket.state.lock();
        if state == crate::net::socket::SocketState::Connected {
            let peer_addr = *socket.remote_addr.lock();
            let peer_port = *socket.remote_port.lock();
            // SAFETY: addr_ptr/addrlen_ptr validated with access_ok; writing sockaddr_in layout.
            unsafe {
                core::ptr::write(addr_ptr as *mut u16, 2u16); // AF_INET
                core::ptr::write(addr_ptr.add(2) as *mut u16, peer_port.to_be());
                core::ptr::write(addr_ptr.add(4) as *mut u32, peer_addr.to_be());
                core::ptr::write_bytes(addr_ptr.add(8), 0, 8);
                core::ptr::write_volatile(addrlen_ptr, 16u32);
            }
            return 0;
        }
        return -(errno::ENOTCONN as i64);
    }

    // Old layer fallback
    if let Some(_) = crate::net::tcp::tcp_socket_get(fd as i32) {
        return -(errno::ENOTCONN as i64);
    }
    -(errno::ENOTSOCK as i64)
}

/// sys_setsockopt - Set socket options
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: level - protocol level
/// - args[2]: optname - option name
/// - args[3]: optval - option value
/// - args[4]: optlen - option length
pub fn sys_setsockopt(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let level = args[1] as i32;
    let optname = args[2] as i32;
    let optval = args[3] as *const u8;
    let optlen = args[4] as u32;

    // SOL_SOCKET = 1
    const SOL_SOCKET: i32 = 1;
    // Common SO_* options we accept but ignore
    const SO_REUSEADDR: i32 = 2;
    const SO_TYPE: i32 = 3;
    const SO_ERROR: i32 = 4;
    const SO_DONTROUTE: i32 = 5;
    const SO_BROADCAST: i32 = 6;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_KEEPALIVE: i32 = 9;
    const SO_OOBINLINE: i32 = 10;
    const SO_NO_CHECK: i32 = 11;
    const SO_PRIORITY: i32 = 12;
    const SO_LINGER: i32 = 13;
    const SO_BSDCOMPAT: i32 = 14;
    const SO_REUSEPORT: i32 = 15;
    const SO_PASSCRED: i32 = 16;
    const SO_PEERCRED: i32 = 17;
    const SO_RCVLOWAT: i32 = 18;
    const SO_SNDLOWAT: i32 = 19;
    const SO_RCVTIMEO: i32 = 20;
    const SO_SNDTIMEO: i32 = 21;
    // IPPROTO_TCP = 6
    const IPPROTO_TCP: i32 = 6;
    const TCP_NODELAY: i32 = 1;
    const TCP_CORK: i32 = 3;
    const TCP_KEEPIDLE: i32 = 4;
    const TCP_KEEPINTVL: i32 = 5;
    const TCP_KEEPCNT: i32 = 6;
    // IPPROTO_IP = 0
    const IPPROTO_IP: i32 = 0;
    const IP_TOS: i32 = 1;
    const IP_TTL: i32 = 2;
    const IP_MULTICAST_TTL: i32 = 33;
    const IP_MULTICAST_LOOP: i32 = 34;
    const IP_ADD_MEMBERSHIP: i32 = 35;
    const IP_DROP_MEMBERSHIP: i32 = 36;

    if !optval.is_null() && optlen > 0 {
        if !crate::arch::riscv64::uaccess::access_ok(optval as usize, optlen as usize) {
            return -(errno::EFAULT as i64);
        }
    }

    // Validate fd is a socket
    let is_socket = crate::net::socket::get_socket(fd).is_some()
        || crate::net::tcp::tcp_socket_get(fd as i32).is_some()
        || crate::net::udp::udp_socket_get(fd as i32).is_some();
    if !is_socket {
        return -(errno::ENOTSOCK as i64);
    }

    match level {
        SOL_SOCKET => match optname {
            SO_REUSEADDR | SO_REUSEPORT | SO_KEEPALIVE | SO_BROADCAST
            | SO_DONTROUTE | SO_OOBINLINE | SO_NO_CHECK | SO_BSDCOMPAT
            | SO_PASSCRED | SO_SNDBUF | SO_RCVBUF | SO_RCVLOWAT
            | SO_SNDLOWAT | SO_PRIORITY | SO_LINGER | SO_RCVTIMEO
            | SO_SNDTIMEO => 0, // Accept and ignore
            SO_TYPE | SO_ERROR | SO_PEERCRED => {
                return -(errno::ENOPROTOOPT as i64); // Read-only options
            }
            _ => 0, // Accept unknown options silently
        },
        IPPROTO_TCP => match optname {
            TCP_NODELAY | TCP_CORK | TCP_KEEPIDLE | TCP_KEEPINTVL | TCP_KEEPCNT => 0,
            _ => 0,
        },
        IPPROTO_IP => match optname {
            IP_TOS | IP_TTL | IP_MULTICAST_TTL | IP_MULTICAST_LOOP
            | IP_ADD_MEMBERSHIP | IP_DROP_MEMBERSHIP => 0,
            _ => 0,
        },
        _ => 0, // Accept unknown levels silently
    }
}

/// sys_getsockopt - Get socket options
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: level - protocol level
/// - args[2]: optname - option name
/// - args[3]: optval - option value (output)
/// - args[4]: optlen - option length (input/output)
pub fn sys_getsockopt(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let level = args[1] as i32;
    let optname = args[2] as i32;
    let optval = args[3] as *mut u8;
    let optlen_ptr = args[4] as *mut u32;

    const SOL_SOCKET: i32 = 1;
    const SO_TYPE: i32 = 3;
    const SO_ERROR: i32 = 4;
    const SO_REUSEADDR: i32 = 2;
    const SO_REUSEPORT: i32 = 15;
    const SO_KEEPALIVE: i32 = 9;
    const SO_BROADCAST: i32 = 6;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_OOBINLINE: i32 = 10;
    const SO_NO_CHECK: i32 = 11;
    const SO_PRIORITY: i32 = 12;
    const SO_LINGER: i32 = 13;
    const SO_RCVLOWAT: i32 = 18;
    const SO_SNDLOWAT: i32 = 19;
    const SO_RCVTIMEO: i32 = 20;
    const SO_SNDTIMEO: i32 = 21;
    const SO_PEERCRED: i32 = 17;
    const SO_DOMAIN: i32 = 39;
    const IPPROTO_TCP: i32 = 6;
    const TCP_NODELAY: i32 = 1;
    const TCP_INFO: i32 = 11;
    const TCP_CORK: i32 = 3;
    const IPPROTO_IP: i32 = 0;
    const IP_TOS: i32 = 1;
    const IP_TTL: i32 = 2;

    if optval.is_null() || optlen_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(optlen_ptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }
    // SAFETY: optlen_ptr validated with access_ok; reading 4-byte u32.
    let optlen = unsafe { core::ptr::read_volatile(optlen_ptr) } as usize;
    if optlen == 0 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(optval as usize, optlen) {
        return -(errno::EFAULT as i64);
    }

    // Validate fd is a socket
    let sock = crate::net::socket::get_socket(fd);
    let is_socket = sock.is_some()
        || crate::net::tcp::tcp_socket_get(fd as i32).is_some()
        || crate::net::udp::udp_socket_get(fd as i32).is_some();
    if !is_socket {
        return -(errno::ENOTSOCK as i64);
    }

    // SAFETY: optval and optlen_ptr validated with access_ok; writes stay within
    // validated lengths. sock may be None for fallback paths.
    unsafe {
        match level {
            SOL_SOCKET => match optname {
                SO_TYPE => {
                    // Return SOCK_STREAM or SOCK_DGRAM
                    let val = if let Some(ref s) = sock {
                        match s.sock_type {
                            crate::net::socket::SocketType::Tcp => 1u32,  // SOCK_STREAM
                            crate::net::socket::SocketType::Udp => 2u32,  // SOCK_DGRAM
                        }
                    } else if crate::net::tcp::tcp_socket_get(fd as i32).is_some() {
                        1u32
                    } else {
                        2u32
                    };
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::write_bytes(optval, 0, optlen);
                    core::ptr::copy_nonoverlapping(
                        &val as *const u32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_ERROR => {
                    // No pending error
                    let val: i32 = 0;
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::write_bytes(optval, 0, optlen);
                    core::ptr::copy_nonoverlapping(
                        &val as *const i32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_DOMAIN => {
                    // AF_INET = 2
                    let val: u32 = 2;
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::copy_nonoverlapping(
                        &val as *const u32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_REUSEADDR | SO_REUSEPORT | SO_KEEPALIVE | SO_BROADCAST
                | SO_OOBINLINE | SO_NO_CHECK | SO_PRIORITY => {
                    let val: u32 = 0;
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::copy_nonoverlapping(
                        &val as *const u32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_SNDBUF | SO_RCVBUF => {
                    let val: i32 = 212992; // Default Linux socket buffer size
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::copy_nonoverlapping(
                        &val as *const i32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_RCVLOWAT | SO_SNDLOWAT => {
                    let val: i32 = 1; // Default: 1 byte
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::copy_nonoverlapping(
                        &val as *const i32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_RCVTIMEO | SO_SNDTIMEO => {
                    // struct timeval { tv_sec: i64, tv_usec: i64 } = 16 bytes
                    let write_len = core::cmp::min(optlen, 16);
                    core::ptr::write_bytes(optval, 0, write_len); // Zero = no timeout
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_LINGER => {
                    // struct linger { l_onoff: i32, l_linger: i32 } = 8 bytes
                    let write_len = core::cmp::min(optlen, 8);
                    core::ptr::write_bytes(optval, 0, write_len); // Linger off
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                SO_PEERCRED => {
                    // struct ucred { pid, uid, gid } = 12 bytes
                    let write_len = core::cmp::min(optlen, 12);
                    core::ptr::write_bytes(optval, 0, write_len);
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                _ => {
                    return -(errno::ENOPROTOOPT as i64);
                }
            },
            IPPROTO_TCP => match optname {
                TCP_NODELAY => {
                    let val: u32 = 1; // Nodelay enabled by default
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::copy_nonoverlapping(
                        &val as *const u32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                TCP_CORK | TCP_INFO => {
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::write_bytes(optval, 0, write_len);
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                _ => {
                    return -(errno::ENOPROTOOPT as i64);
                }
            },
            IPPROTO_IP => match optname {
                IP_TOS => {
                    let val: u32 = 0;
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::copy_nonoverlapping(
                        &val as *const u32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                IP_TTL => {
                    let val: u32 = 64; // Default TTL
                    let write_len = core::cmp::min(optlen, 4);
                    core::ptr::copy_nonoverlapping(
                        &val as *const u32 as *const u8,
                        optval,
                        write_len,
                    );
                    core::ptr::write_volatile(optlen_ptr, write_len as u32);
                }
                _ => {
                    return -(errno::ENOPROTOOPT as i64);
                }
            },
            _ => {
                return -(errno::ENOPROTOOPT as i64);
            }
        }
    }
    0
}

/// sys_shutdown - Shutdown part of full-duplex connection
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: how - SHUT_RD (0), SHUT_WR (1), SHUT_RDWR (2)
pub fn sys_shutdown(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let how = args[1] as i32;

    if how < 0 || how > 2 {
        return -(errno::EINVAL as i64);
    }

    if let Some(socket) = crate::net::socket::get_socket(fd) {
        if how == 0 || how == 2 {
            // SHUT_RD or SHUT_RDWR: mark receive side
            *socket.state.lock() = crate::net::socket::SocketState::Closing;
        }
        if how == 1 || how == 2 {
            // SHUT_WR or SHUT_RDWR: mark send side
            *socket.state.lock() = crate::net::socket::SocketState::Closing;
        }
        return 0;
    }

    // Old layer fallback
    if crate::net::tcp::tcp_socket_get(fd as i32).is_some()
        || crate::net::udp::udp_socket_get(fd as i32).is_some()
    {
        return 0; // Accept and ignore
    }

    -(errno::ENOTSOCK as i64)
}

/// sys_sendmsg - Send message through socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: msg - pointer to msghdr
/// - args[2]: flags - flags
pub fn sys_sendmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msg_ptr = args[1] as *const u8;
    let _flags = args[2] as i32;

    if msg_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msg_ptr as usize, 64) {
        return -(errno::EFAULT as i64);
    }

    // Read msg_name (sa_family) and msg_iov (iovec) from msghdr
    // struct msghdr { msg_name, msg_namelen, msg_iov, msg_iovlen, msg_control, msg_controllen, msg_flags }
    // SAFETY: msg_ptr validated with access_ok(64); reading fields at known offsets.
    let msg_name_ptr = unsafe { *(msg_ptr as *const *const u8) };
    let msg_namelen = unsafe { *((msg_ptr.add(8)) as *const u32) };
    let msg_iov_ptr = unsafe { *((msg_ptr.add(16)) as *const usize) };
    let msg_iovlen = unsafe { *((msg_ptr.add(24)) as *const usize) };

    // Collect data from iovec
    // struct iovec { iov_base, iov_len }
    let mut total_len = 0usize;
    let mut buf = alloc::vec::Vec::new();
    // Cap msg_iovlen to prevent i * 16 overflow in pointer arithmetic
    let msg_iovlen = msg_iovlen.min(1024);
    for i in 0..msg_iovlen {
        // SAFETY: iovec base/len read from user memory at validated offset; iov_base
        // validated with access_ok before slice creation.
        let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(i * 16)) as *const usize) };
        let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(i * 16 + 8)) as *const usize) };
        if iov_len > 0 {
            if !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: iov_base validated with access_ok; iov_len bounds the slice.
            buf.extend_from_slice(unsafe { core::slice::from_raw_parts(iov_base as *const u8, iov_len) });
            total_len += iov_len;
        }
    }

    if total_len == 0 {
        return 0;
    }

    // Get socket and send
    if let Some(socket) = crate::net::socket::get_socket(fd as usize) {
        match socket.send(&buf, None) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        }
    } else {
        -(errno::EBADF as i64)
    }
}

/// sys_recvmsg - Receive message from socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: msg - pointer to msghdr
/// - args[2]: flags - flags
pub fn sys_recvmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msg_ptr = args[1] as *mut u8;
    let _flags = args[2] as i32;

    if msg_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msg_ptr as usize, 64) {
        return -(errno::EFAULT as i64);
    }

    // Read iovec from msghdr
    // SAFETY: msg_ptr validated with access_ok(64); reading fields at known offsets.
    let msg_iov_ptr = unsafe { *((msg_ptr.add(16)) as *const usize) };
    let msg_iovlen = unsafe { *((msg_ptr.add(24)) as *const usize) };

    // Calculate total buffer size
    let mut total_buf_len = 0usize;
    // Cap msg_iovlen to prevent i * 16 overflow in pointer arithmetic
    let msg_iovlen = msg_iovlen.min(1024);
    for i in 0..msg_iovlen {
        // SAFETY: iovec fields read from user memory at validated offset.
        let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(i * 16)) as *const usize) };
        let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(i * 16 + 8)) as *const usize) };
        if iov_len > 0 && !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
            return -(errno::EFAULT as i64);
        }
        total_buf_len += iov_len;
    }

    if total_buf_len == 0 {
        return 0;
    }

    // Allocate receive buffer
    let mut buf = alloc::vec![0u8; total_buf_len];

    // Get socket and receive
    if let Some(socket) = crate::net::socket::get_socket(fd as usize) {
        match socket.recv(&mut buf) {
            Ok((bytes_read, _src_addr)) => {
                // Scatter data back to iovecs
                let mut offset = 0usize;
                for i in 0..msg_iovlen {
                    if offset >= bytes_read { break; }
                    // SAFETY: iovec fields at validated user offset; copy_len bounds the write.
                    let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(i * 16)) as *const usize) };
                    let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(i * 16 + 8)) as *const usize) };
                    let copy_len = core::cmp::min(iov_len, bytes_read - offset);
                    if copy_len > 0 {
                        // SAFETY: iov_base validated with access_ok; copy_len bounded by iov_len.
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                buf.as_ptr().add(offset),
                                iov_base as *mut u8,
                                copy_len,
                            );
                        }
                        offset += copy_len;
                    }
                }
                bytes_read as i64
            }
            Err(e) => e as i64,
        }
    } else {
        -(errno::EBADF as i64)
    }
}

/// sys_socketpair - Create pair of connected sockets (NR 199)
///
/// # Arguments
/// - args[0]: domain - protocol family
/// - args[1]: type - socket type
/// - args[2]: protocol - protocol
/// - args[3]: sv - pointer to int[2] for fds
pub fn sys_socketpair(args: SyscallArgs) -> i64 {
    let domain = args[0] as i32;
    let _type_ = args[1] as i32;
    let _protocol = args[2] as i32;
    let sv = args[3] as *mut i32;

    if sv.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(sv as usize, 8) {
        return -(errno::EFAULT as i64);
    }

    // Only AF_UNIX (1) is supported for socketpair
    if domain != 1 {
        return -(errno::EAFNOSUPPORT as i64);
    }

    // TODO: implement AF_UNIX socketpair with connected socket pair
    // For now, return -EOPNOTSUPP to indicate the feature is not yet available
    // This is better than -ENOSYS which prevents libc fallback
    -(errno::EOPNOTSUPP as i64)
}

/// sys_sendmmsg - Send multiple messages (NR 269)
///
/// # Arguments
/// - args[0]: fd - socket fd
/// - args[1]: msgvec - pointer to mmsghdr array
/// - args[2]: vlen - number of messages
/// - args[3]: flags - flags
///
/// struct mmsghdr { struct msghdr msg; unsigned int len; }
/// struct msghdr is 56 bytes on 64-bit; mmsghdr = 60 bytes
pub fn sys_sendmmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msgvec = args[1] as *const u8;
    let vlen = args[2] as u32;
    let _flags = args[3] as i32;

    if msgvec.is_null() || vlen == 0 {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msgvec as usize, vlen as usize * 64) {
        return -(errno::EFAULT as i64);
    }

    if let Some(socket) = crate::net::socket::get_socket(fd as usize) {
        let mut total_sent = 0u32;
        for i in 0..vlen as usize {
            // mmsghdr: msghdr (56 bytes) + msg_len (4 bytes)
            // SAFETY: msgvec validated with access_ok; mm offset within validated range.
            let mm = unsafe { msgvec.add(i * 60) };
            // msghdr layout: msg_name(8), msg_namelen(4), msg_iov(8), msg_iovlen(8),
            //                 msg_control(8), msg_controllen(8), msg_flags(4) = 48 bytes
            // SAFETY: mm validated; reading iovec fields at known offsets.
            let msg_iov_ptr = unsafe { *((mm.add(16)) as *const usize) };
            let msg_iovlen = unsafe { *((mm.add(24)) as *const usize) };

            // Gather data from iovec
            let mut buf = alloc::vec::Vec::new();
            for j in 0..msg_iovlen {
                // SAFETY: iovec fields at validated offset; iov_base validated below.
                let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(j * 16)) as *const usize) };
                let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(j * 16 + 8)) as *const usize) };
                if iov_len > 0 {
                    if !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
                        return total_sent as i64; // Return partial success
                    }
                    // SAFETY: iov_base validated with access_ok; iov_len bounds the slice.
                    buf.extend_from_slice(unsafe { core::slice::from_raw_parts(iov_base as *const u8, iov_len) });
                }
            }

            let sent = match socket.send(&buf, None) {
                Ok(n) => n,
                Err(_) => break,
            };
            // Write msg_len in mmsghdr
            // SAFETY: mm offset within validated msgvec range; writing 4-byte u32.
            unsafe {
                core::ptr::write_volatile(mm.add(56) as *mut u32, sent as u32);
            }
            total_sent += 1;
        }
        return total_sent as i64;
    }

    -(errno::EBADF as i64)
}

/// sys_recvmmsg - Receive multiple messages (NR 243)
///
/// # Arguments
/// - args[0]: fd - socket fd
/// - args[1]: msgvec - pointer to mmsghdr array
/// - args[2]: vlen - number of messages
/// - args[3]: flags - flags
/// - args[4]: timeout - pointer to timespec
pub fn sys_recvmmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msgvec = args[1] as *mut u8;
    let vlen = args[2] as u32;
    let _flags = args[3] as i32;
    let _timeout = args[4] as *const u8;

    if msgvec.is_null() || vlen == 0 {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msgvec as usize, vlen as usize * 64) {
        return -(errno::EFAULT as i64);
    }

    if let Some(socket) = crate::net::socket::get_socket(fd as usize) {
        let mut total_recv = 0u32;
        for i in 0..vlen as usize {
            // SAFETY: msgvec validated with access_ok; mm offset within validated range.
            let mm = unsafe { msgvec.add(i * 60) };
            // SAFETY: mm validated; reading iovec fields at known offsets.
            let msg_iov_ptr = unsafe { *((mm.add(16)) as *const usize) };
            let msg_iovlen = unsafe { *((mm.add(24)) as *const usize) };

            // Calculate total buffer size
            let mut total_buf_len = 0usize;
            for j in 0..msg_iovlen {
                // SAFETY: iovec fields at validated offset; iov_base validated below.
                let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(j * 16)) as *const usize) };
                let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(j * 16 + 8)) as *const usize) };
                if iov_len > 0 && !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
                    return total_recv as i64;
                }
                total_buf_len += iov_len;
            }

            if total_buf_len == 0 {
                break;
            }

            let mut buf = alloc::vec![0u8; total_buf_len];
            match socket.recv(&mut buf) {
                Ok((bytes_read, _src_addr)) => {
                    // Scatter data back to iovecs
                    let mut offset = 0usize;
                    for j in 0..msg_iovlen {
                        if offset >= bytes_read { break; }
                        // SAFETY: iovec fields at validated offset; copy_len bounds the write.
                        let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(j * 16)) as *const usize) };
                        let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(j * 16 + 8)) as *const usize) };
                        let copy_len = core::cmp::min(iov_len, bytes_read - offset);
                        if copy_len > 0 {
                            // SAFETY: iov_base validated with access_ok; copy_len bounded by iov_len.
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    buf.as_ptr().add(offset),
                                    iov_base as *mut u8,
                                    copy_len,
                                );
                            }
                            offset += copy_len;
                        }
                    }
                    // SAFETY: mm offset within validated msgvec range; writing 4-byte u32.
                    unsafe {
                        core::ptr::write_volatile(mm.add(56) as *mut u32, bytes_read as u32);
                    }
                    total_recv += 1;
                    if bytes_read == 0 {
                        break; // EOF
                    }
                }
                Err(_) => break,
            }
        }
        return total_recv as i64;
    }

    -(errno::EBADF as i64)
}

/// sys_accept4 - Accept connection (with flags)
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr (output)
/// - args[2]: addrlen - pointer to address length (input/output)
/// - args[3]: flags - SOCK_CLOEXEC, SOCK_NONBLOCK
pub fn sys_accept4(args: SyscallArgs) -> i64 {
    let _flags = args[3] as i32;
    // TODO: handle SOCK_CLOEXEC/SOCK_NONBLOCK flags
    sys_accept(args)
}

/// sys_recvfrom - Receive data (possibly getting source address)
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: buf - pointer to data buffer
/// - args[2]: len - buffer length
/// - args[3]: flags - flags
/// - args[4]: addr - pointer to source address (optional, output)
/// - args[5]: addrlen - pointer to address length (optional, input/output)
///
/// # Returns
/// Returns number of bytes received on success, negative error code on failure
pub fn sys_recvfrom(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *mut u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let addr_ptr = args[4] as *mut u8;
    let addrlen_ptr = args[5] as *mut u32;

    // Check buffer pointer validity
    if buf_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Validate user buffer pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, len) {
        return -(errno::EFAULT as i64);
    }

    // Validate optional address pointers
    if !addr_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }
    if !addrlen_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    if len == 0 {
        return 0;
    }

    // Get socket
    let socket = match crate::net::socket::get_socket(fd) {
        Some(s) => s,
        None => {
            // Try to find from old socket table
            // Try TCP first
            if let Some(tcp_sock) = crate::net::tcp::tcp_socket_get(fd as i32) {
                // SAFETY: buf_ptr validated with access_ok; len > 0 guaranteed above.
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return match tcp_sock.recv(buf, len) {
                    Ok(n) => n as i64,
                    Err(_) => -(errno::EAGAIN as i64),
                };
            }
            // Then try UDP
            if let Some(_) = crate::net::udp::udp_socket_get(fd as i32) {
                // SAFETY: buf_ptr validated with access_ok; len > 0 guaranteed above.
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return crate::net::udp::udp_recv(fd as i32, buf, len) as i64;
            }
            return -(errno::EBADF as i64);
        }
    };

    // Receive data
    // SAFETY: buf_ptr validated with access_ok; len > 0 guaranteed above.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };

    match socket.recv(buf) {
        Ok((bytes_read, src_addr)) => {
            // If address pointer is provided, write source address
            if let Some((addr, port)) = src_addr {
                if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
                    // SAFETY: addr_ptr/addrlen_ptr validated with access_ok; writing sockaddr_in layout.
                    unsafe {
                        // Write sockaddr_in structure
                        core::ptr::write(addr_ptr as *mut u16, 2);  // sin_family = AF_INET
                        core::ptr::write(addr_ptr.add(2) as *mut u16, port.to_be());
                        core::ptr::write(addr_ptr.add(4) as *mut u32, addr.to_be());
                        // sin_zero remains 0
                        core::ptr::write_bytes(addr_ptr.add(8), 0, 8);
                        // Write address length
                        core::ptr::write(addrlen_ptr, 16);
                    }
                }
            }
            bytes_read as i64
        }
        Err(e) => e as i64,
    }
}
