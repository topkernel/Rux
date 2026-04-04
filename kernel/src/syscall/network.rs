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
pub fn sys_socket(args: SyscallArgs) -> u64 {
    let domain = args[0] as i32;
    let type_ = args[1] as i32;
    let protocol = args[2] as i32;

    // Try using new socket layer
    match crate::net::socket::sys_socket_create(domain, type_, protocol) {
        Ok(fd) => return fd as u64,
        Err(e) => {
            // If new socket layer fails, fallback to old implementation
            // But only fallback on specific errors
            if e != -97 && e != -94 && e != -22 {
                // Not a parameter error, socket layer may be uninitialized
                // Fallback to old implementation
            } else {
                return e as u64;
            }
        }
    }

    // Old implementation (fallback)
    // Currently only support AF_INET (IPv4)
    if domain != 2 {
        return -errno::EAFNOSUPPORT as u64;
    }

    match type_ {
        1 => {
            // SOCK_STREAM (TCP)
            if protocol != 0 && protocol != 6 {
                return -errno::EINVAL as u64;
            }

            use crate::net::tcp;
            match tcp::tcp_socket_alloc() {
                Ok(fd) => fd as u64,
                Err(e) => e as u64
            }
        }
        2 => {
            // SOCK_DGRAM (UDP)
            if protocol != 0 && protocol != 17 {
                return -errno::EINVAL as u64;
            }

            use crate::net::udp;
            match udp::udp_socket_alloc() {
                Ok(fd) => fd as u64,
                Err(e) => e as u64
            }
        }
        _ => {
            -errno::ESOCKTNOSUPPORT as u64
        }
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
pub fn sys_bind(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    // Check address pointer validity
    if addr_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -errno::EFAULT as u64;
    }

    // Read sockaddr_in structure (simplified implementation)
    // struct sockaddr_in {
    //     sa_family_t sin_family;  // 2 bytes
    //     in_port_t sin_port;      // 2 bytes (network byte order)
    //     struct in_addr sin_addr; // 4 bytes
    //     char sin_zero[8];        // 8 bytes
    // };

    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };

    // Currently only support AF_INET
    if sin_family != 2 {
        return -errno::EAFNOSUPPORT as u64;
    }

    // TODO: Need a way to determine if fd is TCP or UDP socket
    // Simplified implementation: try both protocols
    use crate::net::{tcp, udp};

    // Try TCP first
    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        return tcp::tcp_bind(fd, sin_port) as u64;
    }

    // Then try UDP
    if let Some(_socket) = udp::udp_socket_get(fd) {
        return udp::udp_bind(fd, sin_port) as u64;
    }

    -errno::EBADF as u64
}

/// sys_listen - Listen on socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: backlog - pending connection queue length
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_listen(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let backlog = args[1] as i32;

    use crate::net::tcp;

    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        tcp::tcp_listen(fd, backlog as u32) as u64
    } else {
        -errno::EBADF as u64
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
pub fn sys_accept(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let _addr_ptr = args[1] as *mut u8;
    let _addrlen_ptr = args[2] as *mut u32;

    use crate::net::tcp;

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_accept(fd) as u64,
        None => -errno::EBADF as u64
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
pub fn sys_connect(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    // Check address pointer validity
    if addr_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -errno::EFAULT as u64;
    }

    // Read sockaddr_in structure
    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };
    let sin_addr = unsafe { u32::from_be_bytes(*((addr_ptr.add(4)) as *const [u8; 4])) };

    // Currently only support AF_INET
    if sin_family != 2 {
        return -errno::EAFNOSUPPORT as u64;
    }

    use crate::net::tcp;

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_connect(fd, sin_addr, sin_port) as u64,
        None => -errno::EBADF as u64
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
pub fn sys_sendto(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *const u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let addr_ptr = args[4] as *const u8;
    let _addrlen = args[5] as u32;

    // Check buffer pointer validity
    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user buffer pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, len) {
        return -errno::EFAULT as u64;
    }

    if len == 0 {
        return 0;
    }

    // Validate optional address pointer
    if !addr_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -errno::EFAULT as u64;
    }

    // Get socket
    let socket = match crate::net::socket::get_socket(fd) {
        Some(s) => s,
        None => {
            // Try to find from old socket table
            // Try TCP first
            if let Some(_) = crate::net::tcp::tcp_socket_get(fd as i32) {
                let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
                return data.len() as u64;  // Simplified implementation
            }
            // Then try UDP
            if let Some(_) = crate::net::udp::udp_socket_get(fd as i32) {
                let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
                return crate::net::udp::udp_send(fd as i32, data) as u64;
            }
            return -errno::EBADF as u64;
        }
    };

    // Read data
    let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };

    // Parse destination address (if provided)
    let dest_addr = if !addr_ptr.is_null() {
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
        Ok(bytes_sent) => bytes_sent as u64,
        Err(e) => e as u64,
    }
}

/// sys_getsockname - Get socket local address
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr (output)
/// - args[2]: addrlen - pointer to address length (input/output)
pub fn sys_getsockname(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _addr_ptr = args[1] as *mut u8;
    let _addrlen_ptr = args[2] as *mut u32;
    // TODO: implement getsockname
    -errno::ENOSYS as u64
}

/// sys_getpeername - Get socket peer address
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr (output)
/// - args[2]: addrlen - pointer to address length (input/output)
pub fn sys_getpeername(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _addr_ptr = args[1] as *mut u8;
    let _addrlen_ptr = args[2] as *mut u32;
    // TODO: implement getpeername
    -errno::ENOSYS as u64
}

/// sys_setsockopt - Set socket options
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: level - protocol level
/// - args[2]: optname - option name
/// - args[3]: optval - option value
/// - args[4]: optlen - option length
pub fn sys_setsockopt(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _level = args[1] as i32;
    let _optname = args[2] as i32;
    let _optval = args[3] as *const u8;
    let _optlen = args[4] as u32;
    // TODO: implement setsockopt
    -errno::ENOSYS as u64
}

/// sys_getsockopt - Get socket options
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: level - protocol level
/// - args[2]: optname - option name
/// - args[3]: optval - option value (output)
/// - args[4]: optlen - option length (input/output)
pub fn sys_getsockopt(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _level = args[1] as i32;
    let _optname = args[2] as i32;
    let _optval = args[3] as *mut u8;
    let _optlen = args[4] as *mut u32;
    // TODO: implement getsockopt
    -errno::ENOSYS as u64
}

/// sys_shutdown - Shutdown part of full-duplex connection
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: how - SHUT_RD (0), SHUT_WR (1), SHUT_RDWR (2)
pub fn sys_shutdown(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _how = args[1] as i32;
    // TODO: implement shutdown
    -errno::ENOSYS as u64
}

/// sys_sendmsg - Send message through socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: msg - pointer to msghdr
/// - args[2]: flags - flags
pub fn sys_sendmsg(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let msg_ptr = args[1] as *const u8;
    let _flags = args[2] as i32;

    if msg_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(msg_ptr as usize, 64) {
        return -errno::EFAULT as u64;
    }

    // Read msg_name (sa_family) and msg_iov (iovec) from msghdr
    // struct msghdr { msg_name, msg_namelen, msg_iov, msg_iovlen, msg_control, msg_controllen, msg_flags }
    let msg_name_ptr = unsafe { *(msg_ptr as *const *const u8) };
    let msg_namelen = unsafe { *((msg_ptr.add(8)) as *const u32) };
    let msg_iov_ptr = unsafe { *((msg_ptr.add(16)) as *const usize) };
    let msg_iovlen = unsafe { *((msg_ptr.add(24)) as *const usize) };

    // Collect data from iovec
    // struct iovec { iov_base, iov_len }
    let mut total_len = 0usize;
    let mut buf = alloc::vec::Vec::new();
    for i in 0..msg_iovlen {
        let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(i * 16)) as *const usize) };
        let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(i * 16 + 8)) as *const usize) };
        if iov_len > 0 {
            if !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
                return -errno::EFAULT as u64;
            }
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
            Ok(n) => n as u64,
            Err(e) => e as u64,
        }
    } else {
        -errno::EBADF as u64
    }
}

/// sys_recvmsg - Receive message from socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: msg - pointer to msghdr
/// - args[2]: flags - flags
pub fn sys_recvmsg(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let msg_ptr = args[1] as *mut u8;
    let _flags = args[2] as i32;

    if msg_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(msg_ptr as usize, 64) {
        return -errno::EFAULT as u64;
    }

    // Read iovec from msghdr
    let msg_iov_ptr = unsafe { *((msg_ptr.add(16)) as *const usize) };
    let msg_iovlen = unsafe { *((msg_ptr.add(24)) as *const usize) };

    // Calculate total buffer size
    let mut total_buf_len = 0usize;
    for i in 0..msg_iovlen {
        let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(i * 16)) as *const usize) };
        let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(i * 16 + 8)) as *const usize) };
        if iov_len > 0 && !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
            return -errno::EFAULT as u64;
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
                    let iov_base = unsafe { *((msg_iov_ptr.wrapping_add(i * 16)) as *const usize) };
                    let iov_len = unsafe { *((msg_iov_ptr.wrapping_add(i * 16 + 8)) as *const usize) };
                    let copy_len = core::cmp::min(iov_len, bytes_read - offset);
                    if copy_len > 0 {
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
                bytes_read as u64
            }
            Err(e) => e as u64,
        }
    } else {
        -errno::EBADF as u64
    }
}

/// sys_socketpair - Create pair of connected sockets (NR 199)
///
/// # Arguments
/// - args[0]: domain - protocol family
/// - args[1]: type - socket type
/// - args[2]: protocol - protocol
/// - args[3]: sv - pointer to int[2] for fds
pub fn sys_socketpair(args: SyscallArgs) -> u64 {
    let _domain = args[0] as i32;
    let _type_ = args[1] as i32;
    let _protocol = args[2] as i32;
    let sv = args[3] as *mut i32;

    if sv.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(sv as usize, 8) {
        return -errno::EFAULT as u64;
    }
    -errno::ENOSYS as u64
}

/// sys_sendmmsg - Send multiple messages (NR 269)
///
/// # Arguments
/// - args[0]: fd - socket fd
/// - args[1]: msgvec - pointer to mmsghdr array
/// - args[2]: vlen - number of messages
/// - args[3]: flags - flags
pub fn sys_sendmmsg(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _msgvec = args[1] as *const u8;
    let _vlen = args[2] as u32;
    let _flags = args[3] as i32;
    -errno::ENOSYS as u64
}

/// sys_recvmmsg - Receive multiple messages (NR 243)
///
/// # Arguments
/// - args[0]: fd - socket fd
/// - args[1]: msgvec - pointer to mmsghdr array
/// - args[2]: vlen - number of messages
/// - args[3]: flags - flags
/// - args[4]: timeout - pointer to timespec
pub fn sys_recvmmsg(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _msgvec = args[1] as *mut u8;
    let _vlen = args[2] as u32;
    let _flags = args[3] as i32;
    let _timeout = args[4] as *const u8;
    -errno::ENOSYS as u64
}

/// sys_accept4 - Accept connection (with flags)
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr (output)
/// - args[2]: addrlen - pointer to address length (input/output)
/// - args[3]: flags - SOCK_CLOEXEC, SOCK_NONBLOCK
pub fn sys_accept4(args: SyscallArgs) -> u64 {
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
pub fn sys_recvfrom(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *mut u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let addr_ptr = args[4] as *mut u8;
    let addrlen_ptr = args[5] as *mut u32;

    // Check buffer pointer validity
    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user buffer pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, len) {
        return -errno::EFAULT as u64;
    }

    // Validate optional address pointers
    if !addr_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -errno::EFAULT as u64;
    }
    if !addrlen_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4) {
        return -errno::EFAULT as u64;
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
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return match tcp_sock.recv(buf, len) {
                    Ok(n) => n as u64,
                    Err(_) => -errno::EAGAIN as u64,
                };
            }
            // Then try UDP
            if let Some(_) = crate::net::udp::udp_socket_get(fd as i32) {
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return crate::net::udp::udp_recv(fd as i32, buf, len) as u64;
            }
            return -errno::EBADF as u64;
        }
    };

    // Receive data
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };

    match socket.recv(buf) {
        Ok((bytes_read, src_addr)) => {
            // If address pointer is provided, write source address
            if let Some((addr, port)) = src_addr {
                if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
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
            bytes_read as u64
        }
        Err(e) => e as u64,
    }
}
