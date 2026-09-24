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

/// Copy a 16-byte sockaddr_in from user memory via the exception-table path
/// so an unmapped page yields EFAULT instead of a kernel page fault.
/// Returns None on copy failure.
fn copy_sockaddr_in_from_user(addr_ptr: *const u8) -> Option<[u8; 16]> {
    let mut buf = [0u8; 16];
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(buf.as_mut_ptr(), addr_ptr, 16)
    };
    if uncopied > 0 { None } else { Some(buf) }
}

/// Copy a 28-byte sockaddr_in6 from user memory (exception-table path).
fn copy_sockaddr_in6_from_user(addr_ptr: *const u8) -> Option<[u8; 28]> {
    let mut buf = [0u8; 28];
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(buf.as_mut_ptr(), addr_ptr, 28)
    };
    if uncopied > 0 { None } else { Some(buf) }
}

/// P1 IPv6: a parsed user sockaddr, family-normalized.
#[derive(Debug, Clone, Copy)]
enum ParsedSockAddr {
    /// sockaddr_in (or a v4-mapped sockaddr_in6 — normalized here)
    V4 { addr: u32, port: u16 },
    /// sockaddr_in6 with a pure (non-v4-mapped) v6 address
    V6 { addr: crate::net::ipv6::Ipv6Addr, port: u16 },
}

impl ParsedSockAddr {
    fn as_ip_addr(&self) -> crate::net::ipv6::IpAddr {
        match self {
            ParsedSockAddr::V4 { addr, .. } => crate::net::ipv6::IpAddr::V4(*addr),
            ParsedSockAddr::V6 { addr, .. } => crate::net::ipv6::IpAddr::V6(*addr),
        }
    }
}

/// P1 IPv6: parse a sockaddr_in OR sockaddr_in6 from user memory.
/// v4-mapped (::ffff:x.x.x.x) contents normalize to the v4 shape so the
/// protocol layers keep their u32 fast path; pure v6 stays v6.
/// Unix/netlink families are NOT handled here (callers dispatch earlier).
fn parse_sockaddr_inet(addr_ptr: *const u8) -> Result<ParsedSockAddr, i64> {
    use crate::net::socket::{AF_INET, AF_INET6};

    let raw = match copy_sockaddr_in_from_user(addr_ptr) {
        Some(b) => b,
        None => return Err(-(errno::EFAULT as i64)),
    };
    let family = u16::from_le_bytes([raw[0], raw[1]]);

    if family == AF_INET6 as u16 {
        let raw6 = match copy_sockaddr_in6_from_user(addr_ptr) {
            Some(b) => b,
            None => return Err(-(errno::EFAULT as i64)),
        };
        let port = u16::from_be_bytes([raw6[2], raw6[3]]);
        let mut addr6 = [0u8; 16];
        addr6.copy_from_slice(&raw6[8..24]);
        // v4-mapped normalization (::ffff:a.b.c.d -> v4).
        if let Some(v4) = crate::net::ipv6::v6_to_v4_mapped(&addr6) {
            return Ok(ParsedSockAddr::V4 { addr: v4, port });
        }
        return Ok(ParsedSockAddr::V6 { addr: addr6, port });
    }

    if family == AF_INET as u16 {
        let port = u16::from_be_bytes([raw[2], raw[3]]);
        let addr = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
        return Ok(ParsedSockAddr::V4 { addr, port });
    }

    Err(-(errno::EAFNOSUPPORT as i64))
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
    let sockaddr = match copy_sockaddr_in_from_user(addr_ptr) {
        Some(b) => b,
        None => return -(errno::EFAULT as i64),
    };
    let sin_family = u16::from_le_bytes([sockaddr[0], sockaddr[1]]);

    // P0-1: AF_UNIX — sockaddr_un (family + sun_path[108]) against the
    // unix name table. Dispatched before the inet privileged-port check
    // (sin_port is path bytes for unix sockets).
    if sin_family == crate::net::unix::AF_UNIX as u16 {
        const SOCKADDR_UN_LEN: usize = crate::net::unix::SOCKADDR_UN_LEN;
        let want = ((_addrlen as usize).min(SOCKADDR_UN_LEN)).max(2);
        if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, want) {
            return -(errno::EFAULT as i64);
        }
        let mut ubuf = [0u8; SOCKADDR_UN_LEN];
        // SAFETY: addr_ptr/want validated with access_ok; exception-table copy.
        if unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(ubuf.as_mut_ptr(), addr_ptr, want)
        } != 0
        {
            return -(errno::EFAULT as i64);
        }
        let uaddr = match crate::net::unix::parse_sockaddr_un(&ubuf[..want]) {
            Some(a) => a,
            None => return -(errno::EINVAL as i64),
        };
        return match crate::net::unix::unix_socket_from_fd(fd as usize) {
            Some(sock) => match crate::net::unix::unix_bind(&sock, &uaddr) {
                Ok(()) => 0,
                Err(e) => e as i64,
            },
            None => -(errno::ENOTSOCK as i64),
        };
    }

    // P0-2: AF_NETLINK bind — sockaddr_nl { family, pid, groups }; the
    // port id is accepted (kernel-side portid tracking is implicit).
    if sin_family == crate::net::netlink::AF_NETLINK as u16 {
        return match crate::net::netlink::netlink_socket_from_fd(fd as usize) {
            Some(_) => 0,
            None => -(errno::ENOTSOCK as i64),
        };
    }

    // P1 IPv6: parse AF_INET or AF_INET6 (v4-mapped normalizes to v4).
    let parsed = match parse_sockaddr_inet(addr_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let (sin_port, _sin_addr) = match parsed {
        ParsedSockAddr::V4 { addr, port } => (port, addr),
        ParsedSockAddr::V6 { port, .. } => (port, 0),
    };

    // Permission check: privileged ports (< 1024) require CAP_NET_BIND_SERVICE.
    // Port 0 means "assign an ephemeral port" and must NOT be rejected.
    if sin_port != 0 && sin_port < 1024 && !crate::security::capable(crate::security::CAP_NET_BIND_SERVICE) {
        return -(errno::EACCES as i64);
    }

    // Determine socket type via the VFS socket layer first, then delegate to
    // the correct protocol table.  This avoids accidentally binding a TCP fd
    // when the caller created a UDP socket (or vice-versa), because the TCP
    // and UDP tables use independent fd spaces.
    if let Some(socket) = crate::net::socket::get_socket_from_fd(fd as usize) {
        // P1 IPv6: pure v6 binds take the v6 entry point (AF_INET6 sockets
        // reporting v4-mapped/v4 addresses keep the v4 path).
        let result = match parsed {
            ParsedSockAddr::V6 { addr, port } => {
                if !socket.is_ipv6() {
                    // A v4 socket cannot bind a pure v6 address.
                    return -(errno::EAFNOSUPPORT as i64);
                }
                socket.bind6(addr, port)
            }
            ParsedSockAddr::V4 { addr, port } => socket.bind(addr, port),
        };
        match result {
            Ok(()) => 0,
            Err(e) => e as i64,
        }
    } else {
        // W3: fd resolves but is not a socket -> ENOTSOCK
        -(errno::ENOTSOCK as i64)
    }
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
    let fd = args[0] as usize;
    let backlog = args[1] as i32;

    // Resolve through the per-process fd table (review NET-C3): the old
    // code indexed the global TCP table with the process fd, so listen()
    // hit EBADF (or another process's socket) for every real socket.
    // P0-1: AF_UNIX listeners.
    if let Some(usock) = crate::net::unix::unix_socket_from_fd(fd) {
        return match crate::net::unix::unix_listen(&usock, backlog) {
            Ok(()) => 0,
            Err(e) => e as i64,
        };
    }
    if let Some(socket) = crate::net::socket::get_socket_from_fd(fd) {
        if socket.sock_type != crate::net::socket::SocketType::Tcp {
            return -(errno::EOPNOTSUPP as i64);
        }
        match socket.listen(backlog) {
            Ok(()) => 0,
            Err(e) => e as i64,
        }
    } else {
        // W3: fd resolves but is not a socket -> ENOTSOCK
        -(errno::ENOTSOCK as i64)
    }
}

/// W3: resolve a socket fd to (Arc<Socket>, nonblock-view of its file).

// ==================== user-memory helpers (SUM=0 safe) ====================
//
// Raw dereferences of user pointers must not appear in syscall code: with
// sstatus.SUM=0 in the kernel they page-fault into KernelPanic. These
// helpers route through the exception-table copy paths.

/// Write a sockaddr_in { AF_INET, port, addr, zero-pad } and *addrlen=16
/// into user memory. Callers must access_ok-validate both pointers;
/// failures are dropped (same visible semantics as the old bare stores,
/// minus the kernel-fault window on unmapped pages).
unsafe fn put_sockaddr_in(addr_ptr: *mut u8, addrlen_ptr: *mut u32, port: u16, addr: u32) {
    use crate::arch::riscv64::uaccess::{put_user, clear_user};
    let _ = put_user(addr_ptr as *mut u16, 2u16); // sin_family = AF_INET
    let _ = put_user(addr_ptr.add(2) as *mut u16, port.to_be());
    let _ = put_user(addr_ptr.add(4) as *mut u32, addr.to_be());
    clear_user(addr_ptr.add(8), 8); // sin_zero
    let _ = put_user(addrlen_ptr, 16u32);
}

/// P1 IPv6: write a sockaddr_in6 { AF_INET6, port, 0 flowinfo, addr,
/// scope_id } and *addrlen=28. Callers must access_ok-validate both
/// pointers (28 / 4 bytes); failures are dropped, same visible semantics
/// as put_sockaddr_in. Link-local addresses report the single-interface
/// scope id 2 (eth0, Linux ifindex convention); everything else 0.
unsafe fn put_sockaddr_in6(
    addr_ptr: *mut u8,
    addrlen_ptr: *mut u32,
    port: u16,
    addr: crate::net::ipv6::Ipv6Addr,
) {
    use crate::arch::riscv64::uaccess::{put_user};
    let scope_id: u32 = if crate::net::ipv6::is_link_local(&addr) { 2 } else { 0 };
    let _ = put_user(addr_ptr as *mut u16, 10u16); // sin6_family = AF_INET6
    let _ = put_user(addr_ptr.add(2) as *mut u16, port.to_be());
    let _ = put_user(addr_ptr.add(4) as *mut u32, 0u32); // sin6_flowinfo
    let mut off = 8usize;
    for i in 0..16 {
        let _ = put_user(addr_ptr.add(off) as *mut u8, addr[i]);
        off += 1;
    }
    let _ = put_user(addr_ptr.add(24) as *mut u32, scope_id);
    let _ = put_user(addrlen_ptr, 28u32);
}

/// P1 IPv6: write a source/peer address honoring the socket's family —
/// AF_INET sockets get sockaddr_in; AF_INET6 sockets get sockaddr_in6
/// (v4 sources are reported v4-mapped). `want28` selects the access_ok
/// bound the caller already validated.
unsafe fn put_sockaddr_family(
    socket: &crate::net::socket::Socket,
    addr_ptr: *mut u8,
    addrlen_ptr: *mut u32,
    port: u16,
    src: crate::net::ipv6::IpAddr,
) {
    if socket.is_ipv6() {
        put_sockaddr_in6(addr_ptr, addrlen_ptr, port, src.as_v6());
    } else {
        put_sockaddr_in(
            addr_ptr,
            addrlen_ptr,
            port,
            src.as_v4().unwrap_or(0),
        );
    }
}

/// Read one usize field of a user iovec/msghdr through get_user.
/// `addr` is the raw user address (usize, as stored in msghdr/iovec fields).
/// An unreadable field reads as 0 (callers already range-check).
unsafe fn get_user_usize(addr: usize) -> usize {
    crate::arch::riscv64::uaccess::get_user(addr as *const usize).unwrap_or(0)
}

/// P0-1 (SCM_RIGHTS): parse the cmsg buffer of a user msghdr and resolve
/// every SCM_RIGHTS (SOL_SOCKET/SCM_RIGHTS) payload fd into an Arc<File>
/// from the CURRENT (sending) process's fd table. Returns EBADF for
/// unknown fds, EFAULT/EINVAL on malformed control data.
fn parse_scm_rights(
    msg_ptr: *const u8,
) -> Result<alloc::vec::Vec<alloc::sync::Arc<crate::fs::file::File>>, i64> {
    use crate::arch::riscv64::uaccess::{access_ok, copy_from_user, get_user};

    // msghdr layout (64-bit): msg_control @32, msg_controllen @40.
    // SAFETY: msg_ptr was access_ok(64)-validated by the caller.
    let control = unsafe { get_user::<usize>(msg_ptr.add(32) as *const usize).unwrap_or(0) };
    let controllen = unsafe { get_user::<usize>(msg_ptr.add(40) as *const usize).unwrap_or(0) };
    let mut files = alloc::vec::Vec::new();
    if control == 0 || controllen == 0 {
        return Ok(files);
    }
    // Bound the control buffer — a handful of fds needs tens of bytes; a
    // wild controllen must not drive a kernel-heap allocation.
    const MAX_CONTROL: usize = 1024;
    if controllen > MAX_CONTROL {
        return Err(-(errno::EINVAL as i64));
    }
    if !access_ok(control, controllen) {
        return Err(-(errno::EFAULT as i64));
    }
    let mut cbuf = alloc::vec![0u8; controllen];
    // SAFETY: control/controllen validated with access_ok above.
    if unsafe { copy_from_user(cbuf.as_mut_ptr(), control as *const u8, controllen) } != 0 {
        return Err(-(errno::EFAULT as i64));
    }
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => return Err(-(errno::EBADF as i64)),
    };
    // Walk cmsghdrs: { usize cmsg_len; i32 cmsg_level; i32 cmsg_type; }
    // aligned to 8 (CMSG_ALIGN(sizeof(size_t))).
    let mut off = 0usize;
    while off + 16 <= controllen {
        let cmsg_len = usize::from_ne_bytes(cbuf[off..off + 8].try_into().unwrap());
        if cmsg_len < 16 || off + cmsg_len > controllen {
            break; // malformed tail — stop walking
        }
        let level = i32::from_ne_bytes(cbuf[off + 8..off + 12].try_into().unwrap());
        let ctype = i32::from_ne_bytes(cbuf[off + 12..off + 16].try_into().unwrap());
        if level == crate::net::unix::SOL_SOCKET && ctype == crate::net::unix::SCM_RIGHTS {
            let data = &cbuf[off + 16..off + cmsg_len];
            for chunk in data.chunks_exact(4) {
                let fd = i32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if fd < 0 {
                    return Err(-(errno::EBADF as i64));
                }
                match fdtable.get_file(fd as usize) {
                    Some(f) => files.push(f),
                    None => return Err(-(errno::EBADF as i64)),
                }
            }
        }
        off += (cmsg_len + 7) & !7;
    }
    Ok(files)
}

/// P0-1 (SCM_RIGHTS): install the files attached to a received message
/// into the CURRENT (receiving) process's fd table and write an
/// SCM_RIGHTS cmsg into the user msghdr's msg_control. Returns the number
/// of control bytes written (0 when there is no room — the fds are then
/// closed, mirroring Linux's drop-on-no-control-buffer), or EFAULT/EMFILE.
unsafe fn deliver_scm_rights(
    msg_ptr: *mut u8,
    files: &[alloc::sync::Arc<crate::fs::file::File>],
) -> Result<usize, i64> {
    use crate::arch::riscv64::uaccess::{access_ok, copy_to_user, get_user, put_user};

    if files.is_empty() {
        return Ok(0);
    }
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => return Err(-(errno::EBADF as i64)),
    };
    let mut new_fds = alloc::vec::Vec::new();
    let mut unwind = |new_fds: &mut alloc::vec::Vec<i32>, e: i64| -> i64 {
        for nfd in new_fds.drain(..) {
            let _ = fdtable.close_fd(nfd as usize);
        }
        e
    };
    for f in files {
        let fd = match fdtable.alloc_fd() {
            Some(f) => f,
            None => return Err(unwind(&mut new_fds, -(errno::EMFILE as i64))),
        };
        if fdtable.install_fd(fd, f.clone()).is_err() {
            return Err(unwind(&mut new_fds, -(errno::EMFILE as i64)));
        }
        new_fds.push(fd as i32);
    }

    // SAFETY: msg_ptr was access_ok(64)-validated by the caller.
    let control = get_user::<usize>(msg_ptr.add(32) as *const usize).unwrap_or(0);
    let controllen = get_user::<usize>(msg_ptr.add(40) as *const usize).unwrap_or(0);
    let data_len = new_fds.len() * 4;
    let cmsg_len = 16 + data_len;
    let aligned = (cmsg_len + 7) & !7;
    if control == 0 || controllen < aligned {
        // No control buffer room: the fds cannot be reported — close them.
        for nfd in new_fds {
            let _ = fdtable.close_fd(nfd as usize);
        }
        return Ok(0);
    }
    if !access_ok(control, aligned) {
        for nfd in new_fds {
            let _ = fdtable.close_fd(nfd as usize);
        }
        return Err(-(errno::EFAULT as i64));
    }
    let mut cmsg = alloc::vec![0u8; aligned];
    cmsg[0..8].copy_from_slice(&cmsg_len.to_ne_bytes());
    cmsg[8..12].copy_from_slice(&crate::net::unix::SOL_SOCKET.to_ne_bytes());
    cmsg[12..16].copy_from_slice(&crate::net::unix::SCM_RIGHTS.to_ne_bytes());
    for (i, fd) in new_fds.iter().enumerate() {
        cmsg[16 + i * 4..20 + i * 4].copy_from_slice(&fd.to_ne_bytes());
    }
    // SAFETY: control/aligned validated with access_ok above.
    if copy_to_user(control as *mut u8, cmsg.as_ptr(), aligned) != 0 {
        for nfd in new_fds {
            let _ = fdtable.close_fd(nfd as usize);
        }
        return Err(-(errno::EFAULT as i64));
    }
    // Update msg_controllen to the cmsg size actually written.
    // SAFETY: msg_ptr validated by the caller.
    let _ = put_user(msg_ptr.add(40) as *mut usize, aligned);
    Ok(aligned)
}

fn socket_file_of(fd: usize) -> Option<(alloc::sync::Arc<crate::net::socket::Socket>, bool)> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;
    let nonblock = (file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK) != 0;
    let socket = crate::net::socket::get_socket_from_fd(fd)?;
    Some((socket, nonblock))
}

/// Common accept engine (sys_accept / sys_accept4).
///
/// W3: blocking semantics — with no pending connection, a BLOCKING socket
/// sleeps on the listener's wait queue (the RX path wakes it when a child
/// establishes) instead of returning EAGAIN; O_NONBLOCK / SOCK_NONBLOCK
/// differentiates. `addr_ptr`/`addrlen_ptr` receive the peer address
/// (Linux writes it on success; NULL skips).
fn sys_accept_common(fd: usize, flags: i32, addr_ptr: *mut u8, addrlen_ptr: *mut u32) -> i64 {
    use crate::net::socket::SOCK_NONBLOCK_FLAG;

    // P0-1: AF_UNIX listener — same blocking contract as the TCP path.
    if let Some(usock) = crate::net::unix::unix_socket_from_fd(fd) {
        if usock.kind != crate::net::unix::UnixKind::Stream {
            return -(errno::EOPNOTSUPP as i64);
        }
        if *usock.state.lock() != crate::net::unix::UnixState::Listening {
            return -(errno::EINVAL as i64);
        }
        let file_nonblock = {
            let fdtable = match crate::sched::get_current_fdtable() {
                Some(t) => t,
                None => return -(errno::EBADF as i64),
            };
            match fdtable.get_file(fd) {
                Some(f) => (f.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK) != 0,
                None => return -(errno::EBADF as i64),
            }
        };
        let nonblock = file_nonblock || (flags & SOCK_NONBLOCK_FLAG) != 0;
        let deadline = usock.rcvtimeo_deadline();
        loop {
            match crate::net::unix::unix_accept(&usock) {
                Ok(child) => {
                    let new_fd = match crate::net::unix::unix_install_accepted(&child, flags) {
                        Ok(fd) => fd,
                        Err(e) => return e as i64,
                    };
                    // Write the peer (client) address on success.
                    if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
                        if crate::arch::riscv64::uaccess::access_ok(
                            addr_ptr as usize,
                            crate::net::unix::SOCKADDR_UN_LEN,
                        ) && crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4)
                        {
                            // The child's peer is the connecting client.
                            let peer_name = crate::net::unix::unix_peer_bound_name(&child);
                            // SAFETY: pointers validated with access_ok;
                            // exception-table copy inside.
                            unsafe {
                                crate::net::unix::put_sockaddr_un(
                                    addr_ptr,
                                    addrlen_ptr,
                                    peer_name.as_deref(),
                                );
                            }
                        }
                    }
                    return new_fd as i64;
                }
                Err(e) if e != -11 => return e as i64,
                Err(_) => {}
            }
            if nonblock {
                return -(errno::EAGAIN as i64);
            }
            // Blocking: wait on the listener's queue (client connect wakes).
            let s = usock.clone();
            if let Err(e) = crate::net::unix::unix_accept_wait(&s, deadline) {
                return e as i64;
            }
        }
    }

    let (socket, file_nonblock) = match socket_file_of(fd) {
        Some(s) => s,
        None => return -(errno::ENOTSOCK as i64),
    };
    if socket.sock_type != crate::net::socket::SocketType::Tcp {
        return -(errno::EOPNOTSUPP as i64);
    }
    if *socket.state.lock() != crate::net::socket::SocketState::Listening {
        return -(errno::EINVAL as i64);
    }
    let tcp_fd = socket.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
    if tcp_fd < 0 {
        return -(errno::EBADF as i64);
    }

    // W3: nonblocking when the file says so OR accept4 passed SOCK_NONBLOCK.
    let nonblock = file_nonblock || (flags & SOCK_NONBLOCK_FLAG) != 0;
    // SO_RCVTIMEO bounds the accept wait (Linux applies sk_rcvtimeo).
    let deadline = socket.rcvtimeo_deadline();

    loop {
        // Loopback delivery is backlog-based: drain once from syscall context
        // so each accept() attempt advances the handshake one step even if the
        // NetRx softirq has not fired yet (safe now that loopback TX only
        // queues — no RX re-entry).
        crate::net::ethernet::ethernet_poll();
        let ret = crate::net::tcp::tcp_accept(tcp_fd);
        if ret >= 0 {
            // tcp_accept returned a protocol-table index; wrap it into a
            // process fd (Socket + File) so the caller can use it.
            let new_fd = match crate::net::socket::socket_create_accepted(ret, flags) {
                Ok(fd) => fd,
                Err(e) => return e as i64,
            };
            // Linux accept(): write the peer address (and length) on success.
            // P1 IPv6: v6 children report sockaddr_in6.
            if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
                // SAFETY: validated below via access_ok before any write.
                let want = if crate::net::tcp::tcp_is_v6(ret) { 28 } else { 16 };
                if crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, want)
                    && crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4)
                {
                    // For an accepted connection the peer is the CHILD's
                    // remote endpoint, not the listener's.
                    let (paddr, pport) = crate::net::tcp::tcp_remote_endpoint(ret);
                    let (paddr6, _) = crate::net::tcp::tcp_remote_endpoint6(ret);
                    // SAFETY: addr_ptr/addrlen_ptr validated with access_ok;
                    // put_sockaddr_* use the exception-table copy path.
                    unsafe {
                        if want == 28 {
                            put_sockaddr_in6(addr_ptr, addrlen_ptr, pport, paddr6);
                        } else {
                            put_sockaddr_in(addr_ptr, addrlen_ptr, pport, paddr);
                        }
                    }
                }
            }
            return new_fd as i64;
        }
        if ret != -11 {
            return ret as i64;
        }
        // EAGAIN: no completed connection.
        if nonblock {
            return -(errno::EAGAIN as i64);
        }
        // W3: blocking — wait for the RX path to establish a child.
        if let Err(e) = crate::net::socket::socket_accept_wait_round(&socket, deadline) {
            return e as i64;
        }
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
    let fd = args[0] as usize;
    let addr_ptr = args[1] as *mut u8;
    let addrlen_ptr = args[2] as *mut u32;
    sys_accept_common(fd, 0, addr_ptr, addrlen_ptr)
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

    // Read sockaddr_in structure via the exception-table copy path
    let sockaddr = match copy_sockaddr_in_from_user(addr_ptr) {
        Some(b) => b,
        None => return -(errno::EFAULT as i64),
    };
    let sin_family = u16::from_le_bytes([sockaddr[0], sockaddr[1]]);

    // P0-1: AF_UNIX connect — resolve the name in the unix table.
    if sin_family == crate::net::unix::AF_UNIX as u16 {
        const SOCKADDR_UN_LEN: usize = crate::net::unix::SOCKADDR_UN_LEN;
        let want = ((_addrlen as usize).min(SOCKADDR_UN_LEN)).max(2);
        if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, want) {
            return -(errno::EFAULT as i64);
        }
        let mut ubuf = [0u8; SOCKADDR_UN_LEN];
        // SAFETY: addr_ptr/want validated with access_ok; exception-table copy.
        if unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(ubuf.as_mut_ptr(), addr_ptr, want)
        } != 0
        {
            return -(errno::EFAULT as i64);
        }
        let uaddr = match crate::net::unix::parse_sockaddr_un(&ubuf[..want]) {
            Some(a) => a,
            None => return -(errno::EINVAL as i64),
        };
        return match crate::net::unix::unix_socket_from_fd(fd as usize) {
            Some(sock) => match crate::net::unix::unix_connect(&sock, &uaddr) {
                Ok(()) => 0,
                Err(e) => e as i64,
            },
            None => -(errno::ENOTSOCK as i64),
        };
    }

    // P0-2: AF_NETLINK connect — the default peer is the kernel (pid 0);
    // accepted without further state.
    if sin_family == crate::net::netlink::AF_NETLINK as u16 {
        return match crate::net::netlink::netlink_socket_from_fd(fd as usize) {
            Some(_) => 0,
            None => -(errno::ENOTSOCK as i64),
        };
    }

    // P1 IPv6: parse AF_INET or AF_INET6 (v4-mapped normalizes to v4).
    let parsed = match parse_sockaddr_inet(addr_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Resolve through the per-process fd table — never index the global
    // protocol tables with a process fd (review NET-C3).
    if let Some(socket) = crate::net::socket::get_socket_from_fd(fd as usize) {
        let result = match parsed {
            ParsedSockAddr::V6 { addr, port } => {
                if !socket.is_ipv6() {
                    return -(errno::EAFNOSUPPORT as i64);
                }
                socket.connect6(addr, port)
            }
            ParsedSockAddr::V4 { addr, port } => socket.connect(addr, port),
        };
        match result {
            Ok(()) => 0,
            Err(e) => e as i64,
        }
    } else {
        // W3: fd resolves but is not a socket -> ENOTSOCK
        -(errno::ENOTSOCK as i64)
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

    // P0-1: AF_UNIX sendto — stage the payload and route through the unix
    // send engine (sendto cannot carry cmsg data; SCM_RIGHTS is sendmsg).
    if let Some((usock, file_nonblock)) = crate::net::unix::unix_file_of(fd) {
        let stage = len.min(crate::syscall::io::RW_CHUNK);
        let mut kbuf = alloc::vec::Vec::new();
        if kbuf.try_reserve_exact(stage).is_err() {
            return -(errno::ENOMEM as i64);
        }
        kbuf.resize(stage, 0);
        // SAFETY: buf_ptr validated with access_ok(len) above; stage <= len.
        if unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(kbuf.as_mut_ptr(), buf_ptr, stage)
        } != 0
        {
            return -(errno::EFAULT as i64);
        }
        let dest = if !addr_ptr.is_null() {
            const SOCKADDR_UN_LEN: usize = crate::net::unix::SOCKADDR_UN_LEN;
            let want = ((_addrlen as usize).min(SOCKADDR_UN_LEN)).max(2);
            if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, want) {
                return -(errno::EFAULT as i64);
            }
            let mut ubuf = [0u8; SOCKADDR_UN_LEN];
            // SAFETY: validated with access_ok above.
            if unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(ubuf.as_mut_ptr(), addr_ptr, want)
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            crate::net::unix::parse_sockaddr_un(&ubuf[..want])
        } else {
            None
        };
        let nonblock = file_nonblock || (_flags & MSG_DONTWAIT) != 0;
        let deadline = usock.sndtimeo_deadline();
        return match crate::net::unix::unix_send(
            &usock,
            &kbuf,
            alloc::vec::Vec::new(),
            dest.as_ref(),
            nonblock,
            deadline,
        ) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }

    // P0-2: AF_NETLINK — the buffer is/are rtnetlink request message(s);
    // execution queues the responses for the matching recv.
    if let Some((nlsock, _)) = crate::net::netlink::netlink_file_of(fd) {
        let stage = len.min(crate::syscall::io::RW_CHUNK);
        let mut kbuf = alloc::vec::Vec::new();
        if kbuf.try_reserve_exact(stage).is_err() {
            return -(errno::ENOMEM as i64);
        }
        kbuf.resize(stage, 0);
        // SAFETY: buf_ptr validated with access_ok(len) above; stage <= len.
        if unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(kbuf.as_mut_ptr(), buf_ptr, stage)
        } != 0
        {
            return -(errno::EFAULT as i64);
        }
        return match crate::net::netlink::netlink_send(&nlsock, &kbuf) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }

    // Get socket through the per-process fd table (review NET-C3). The old
    // code first indexed the GLOBAL socket table with the process fd and
    // then "fell back" to indexing the protocol tables with it — both wrong
    // namespaces.
    let socket = match crate::net::socket::get_socket_from_fd(fd) {
        Some(s) => s,
        // W3: fd resolves but is not a socket -> ENOTSOCK
        None => return -(errno::ENOTSOCK as i64),
    };

    // R35: copy the user payload into a kernel buffer BEFORE the protocol
    // path. The old raw `from_raw_parts(buf_ptr, len)` deref ran INSIDE
    // Socket::send → send_reliable while holding TCP_TABLE_LOCK — user
    // memory was read (and could take a uaccess exception / page-fault
    // detour) inside the table critical section. try_reserve_exact makes
    // an OOM a clean ENOMEM (file.rs convention) instead of the allocator
    // panic handler.
    //
    // R36 (R7-D4 family): bound the staged copy. `len` is bounded only by
    // access_ok (the USER_END ceiling, ~256GB); the fixed 32MB buddy heap
    // means a multi-MB `len` either fails cleanly (ENOMEM) or — worse —
    // SUCCEEDS and transiently monopolizes most of the kernel heap for
    // data the protocol layer accepts only in bounded chunks anyway:
    // TCP takes at most TCP_SEND_MAX_CHUNK per call (partial write is
    // POSIX-legal for stream sockets — R32-N4), and a UDP datagram can
    // never exceed UDP_MAX_DATAGRAM (Linux returns EMSGSIZE above it).
    // sys_read/sys_write/sys_recvfrom stage at RW_CHUNK and sys_sendmsg
    // caps its iovec aggregate at 4*RW_CHUNK for exactly this reason —
    // the R35 kbuf here was the one uncapped outlier.
    let stage = match socket.sock_type {
        crate::net::socket::SocketType::Tcp => {
            len.min(crate::net::tcp::TcpSocket::TCP_SEND_MAX_CHUNK)
        }
        crate::net::socket::SocketType::Udp => {
            if len > crate::net::udp::UDP_MAX_DATAGRAM {
                return -(errno::EMSGSIZE as i64);
            }
            len
        }
    };
    let mut kbuf = alloc::vec::Vec::new();
    if kbuf.try_reserve_exact(stage).is_err() {
        return -(errno::ENOMEM as i64);
    }
    kbuf.resize(stage, 0);
    // SAFETY: buf_ptr validated with access_ok(len) above; stage <= len.
    if unsafe { crate::arch::riscv64::uaccess::copy_from_user(kbuf.as_mut_ptr(), buf_ptr, stage) } != 0 {
        return -(errno::EFAULT as i64);
    }
    let data = kbuf.as_slice();

    // Parse destination address (if provided)
    let dest_addr = if !addr_ptr.is_null() {
        // R35-fix: parse through the exception-table copy — the old raw
        // from_raw_parts deref of the USER pointer faulted with SUM=0 in
        // syscall context (deterministic KERNPANIC at SockAddrIn::addr,
        // badaddr = the user sockaddr address).
        // P1 IPv6: family-aware parse (sockaddr_in OR sockaddr_in6 with
        // v4-mapped normalization).
        match parse_sockaddr_inet(addr_ptr) {
            Ok(ParsedSockAddr::V4 { addr, port }) => {
                Some((crate::net::ipv6::IpAddr::V4(addr), port))
            }
            Ok(ParsedSockAddr::V6 { addr, port }) => {
                Some((crate::net::ipv6::IpAddr::V6(addr), port))
            }
            Err(_) => {
                // AF_UNIX msg destinations were dispatched above; anything
                // else is not an inet destination.
                return -(errno::EAFNOSUPPORT as i64);
            }
        }
    } else {
        None
    };

    // Send data (W3: through the blocking send engine — blocking sends
    // complete the whole buffer; MSG_DONTWAIT / O_NONBLOCK differentiate).
    let file_nonblock = {
        let fdtable = match crate::sched::get_current_fdtable() {
            Some(t) => t,
            None => return -(errno::EBADF as i64),
        };
        match fdtable.get_file(fd) {
            Some(f) => (f.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK) != 0,
            None => return -(errno::EBADF as i64),
        }
    };
    let nonblock = file_nonblock || (_flags & MSG_DONTWAIT) != 0;
    let deadline = socket.sndtimeo_deadline();
    match crate::net::socket::socket_send_ctl(&socket, data, dest_addr, nonblock, deadline) {
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

    // W3: non-socket fd → ENOTSOCK (matching getpeername). The old
    // fallback fabricated 0.0.0.0:0 "success" for any non-socket fd —
    // worse, get_socket_from_fd used to blindly reinterpret private_data,
    // so a pipe fd produced a garbage Socket read.
    // P0-1: AF_UNIX — report the bound (or peer) path; dispatched before
    // the inet addrlen<16 check because a sockaddr_un can legitimately be
    // shorter than a sockaddr_in.
    if let Some(usock) = crate::net::unix::unix_socket_from_fd(fd) {
        if !crate::arch::riscv64::uaccess::access_ok(
            addr_ptr as usize,
            crate::net::unix::SOCKADDR_UN_LEN,
        ) {
            return -(errno::EFAULT as i64);
        }
        let name = usock.bound_name.lock().clone();
        // SAFETY: addr_ptr validated with access_ok; exception-table copy.
        unsafe {
            crate::net::unix::put_sockaddr_un(addr_ptr, addrlen_ptr, name.as_deref());
        }
        return 0;
    }

    // Inet path: a sockaddr_in needs the full 16 bytes.
    // SAFETY: addrlen_ptr validated with access_ok(4); get_user is the
    // exception-table copy path (SUM=0 safe).
    let addrlen = unsafe {
        crate::arch::riscv64::uaccess::get_user::<u32>(addrlen_ptr).unwrap_or(0)
    } as usize;
    if addrlen < 16 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // P0-2: AF_NETLINK — sockaddr_nl { family, portid, groups }.
    if let Some(nlsock) = crate::net::netlink::netlink_socket_from_fd(fd) {
        if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 12) {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: addr_ptr validated with access_ok; exception-table copy.
        unsafe {
            crate::net::netlink::put_sockaddr_nl_bound(addr_ptr, addrlen_ptr, nlsock.portid);
        }
        return 0;
    }

    let Some(socket) = crate::net::socket::get_socket_from_fd(fd) else {
        return -(errno::ENOTSOCK as i64);
    };

    // UDP sockets that were implicitly bound report the assigned port;
    // connect()-bound TCP sockets likewise (mirror the protocol table).
    if *socket.local_port.lock() == 0 {
        match socket.sock_type {
            crate::net::socket::SocketType::Tcp => {
                let tcp_fd_v = socket.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
                if tcp_fd_v >= 0 {
                    let tcp_fd = tcp_fd_v;
                    let p = crate::net::tcp::tcp_local_port(tcp_fd);
                    if p != 0 {
                        *socket.local_port.lock() = p;
                    }
                }
            }
            crate::net::socket::SocketType::Udp => {
                let udp_fd_v = socket.udp_fd.load(core::sync::atomic::Ordering::Acquire);
                if udp_fd_v >= 0 {
                    let udp_fd = udp_fd_v;
                    let p = crate::net::udp::udp_local_port(udp_fd);
                    if p != 0 {
                        *socket.local_port.lock() = p;
                    }
                }
            }
        }
    }

    let local_addr = *socket.local_addr.lock();
    let local_port = *socket.local_port.lock();
    // P1 IPv6: v6 sockets report sockaddr_in6 (a v4 local address as
    // v4-mapped).
    // SAFETY: addr_ptr/addrlen_ptr validated with access_ok; exception-table copy.
    unsafe {
        if socket.is_ipv6() {
            let local6 = *socket.local_addr6.lock();
            let addr6 = if local6 == crate::net::ipv6::IPV6_ADDR_UNSPECIFIED {
                crate::net::ipv6::v4_to_mapped(local_addr)
            } else {
                local6
            };
            put_sockaddr_in6(addr_ptr, addrlen_ptr, local_port, addr6);
        } else {
            put_sockaddr_in(addr_ptr, addrlen_ptr, local_port, local_addr);
        }
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

    // P0-1: AF_UNIX — the peer's bound path (or the connected DGRAM name).
    // Dispatched before the inet addrlen<16 check: a sockaddr_un can be
    // shorter than a sockaddr_in.
    if let Some(usock) = crate::net::unix::unix_socket_from_fd(fd) {
        if *usock.state.lock() != crate::net::unix::UnixState::Connected {
            return -(errno::ENOTCONN as i64);
        }
        if !crate::arch::riscv64::uaccess::access_ok(
            addr_ptr as usize,
            crate::net::unix::SOCKADDR_UN_LEN,
        ) {
            return -(errno::EFAULT as i64);
        }
        let name = match usock.kind {
            crate::net::unix::UnixKind::Stream => crate::net::unix::unix_peer_bound_name(&usock),
            crate::net::unix::UnixKind::Dgram => usock.dgram_peer.lock().clone(),
        };
        // SAFETY: addr_ptr validated with access_ok; exception-table copy.
        unsafe {
            crate::net::unix::put_sockaddr_un(addr_ptr, addrlen_ptr, name.as_deref());
        }
        return 0;
    }

    // Inet path: a sockaddr_in needs the full 16 bytes.
    // SAFETY: addrlen_ptr validated with access_ok(4); get_user is the
    // exception-table copy path (SUM=0 safe).
    let addrlen = unsafe {
        crate::arch::riscv64::uaccess::get_user::<u32>(addrlen_ptr).unwrap_or(0)
    } as usize;
    if addrlen < 16 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Try new socket layer
    if let Some(socket) = crate::net::socket::get_socket_from_fd(fd) {
        let state = *socket.state.lock();
        if state == crate::net::socket::SocketState::Connected {
            let peer_addr = *socket.remote_addr.lock();
            let peer_port = *socket.remote_port.lock();
            // SAFETY: addr_ptr/addrlen_ptr validated with access_ok; exception-table copy.
            unsafe {
                if socket.is_ipv6() {
                    put_sockaddr_in6(addr_ptr, addrlen_ptr, peer_port, *socket.remote_addr6.lock());
                } else {
                    put_sockaddr_in(addr_ptr, addrlen_ptr, peer_port, peer_addr);
                }
            }
            return 0;
        }
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

    // Validate fd is a socket (W3: get_socket_from_fd verifies the ops
    // table — non-socket fds are ENOTSOCK, not type-confused pointers).
    // P0-1/P0-2: AF_UNIX / AF_NETLINK sockets never resolve through
    // get_socket_from_fd (different ops tables) — dispatch them first.
    let read_i32 = |need: u32| -> Option<i32> {
        if optlen < need || optval.is_null() {
            return None;
        }
        let mut b = [0u8; 4];
        // SAFETY: access_ok(4) was verified (optlen >= need >= 4).
        if unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(b.as_mut_ptr(), optval, 4)
        } != 0
        {
            return None;
        }
        Some(i32::from_ne_bytes(b))
    };

    if let Some(usock) = crate::net::unix::unix_socket_from_fd(fd) {
        let read_tv = || -> Option<u64> {
            if optval.is_null() || optlen < 8 {
                return None;
            }
            let mut tv = [0u8; 16];
            let cpy = core::cmp::min(optlen as usize, 16);
            // SAFETY: access_ok covered optlen bytes at fn entry.
            if unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(tv.as_mut_ptr(), optval, cpy)
            } != 0
            {
                return None;
            }
            let sec = i64::from_ne_bytes(tv[0..8].try_into().unwrap());
            let usec = i64::from_ne_bytes(tv[8..16].try_into().unwrap());
            if sec < 0 || usec < 0 {
                return None;
            }
            Some((sec as u64).saturating_mul(1_000_000).saturating_add(usec as u64))
        };
        return crate::net::unix::unix_setsockopt(
            &usock,
            level,
            optname,
            &|| read_i32(4),
            &read_tv,
        ) as i64;
    }
    if let Some(nlsock) = crate::net::netlink::netlink_socket_from_fd(fd) {
        return match level {
            SOL_SOCKET => match optname {
                SO_RCVTIMEO | SO_SNDTIMEO => {
                    let mut tv = [0u8; 16];
                    if optval.is_null() || optlen < 8 {
                        return -(errno::EINVAL as i64);
                    }
                    let cpy = core::cmp::min(optlen as usize, 16);
                    // SAFETY: access_ok covered optlen bytes at fn entry.
                    if unsafe {
                        crate::arch::riscv64::uaccess::copy_from_user(tv.as_mut_ptr(), optval, cpy)
                    } != 0
                    {
                        return -(errno::EFAULT as i64);
                    }
                    let sec = i64::from_ne_bytes(tv[0..8].try_into().unwrap());
                    let usec = i64::from_ne_bytes(tv[8..16].try_into().unwrap());
                    if sec < 0 || usec < 0 {
                        return -(errno::EINVAL as i64);
                    }
                    let us = (sec as u64)
                        .saturating_mul(1_000_000)
                        .saturating_add(usec as u64);
                    let mut opts = nlsock.options.lock();
                    if optname == SO_RCVTIMEO {
                        opts.rcvtimeo_us = us;
                    } else {
                        opts.sndtimeo_us = us;
                    }
                    0
                }
                SO_SNDBUF | SO_RCVBUF => {
                    let v = match read_i32(4) {
                        Some(v) => v,
                        None => return -(errno::EFAULT as i64),
                    };
                    let mut opts = nlsock.options.lock();
                    let doubled = (v.saturating_mul(2).max(2048)) as u32;
                    if optname == SO_SNDBUF {
                        opts.sndbuf = doubled;
                    } else {
                        opts.rcvbuf = doubled;
                    }
                    0
                }
                _ => 0, // accepted-and-ignored
            },
            _ => -(errno::ENOPROTOOPT as i64),
        };
    }

    let socket = match crate::net::socket::get_socket_from_fd(fd) {
        Some(s) => s,
        None => return -(errno::ENOTSOCK as i64),
    };

    match level {
        SOL_SOCKET => match optname {
            SO_RCVTIMEO | SO_SNDTIMEO => {
                // W3: struct timeval { i64 tv_sec; i64 tv_usec } — the value
                // now TAKES EFFECT (blocking recv/send bounded by it).
                if optval.is_null() || optlen < 8 {
                    return -(errno::EINVAL as i64);
                }
                let mut tv = [0u8; 16];
                // SAFETY: optlen >= 8 and access_ok covered optlen bytes;
                // copy 16 only when the buffer allows, else 8.
                let cpy = core::cmp::min(optlen as usize, 16);
                if cpy < 8 {
                    return -(errno::EINVAL as i64);
                }
                if !crate::arch::riscv64::uaccess::access_ok(optval as usize, cpy) {
                    return -(errno::EFAULT as i64);
                }
                // SAFETY: cpy <= 16, optval validated.
                if unsafe {
                    crate::arch::riscv64::uaccess::copy_from_user(tv.as_mut_ptr(), optval, cpy)
                } != 0
                {
                    return -(errno::EFAULT as i64);
                }
                let sec = i64::from_ne_bytes(tv[0..8].try_into().unwrap());
                let usec = i64::from_ne_bytes(tv[8..16].try_into().unwrap());
                if sec < 0 || usec < 0 {
                    return -(errno::EINVAL as i64);
                }
                let us = (sec as u64)
                    .saturating_mul(1_000_000)
                    .saturating_add(usec as u64);
                let mut opts = socket.options.lock();
                if optname == SO_RCVTIMEO {
                    opts.rcvtimeo_us = us;
                } else {
                    opts.sndtimeo_us = us;
                }
                0
            }
            SO_REUSEADDR => {
                // W3: stored and CHECKED at bind (mirrored into the TCP
                // protocol slot; both binders must opt in to coexist).
                let v = match read_i32(4) {
                    Some(v) => v,
                    None => return -(errno::EFAULT as i64),
                };
                socket.options.lock().reuseaddr = v != 0;
                let tcp_fd_v = socket.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
                if tcp_fd_v >= 0 {
                    let tcp_fd = tcp_fd_v;
                    crate::net::tcp::tcp_set_reuseaddr(tcp_fd, v != 0);
                }
                0
            }
            SO_REUSEPORT => {
                let v = match read_i32(4) {
                    Some(v) => v,
                    None => return -(errno::EFAULT as i64),
                };
                socket.options.lock().reuseport = v != 0;
                0
            }
            SO_SNDBUF | SO_RCVBUF => {
                // W3: stored (doubled like Linux) and reported by getsockopt.
                let v = match read_i32(4) {
                    Some(v) => v,
                    None => return -(errno::EFAULT as i64),
                };
                let mut opts = socket.options.lock();
                let doubled = (v.saturating_mul(2).max(2048)) as u32;
                if optname == SO_SNDBUF {
                    opts.sndbuf = doubled;
                } else {
                    opts.rcvbuf = doubled;
                }
                0
            }
            // SO_KEEPALIVE (P2 fake-success cleanup): stored, mirrored into
            // the TCP protocol slot, and enforced — the timer tick arms a
            // keepidle/keepintvl/keepcnt probe cycle (defaults 2h/75s/9).
            SO_KEEPALIVE => {
                let v = match read_i32(4) {
                    Some(v) => v,
                    None => return -(errno::EFAULT as i64),
                };
                let (idle, intvl, cnt) = {
                    let mut opts = socket.options.lock();
                    opts.keepalive = v != 0;
                    (opts.keepidle_s, opts.keepintvl_s, opts.keepcnt)
                };
                let tcp_fd_v = socket.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
                if tcp_fd_v >= 0 {
                    crate::net::tcp::tcp_set_keepalive(tcp_fd_v, v != 0, idle, intvl, cnt);
                }
                0
            }
            // SO_BROADCAST (P2): stored and ENFORCED — a UDP sendto a
            // broadcast address without it returns EACCES (udp_send_locked).
            SO_BROADCAST => {
                let v = match read_i32(4) {
                    Some(v) => v,
                    None => return -(errno::EFAULT as i64),
                };
                socket.options.lock().broadcast = v != 0;
                let udp_fd_v = socket.udp_fd.load(core::sync::atomic::Ordering::Acquire);
                if udp_fd_v >= 0 {
                    crate::net::udp::udp_set_broadcast(udp_fd_v, v != 0);
                }
                0
            }
            // SO_LINGER (P2): struct linger { int l_onoff; int l_linger }.
            // Stored; close() honors it — l_onoff&&l_linger>0 blocks until
            // the send path drains (bounded by l_linger seconds),
            // l_onoff&&l_linger==0 aborts with RST + data drop.
            SO_LINGER => {
                if optval.is_null() || optlen < 8 {
                    return -(errno::EINVAL as i64);
                }
                let mut raw = [0u8; 8];
                // SAFETY: optlen >= 8 and access_ok covered optlen at entry.
                if unsafe {
                    crate::arch::riscv64::uaccess::copy_from_user(raw.as_mut_ptr(), optval, 8)
                } != 0
                {
                    return -(errno::EFAULT as i64);
                }
                let onoff = i32::from_ne_bytes(raw[0..4].try_into().unwrap());
                let linger = i32::from_ne_bytes(raw[4..8].try_into().unwrap());
                if linger < 0 {
                    return -(errno::EINVAL as i64);
                }
                let mut opts = socket.options.lock();
                opts.linger_on = onoff != 0;
                opts.linger_secs = linger as u32;
                0
            }
            // Accepted-and-ignored (no operational effect in this stack):
            SO_DONTROUTE | SO_OOBINLINE
            | SO_NO_CHECK | SO_BSDCOMPAT | SO_PASSCRED | SO_RCVLOWAT
            | SO_SNDLOWAT | SO_PRIORITY => 0,
            SO_TYPE | SO_ERROR | SO_PEERCRED => {
                -(errno::ENOPROTOOPT as i64) // Read-only options
            }
            // W3: unknown options are no longer silently accepted.
            _ => -(errno::ENOPROTOOPT as i64),
        },
        IPPROTO_TCP => match optname {
            // P2: keepalive tunables — stored, validated (>= 1 like Linux)
            // and mirrored into the protocol slot.
            TCP_KEEPIDLE | TCP_KEEPINTVL | TCP_KEEPCNT => {
                let v = match read_i32(4) {
                    Some(v) => v,
                    None => return -(errno::EFAULT as i64),
                };
                if v < 1 {
                    return -(errno::EINVAL as i64);
                }
                let (ka, idle, intvl, cnt) = {
                    let mut opts = socket.options.lock();
                    match optname {
                        TCP_KEEPIDLE => opts.keepidle_s = v as u32,
                        TCP_KEEPINTVL => opts.keepintvl_s = v as u32,
                        _ => opts.keepcnt = v as u32,
                    }
                    (opts.keepalive, opts.keepidle_s, opts.keepintvl_s, opts.keepcnt)
                };
                let tcp_fd_v = socket.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
                if tcp_fd_v >= 0 {
                    crate::net::tcp::tcp_set_keepalive(tcp_fd_v, ka, idle, intvl, cnt);
                }
                0
            }
            TCP_NODELAY | TCP_CORK => 0,
            _ => -(errno::ENOPROTOOPT as i64),
        },
        IPPROTO_IP => match optname {
            // P2 IP_TTL: stored (1..=255 like Linux) and pushed into the
            // IPv4 header on transmit (TCP and UDP paths).
            IP_TTL => {
                let v = match read_i32(4) {
                    Some(v) => v,
                    None => return -(errno::EFAULT as i64),
                };
                if !(1..=255).contains(&v) {
                    return -(errno::EINVAL as i64);
                }
                socket.options.lock().ttl = v as u32;
                let tcp_fd_v = socket.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
                if tcp_fd_v >= 0 {
                    crate::net::tcp::tcp_set_ttl(tcp_fd_v, v as u8);
                }
                let udp_fd_v = socket.udp_fd.load(core::sync::atomic::Ordering::Acquire);
                if udp_fd_v >= 0 {
                    crate::net::udp::udp_set_ttl(udp_fd_v, v as u8);
                }
                0
            }
            IP_TOS | IP_MULTICAST_TTL | IP_MULTICAST_LOOP
            | IP_ADD_MEMBERSHIP | IP_DROP_MEMBERSHIP => 0,
            _ => -(errno::ENOPROTOOPT as i64),
        },
        // W3: unknown levels are no longer silently accepted.
        _ => -(errno::ENOPROTOOPT as i64),
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
    const SO_ACCEPTCONN: i32 = 30;
    const SO_PROTOCOL: i32 = 38;
    const IPPROTO_TCP: i32 = 6;
    const TCP_NODELAY: i32 = 1;
    const TCP_INFO: i32 = 11;
    const TCP_CORK: i32 = 3;
    const TCP_KEEPIDLE: i32 = 4;
    const TCP_KEEPINTVL: i32 = 5;
    const TCP_KEEPCNT: i32 = 6;
    const IPPROTO_IP: i32 = 0;
    const IP_TOS: i32 = 1;
    const IP_TTL: i32 = 2;

    if optval.is_null() || optlen_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(optlen_ptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }
    // SAFETY: optlen_ptr validated with access_ok; get_user is the
    // exception-table copy path (SUM=0 safe).
    let optlen = unsafe {
        crate::arch::riscv64::uaccess::get_user::<u32>(optlen_ptr).unwrap_or(0)
    } as usize;
    if optlen == 0 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(optval as usize, optlen) {
        return -(errno::EFAULT as i64);
    }

    // P0-1/P0-2: AF_UNIX / AF_NETLINK option reporting. Dispatched before
    // the AF_INET resolution (unix/netlink fds never resolve there).
    if let Some(usock) = crate::net::unix::unix_socket_from_fd(fd) {
        const SO_RCVTIMEO_GET: i32 = 20;
        const SO_SNDTIMEO_GET: i32 = 21;
        const SOL_SOCKET_GET: i32 = 1;
        if level == SOL_SOCKET_GET && (optname == SO_RCVTIMEO_GET || optname == SO_SNDTIMEO_GET) {
            let opts = usock.options.lock();
            let us = if optname == SO_RCVTIMEO_GET {
                opts.rcvtimeo_us
            } else {
                opts.sndtimeo_us
            };
            drop(opts);
            let sec = us / 1_000_000;
            let usec = us % 1_000_000;
            let write_len = core::cmp::min(optlen, 16);
            // SAFETY: optval validated with access_ok(optlen) at entry.
            unsafe {
                crate::arch::riscv64::uaccess::clear_user(optval, optlen.min(write_len));
                if write_len >= 8 {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        optval,
                        &sec as *const u64 as *const u8,
                        8.min(write_len),
                    );
                }
                if write_len >= 16 {
                    crate::arch::riscv64::uaccess::copy_to_user(optval.add(8), &usec as *const u64 as *const u8, 8);
                }
                let _ = crate::arch::riscv64::uaccess::put_user(optlen_ptr, write_len as u32);
            }
            return 0;
        }
        return match crate::net::unix::unix_getsockopt(&usock, level, optname) {
            Ok(v) => {
                // SAFETY: optval/optlen_ptr validated with access_ok above.
                unsafe { write_int(optval, optlen, optlen_ptr, v) };
                0
            }
            Err(e) => e as i64,
        };
    }
    if let Some(_nlsock) = crate::net::netlink::netlink_socket_from_fd(fd) {
        const SOL_SOCKET_GET: i32 = 1;
        const SO_TYPE_GET: i32 = 3;
        const SO_ERROR_GET: i32 = 4;
        const SO_DOMAIN_GET: i32 = 39;
        const SO_PROTOCOL_GET: i32 = 38;
        let val = match (level, optname) {
            (SOL_SOCKET_GET, SO_TYPE_GET) => 3,        // SOCK_RAW
            (SOL_SOCKET_GET, SO_ERROR_GET) => 0,
            (SOL_SOCKET_GET, SO_DOMAIN_GET) => 16,     // AF_NETLINK
            (SOL_SOCKET_GET, SO_PROTOCOL_GET) => 0,    // NETLINK_ROUTE
            _ => return -(errno::ENOPROTOOPT as i64),
        };
        // SAFETY: optval/optlen_ptr validated with access_ok above.
        unsafe { write_int(optval, optlen, optlen_ptr, val) };
        return 0;
    }

    // Validate fd is a socket
    let sock = crate::net::socket::get_socket_from_fd(fd);
    if sock.is_none() {
        return -(errno::ENOTSOCK as i64);
    }
    let sock = sock.unwrap();

    /// Write `val` (little-endian native i32) into optval, truncate to
    /// min(optlen, 4), update optlen.
    // SAFETY: optval/optlen_ptr validated with access_ok above.
    unsafe fn write_int(optval: *mut u8, optlen: usize, optlen_ptr: *mut u32, val: i32) {
        let write_len = core::cmp::min(optlen, 4);
        crate::arch::riscv64::uaccess::clear_user(optval, optlen.min(write_len));
        crate::arch::riscv64::uaccess::copy_to_user(
            optval,
            &val as *const i32 as *const u8,
            write_len,
        );
        let _ = crate::arch::riscv64::uaccess::put_user(optlen_ptr, write_len as u32);
    }

    // SAFETY: optval and optlen_ptr validated with access_ok; writes stay within
    // validated lengths.
    unsafe {
        match level {
            SOL_SOCKET => match optname {
                SO_TYPE => write_int(optval, optlen, optlen_ptr, match sock.sock_type {
                    crate::net::socket::SocketType::Tcp => 1,  // SOCK_STREAM
                    crate::net::socket::SocketType::Udp => 2,  // SOCK_DGRAM
                }),
                SO_ERROR => {
                    // W3: the REAL pending error — socket-level connect()
                    // failure first, then the protocol slot's error (RST /
                    // ICMP / retransmit exhaustion). Reading clears it
                    // (Linux semantics; non-blocking connect probes rely
                    // on exactly this contract).
                    let mut err = sock.options.lock().error;
                    if err == 0 {
                        match sock.sock_type {
                            crate::net::socket::SocketType::Tcp => {
                                let tcp_fd_v = sock.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
                    if tcp_fd_v >= 0 {
                        let tcp_fd = tcp_fd_v;
                                    err = crate::net::tcp::tcp_take_pending_error(tcp_fd);
                                }
                            }
                            crate::net::socket::SocketType::Udp => {
                                let udp_fd_v = sock.udp_fd.load(core::sync::atomic::Ordering::Acquire);
                    if udp_fd_v >= 0 {
                        let udp_fd = udp_fd_v;
                                    err = crate::net::udp::udp_take_pending_error(udp_fd);
                                }
                            }
                        }
                    } else {
                        sock.options.lock().error = 0;
                    }
                    write_int(optval, optlen, optlen_ptr, err);
                }
                SO_ACCEPTCONN => {
                    // W3: 1 when listening (Linux reports it here).
                    let listening =
                        *sock.state.lock() == crate::net::socket::SocketState::Listening;
                    write_int(optval, optlen, optlen_ptr, listening as i32);
                }
                SO_PROTOCOL => {
                    // W3: IPPROTO_TCP / IPPROTO_UDP.
                    let proto = match sock.sock_type {
                        crate::net::socket::SocketType::Tcp => 6,
                        crate::net::socket::SocketType::Udp => 17,
                    };
                    write_int(optval, optlen, optlen_ptr, proto);
                }
                SO_DOMAIN => {
                    // P1 IPv6: report the creation family (AF_INET=2 /
                    // AF_INET6=10).
                    write_int(optval, optlen, optlen_ptr, sock.family);
                }
                SO_REUSEADDR | SO_REUSEPORT => {
                    // W3: report the stored value (was always 0).
                    let opts = sock.options.lock();
                    let v = if optname == SO_REUSEADDR { opts.reuseaddr } else { opts.reuseport };
                    drop(opts);
                    write_int(optval, optlen, optlen_ptr, v as i32);
                }
                SO_KEEPALIVE | SO_BROADCAST => {
                    // P2: report the STORED value (used to be always 0,
                    // hiding a fake success).
                    let opts = sock.options.lock();
                    let v = if optname == SO_KEEPALIVE {
                        opts.keepalive
                    } else {
                        opts.broadcast
                    };
                    drop(opts);
                    write_int(optval, optlen, optlen_ptr, v as i32);
                }
                SO_SNDBUF | SO_RCVBUF => {
                    // W3: the actual stored value (setsockopt now records it).
                    let opts = sock.options.lock();
                    let v = if optname == SO_SNDBUF { opts.sndbuf } else { opts.rcvbuf };
                    drop(opts);
                    write_int(optval, optlen, optlen_ptr, v as i32);
                }
                SO_RCVLOWAT | SO_SNDLOWAT => {
                    write_int(optval, optlen, optlen_ptr, 1); // Default: 1 byte
                }
                SO_RCVTIMEO | SO_SNDTIMEO => {
                    // W3: struct timeval { tv_sec, tv_usec } — the stored
                    // value (0,0 = no timeout).
                    let opts = sock.options.lock();
                    let us = if optname == SO_RCVTIMEO { opts.rcvtimeo_us } else { opts.sndtimeo_us };
                    drop(opts);
                    let sec = (us / 1_000_000) as u64;
                    let usec = (us % 1_000_000) as u64;
                    let write_len = core::cmp::min(optlen, 16);
                    crate::arch::riscv64::uaccess::clear_user(optval, optlen.min(write_len));
                    if write_len >= 8 {
                        crate::arch::riscv64::uaccess::copy_to_user(
                            optval,
                            &sec as *const u64 as *const u8,
                            8.min(write_len),
                        );
                    }
                    if write_len >= 16 {
                        crate::arch::riscv64::uaccess::copy_to_user(
                            optval.add(8),
                            &usec as *const u64 as *const u8,
                            8,
                        );
                    }
                    let _ = crate::arch::riscv64::uaccess::put_user(optlen_ptr, write_len as u32);
                }
                SO_LINGER => {
                    // struct linger { l_onoff: i32, l_linger: i32 } = 8 bytes
                    // (P2: the STORED value — was always linger-off).
                    let opts = sock.options.lock();
                    let onoff = opts.linger_on as i32;
                    let secs = opts.linger_secs as i32;
                    drop(opts);
                    let write_len = core::cmp::min(optlen, 8);
                    crate::arch::riscv64::uaccess::clear_user(optval, optlen.min(write_len));
                    if write_len >= 4 {
                        crate::arch::riscv64::uaccess::copy_to_user(
                            optval,
                            &onoff as *const i32 as *const u8,
                            4,
                        );
                    }
                    if write_len >= 8 {
                        crate::arch::riscv64::uaccess::copy_to_user(
                            optval.add(4),
                            &secs as *const i32 as *const u8,
                            4,
                        );
                    }
                    let _ = crate::arch::riscv64::uaccess::put_user(optlen_ptr, write_len as u32);
                }
                SO_PEERCRED => {
                    // struct ucred { pid, uid, gid } = 12 bytes
                    let write_len = core::cmp::min(optlen, 12);
                    crate::arch::riscv64::uaccess::clear_user(optval, optlen.min(write_len));
                    let _ = crate::arch::riscv64::uaccess::put_user(optlen_ptr, write_len as u32);
                }
                _ => {
                    return -(errno::ENOPROTOOPT as i64);
                }
            },
            IPPROTO_TCP => match optname {
                TCP_NODELAY => {
                    write_int(optval, optlen, optlen_ptr, 1); // Nodelay enabled by default
                }
                TCP_CORK | TCP_INFO => {
                    let write_len = core::cmp::min(optlen, 4);
                    crate::arch::riscv64::uaccess::clear_user(optval, optlen.min(write_len));
                    let _ = crate::arch::riscv64::uaccess::put_user(optlen_ptr, write_len as u32);
                }
                TCP_KEEPIDLE | TCP_KEEPINTVL | TCP_KEEPCNT => {
                    // P2: the stored tunables (Linux defaults 7200/75/9).
                    let opts = sock.options.lock();
                    let v = match optname {
                        TCP_KEEPIDLE => opts.keepidle_s,
                        TCP_KEEPINTVL => opts.keepintvl_s,
                        _ => opts.keepcnt,
                    };
                    drop(opts);
                    write_int(optval, optlen, optlen_ptr, v as i32);
                }
                _ => {
                    return -(errno::ENOPROTOOPT as i64);
                }
            },
            IPPROTO_IP => match optname {
                IP_TOS => {
                    write_int(optval, optlen, optlen_ptr, 0);
                }
                IP_TTL => {
                    // P2: the per-socket TTL (0-stored = system default 64).
                    let v = {
                        let opts = sock.options.lock();
                        if opts.ttl == 0 {
                            crate::config::IP_DEFAULT_TTL as i32
                        } else {
                            opts.ttl as i32
                        }
                    };
                    write_int(optval, optlen, optlen_ptr, v);
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

    // P0-1: AF_UNIX — mark the peer's EOF / our shut-write state.
    if let Some(usock) = crate::net::unix::unix_socket_from_fd(fd) {
        return match crate::net::unix::unix_shutdown(&usock, how) {
            Ok(()) => 0,
            Err(e) => e as i64,
        };
    }

    if let Some(socket) = crate::net::socket::get_socket_from_fd(fd) {
        if how == 1 || how == 2 {
            // SHUT_WR or SHUT_RDWR: send FIN for TCP (review NET-M12 — the
            // old code only flipped a state bit and never emitted a FIN).
            // R32-B6: the close mutates connection state (and pushes the
            // FIN onto the retransmit queue) — it MUST hold TCP_TABLE_LOCK
            // like every other protocol-table writer (tcp_rcv, timer tick,
            // Socket::close); the old unlocked path raced both. Leaf-scoped,
            // no RX re-entry: close() only queues to loopback/virtio xmit.
            //
            // R35 (chain-2 fix): the FIN is recorded into a deferred
            // TcpTxBatch reserved outside the lock and emitted after it
            // drops — send_fin used to run the virtio completion spin
            // under TCP_TABLE_LOCK.
            let tcp_fd_v = socket.tcp_fd.load(core::sync::atomic::Ordering::Acquire);
            if tcp_fd_v >= 0 {
                let tcp_fd = tcp_fd_v;
                let mut tx = crate::net::tcp::TcpTxBatch::new();
                tx.set_ttl(socket.options.lock().ttl as u8); // P2 IP_TTL
                let _ = tx.reserve(1, 0);
                {
                    let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
                    if let Some(tcp_sock) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                        tcp_sock.close(&mut tx);
                    }
                }
                tx.emit_all();
            }
            *socket.state.lock() = crate::net::socket::SocketState::Closing;
        }
        // SHUT_RD alone must NOT block the send side (review NEW): setting
        // Closing for how==0 broke Socket::send with EPIPE.
        return 0;
    }

    -(errno::ENOTSOCK as i64)
}

/// sys_sendmsg - Send message through socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: msg - pointer to msghdr
/// - args[2]: flags - flags
///
/// W3: msg_name is honored (the old code read and DISCARDED it — a UDP
/// sendmsg could never address its datagram); MSG_DONTWAIT /
/// O_NONBLOCK / SO_SNDTIMEO participate in the send.
pub fn sys_sendmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msg_ptr = args[1] as *const u8;
    let flags = args[2] as i32;

    if msg_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msg_ptr as usize, 64) {
        return -(errno::EFAULT as i64);
    }

    // Read msg_name (sa_family) and msg_iov (iovec) from msghdr
    // struct msghdr { msg_name, msg_namelen, msg_iov, msg_iovlen, msg_control, msg_controllen, msg_flags }
    // SAFETY: msg_ptr validated with access_ok(64); reading fields at known offsets.
    // SAFETY: msg_ptr validated with access_ok(64); get_user is the
    // exception-table copy path (SUM=0 safe). Unreadable fields read as 0.
    let msg_name_ptr = unsafe {
        crate::arch::riscv64::uaccess::get_user::<*const u8>(msg_ptr as *const *const u8)
            .unwrap_or(core::ptr::null())
    };
    let msg_namelen = unsafe {
        crate::arch::riscv64::uaccess::get_user::<u32>(msg_ptr.add(8) as *const u32).unwrap_or(0)
    };
    let msg_iov_ptr = unsafe { get_user_usize(msg_ptr.add(16) as usize) };
    let msg_iovlen = unsafe { get_user_usize(msg_ptr.add(24) as usize) };

    // Collect data from iovec
    // struct iovec { iov_base, iov_len }
    let mut total_len = 0usize;
    let mut buf = alloc::vec::Vec::new();
    // R20-2 (LOW-11): reject oversized iovec arrays instead of silently
    // clamping — the clamp dropped trailing iovecs, silently truncating
    // the message. Linux returns EMSGSIZE for msg_iovlen > UIO_MAXIOV.
    if msg_iovlen > 1024 {
        return -(errno::EMSGSIZE as i64);
    }
    // R20-3: the iovec array itself was dereferenced raw — a kernel or
    // unmapped-range msg_iov pointer faulted the kernel (no exception
    // table). Range-check it like every other user pointer.
    if !crate::arch::riscv64::uaccess::access_ok(msg_iov_ptr as usize, msg_iovlen * 16) {
        return -(errno::EFAULT as i64);
    }
    for i in 0..msg_iovlen {
        // SAFETY: iovec base/len read from user memory at validated offset; iov_base
        // validated with access_ok before slice creation.
        let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16)) };
        let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16 + 8)) };
        if iov_len > 0 {
            if !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
                return -(errno::EFAULT as i64);
            }
            // R7-D4: cap the aggregate iovec length at RW_CHUNK — the
            // kernel heap is 32MB and `access_ok` only bounds each buffer
            // by USER_END (256GB); a single iov_len near 2^32 panicked the
            // kernel in vec allocation (SYSA-C1 class, read/write were
            // already chunked).
            // R32-B2: the check must run INSIDE the loop against the
            // running total. The old per-iov-only check (plus a post-loop
            // aggregate check) let the loop first gather up to
            // msg_iovlen × 256KB into `buf` before rejecting — the very
            // over-allocation R7-D4 was meant to stop. This subsumes the
            // single-iov case (total_len starts at 0) and returns the
            // Linux error (EMSGSIZE, not EFAULT) for an oversized message.
            const MSG_IOV_MAX_TOTAL: usize = crate::syscall::io::RW_CHUNK.saturating_mul(4);
            if total_len.saturating_add(iov_len) > MSG_IOV_MAX_TOTAL {
                return -(errno::EMSGSIZE as i64);
            }

            // SAFETY: iov_base validated with access_ok; gather through the
            // exception-table copy path (SUM=0 safe).
            let start = buf.len();
            buf.resize(start + iov_len, 0);
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    buf.as_mut_ptr().add(start),
                    iov_base as *const u8,
                    iov_len,
                )
            };
            if uncopied > 0 {
                return -(errno::EFAULT as i64);
            }
            total_len += iov_len;
        }
    }

    if total_len == 0 {
        return 0;
    }

    // P0-1: AF_UNIX sendmsg — full path with SCM_RIGHTS support.
    if let Some((usock, file_nonblock)) = crate::net::unix::unix_file_of(fd as usize) {
        // Parse SCM_RIGHTS cmsg(s) from msg_control (msghdr offsets 32/40).
        let scm_files = match parse_scm_rights(msg_ptr) {
            Ok(f) => f,
            Err(e) => return e,
        };
        // Destination: a sockaddr_un msg_name for DGRAM.
        let udest = if !msg_name_ptr.is_null() && msg_namelen >= 3 {
            const SOCKADDR_UN_LEN: usize = crate::net::unix::SOCKADDR_UN_LEN;
            let want = (msg_namelen as usize).min(SOCKADDR_UN_LEN);
            if !crate::arch::riscv64::uaccess::access_ok(msg_name_ptr as usize, want) {
                return -(errno::EFAULT as i64);
            }
            let mut ubuf = [0u8; SOCKADDR_UN_LEN];
            // SAFETY: msg_name_ptr/want validated with access_ok.
            if unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    ubuf.as_mut_ptr(),
                    msg_name_ptr,
                    want,
                )
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            crate::net::unix::parse_sockaddr_un(&ubuf[..want])
        } else {
            None
        };
        let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
        let deadline = usock.sndtimeo_deadline();
        return match crate::net::unix::unix_send(
            &usock,
            &buf,
            scm_files,
            udest.as_ref(),
            nonblock,
            deadline,
        ) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }

    // P0-2: AF_NETLINK — execute the rtnetlink request(s) in the buffer.
    if let Some((nlsock, _)) = crate::net::netlink::netlink_file_of(fd as usize) {
        return match crate::net::netlink::netlink_send(&nlsock, &buf) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }

    // W3: parse the destination from msg_name (same path as sendto's
    // addr_ptr) — UDP sendmsg used to pass NULL and could never send a
    // datagram; TCP ignores it.
    // P1 IPv6: family-aware parse (v4-mapped normalizes to V4).
    let dest_addr = if !msg_name_ptr.is_null() && msg_namelen >= 16 {
        if !crate::arch::riscv64::uaccess::access_ok(msg_name_ptr as usize, 16) {
            return -(errno::EFAULT as i64);
        }
        match parse_sockaddr_inet(msg_name_ptr) {
            Ok(ParsedSockAddr::V4 { addr, port }) => {
                Some((crate::net::ipv6::IpAddr::V4(addr), port))
            }
            Ok(ParsedSockAddr::V6 { addr, port }) => {
                Some((crate::net::ipv6::IpAddr::V6(addr), port))
            }
            Err(_) => return -(errno::EAFNOSUPPORT as i64),
        }
    } else {
        None
    };

    // Get socket and send (W3: through the blocking send engine).
    match socket_file_of(fd as usize) {
        Some((socket, file_nonblock)) => {
            // W3: a UDP datagram above 65507 bytes cannot be represented
            // in the 16-bit length field — EMSGSIZE up front (not a late
            // EIO from the packet builder).
            if socket.sock_type == crate::net::socket::SocketType::Udp
                && total_len > crate::net::udp::UDP_MAX_DATAGRAM
            {
                return -(errno::EMSGSIZE as i64);
            }
            let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
            let deadline = socket.sndtimeo_deadline();
            match crate::net::socket::socket_send_ctl(&socket, &buf, dest_addr, nonblock, deadline) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        None => -(errno::ENOTSOCK as i64),
    }
}

/// W3: MSG_DONTWAIT (recvfrom/sendto/recvmsg/sendmsg).
const MSG_DONTWAIT: i32 = 0x40;
/// W3: MSG_TRUNC (reported in recvmsg's msg_flags).
const MSG_TRUNC: i32 = 0x20;

/// sys_recvmsg - Receive message from socket
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: msg - pointer to msghdr
/// - args[2]: flags - flags
///
/// W3: the source address is written back to msg_name/msg_namelen (the
/// old code discarded it — recvfrom-style callers on UDP never saw the
/// peer), msg_flags reports MSG_TRUNC on truncated datagrams, and the
/// receive honors blocking / O_NONBLOCK / MSG_DONTWAIT / SO_RCVTIMEO.
pub fn sys_recvmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msg_ptr = args[1] as *mut u8;
    let flags = args[2] as i32;

    if msg_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msg_ptr as usize, 64) {
        return -(errno::EFAULT as i64);
    }

    // Read iovec from msghdr
    // SAFETY: msg_ptr validated with access_ok(64); reading fields at known offsets.
    // SAFETY: msg_ptr validated with access_ok(64); get_user is the
    // exception-table copy path (SUM=0 safe). Unreadable fields read as 0.
    let msg_name_ptr = unsafe {
        crate::arch::riscv64::uaccess::get_user::<*mut u8>(msg_ptr as *const *mut u8)
            .unwrap_or(core::ptr::null_mut())
    };
    let msg_namelen_ptr = unsafe {
        crate::arch::riscv64::uaccess::get_user::<*mut u32>(msg_ptr.add(8) as *const *mut u32)
            .unwrap_or(core::ptr::null_mut())
    };
    let msg_iov_ptr = unsafe { get_user_usize(msg_ptr.add(16) as usize) };
    let msg_iovlen = unsafe { get_user_usize(msg_ptr.add(24) as usize) };

    // Calculate total buffer size
    let mut total_buf_len = 0usize;
    // R20-2 (LOW-11): EMSGSIZE instead of silently dropping trailing
    // iovecs (Linux UIO_MAXIOV limit).
    if msg_iovlen > 1024 {
        return -(errno::EMSGSIZE as i64);
    }
    // R20-3: range-check the iovec array before raw deref (see sys_sendmsg).
    if !crate::arch::riscv64::uaccess::access_ok(msg_iov_ptr as usize, msg_iovlen * 16) {
        return -(errno::EFAULT as i64);
    }
    for i in 0..msg_iovlen {
        // SAFETY: iovec fields read from user memory at validated offset.
        let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16)) };
        let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16 + 8)) };
        if iov_len > 0 && !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
            return -(errno::EFAULT as i64);
        }
        total_buf_len += iov_len;
        // R7-D4: bound the aggregate before the vec allocation (heap is
        // 32MB; access_ok alone admits iov_len up to 256GB).
        if total_buf_len > crate::syscall::io::RW_CHUNK.saturating_mul(4) {
            return -(errno::EMSGSIZE as i64);
        }
    }

    if total_buf_len == 0 {
        return 0;
    }

    // Allocate receive buffer
    let mut buf = alloc::vec![0u8; total_buf_len];

    // P0-1: AF_UNIX recvmsg — one receive with SCM_RIGHTS delivery and a
    // sockaddr_un source address.
    if let Some((usock, file_nonblock)) = crate::net::unix::unix_file_of(fd as usize) {
        let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
        let deadline = usock.rcvtimeo_deadline();
        let r = match crate::net::unix::unix_recv_ctl(&usock, &mut buf, nonblock, deadline) {
            Ok(r) => r,
            Err(e) => return e as i64,
        };
        // Scatter data back to iovecs.
        let mut offset = 0usize;
        for i in 0..msg_iovlen {
            if offset >= r.len {
                break;
            }
            // SAFETY: iovec fields at validated user offset (the array was
            // access_ok-checked above).
            let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16)) };
            let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16 + 8)) };
            let copy_len = core::cmp::min(iov_len, r.len - offset);
            if copy_len > 0 {
                // SAFETY: iov_base validated with access_ok above.
                let uncopied = unsafe {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        iov_base as *mut u8,
                        buf.as_ptr().add(offset),
                        copy_len,
                    )
                };
                if uncopied > 0 {
                    return if offset > 0 { offset as i64 } else { -(errno::EFAULT as i64) };
                }
                offset += copy_len;
            }
        }
        // SCM_RIGHTS: install the attached files + write the cmsg.
        if !r.files.is_empty() {
            // SAFETY: msg_ptr was access_ok(64)-validated at fn entry.
            if let Err(e) = unsafe { deliver_scm_rights(msg_ptr, &r.files) } {
                return e;
            }
        }
        // Source address.
        if let Some(src) = r.src {
            if !msg_name_ptr.is_null() && !msg_namelen_ptr.is_null() {
                if crate::arch::riscv64::uaccess::access_ok(
                    msg_name_ptr as usize,
                    crate::net::unix::SOCKADDR_UN_LEN,
                ) && crate::arch::riscv64::uaccess::access_ok(msg_namelen_ptr as usize, 4)
                {
                    // SAFETY: pointers validated with access_ok.
                    unsafe {
                        crate::net::unix::put_sockaddr_un(
                            msg_name_ptr,
                            msg_namelen_ptr,
                            Some(&src),
                        );
                    }
                }
            }
        }
        // msg_flags (offset 48): MSG_TRUNC for truncated datagrams.
        let mut out_flags: u32 = 0;
        if r.truncated {
            out_flags |= MSG_TRUNC as u32;
        }
        // SAFETY: msg_ptr validated with access_ok(64) at entry.
        unsafe {
            let _ = crate::arch::riscv64::uaccess::put_user(msg_ptr.add(48) as *mut u32, out_flags);
        }
        return r.len as i64;
    }

    // P0-2: AF_NETLINK recvmsg — exactly one rtnetlink message per call,
    // source address = kernel sockaddr_nl.
    if let Some((nlsock, file_nonblock)) = crate::net::netlink::netlink_file_of(fd as usize) {
        let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
        let deadline = nlsock.rcvtimeo_deadline();
        let n = match crate::net::netlink::netlink_recv(&nlsock, &mut buf, nonblock, deadline) {
            Ok(n) => n,
            Err(e) => return e as i64,
        };
        let mut offset = 0usize;
        for i in 0..msg_iovlen {
            if offset >= n {
                break;
            }
            // SAFETY: iovec fields at validated user offset.
            let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16)) };
            let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16 + 8)) };
            let copy_len = core::cmp::min(iov_len, n - offset);
            if copy_len > 0 {
                // SAFETY: iov_base validated with access_ok above.
                let uncopied = unsafe {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        iov_base as *mut u8,
                        buf.as_ptr().add(offset),
                        copy_len,
                    )
                };
                if uncopied > 0 {
                    return if offset > 0 { offset as i64 } else { -(errno::EFAULT as i64) };
                }
                offset += copy_len;
            }
        }
        if !msg_name_ptr.is_null() && !msg_namelen_ptr.is_null() {
            if crate::arch::riscv64::uaccess::access_ok(msg_name_ptr as usize, 12)
                && crate::arch::riscv64::uaccess::access_ok(msg_namelen_ptr as usize, 4)
            {
                // SAFETY: pointers validated with access_ok.
                unsafe {
                    crate::net::netlink::put_sockaddr_nl(msg_name_ptr, msg_namelen_ptr);
                }
            }
        }
        return n as i64;
    }

    // Get socket and receive (W3: through the blocking recv engine).
    let (socket, file_nonblock) = match socket_file_of(fd as usize) {
        Some(s) => s,
        None => return -(errno::ENOTSOCK as i64),
    };
    let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
    let deadline = socket.rcvtimeo_deadline();

    // W3: MSG_TRUNC — measure the NEXT (about-to-be-received) datagram
    // against the iovec space BEFORE consuming it.
    let datagram_truncated = if socket.sock_type == crate::net::socket::SocketType::Udp {
        let proto = socket.udp_fd.load(core::sync::atomic::Ordering::Acquire);
        if proto >= 0 {
            crate::net::udp::udp_next_dgram_len(proto)
                .map(|l| l > total_buf_len)
                .unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };

    let (bytes_read, src_addr) =
        match crate::net::socket::socket_recv_ctl(&socket, &mut buf, nonblock, deadline) {
            Ok(r) => r,
            Err(e) => return e as i64,
        };

    // W3: MSG_TRUNC — a datagram longer than the iovec space was truncated.
    let mut out_flags: u32 = 0;
    if datagram_truncated && bytes_read > 0 {
        out_flags |= MSG_TRUNC as u32;
    }
    // Scatter data back to iovecs
    let mut offset = 0usize;
    for i in 0..msg_iovlen {
        if offset >= bytes_read { break; }
        // SAFETY: iovec fields at validated user offset; copy_len bounds the write.
        let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16)) };
        let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(i * 16 + 8)) };
        let copy_len = core::cmp::min(iov_len, bytes_read - offset);
        if copy_len > 0 {
            // R20-3: exception-table copy — a raw
            // copy_nonoverlapping to an unmapped (or COW-read-only) user
            // page faulted the kernel.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    iov_base as *mut u8,
                    buf.as_ptr().add(offset),
                    copy_len,
                )
            };
            if uncopied > 0 {
                return if offset > 0 { offset as i64 } else { -(errno::EFAULT as i64) };
            }
            offset += copy_len;
        }
    }

    // W3: write the source address into msg_name/msg_namelen (UDP peers
    // are now visible; TCP reports the connected remote).
    // P1 IPv6: family-aware shape (v6 sockets get sockaddr_in6; a v4
    // source is reported v4-mapped).
    if let Some((addr, port)) = src_addr {
        if !msg_name_ptr.is_null() && !msg_namelen_ptr.is_null() {
            if crate::arch::riscv64::uaccess::access_ok(msg_name_ptr as usize, 16)
                && crate::arch::riscv64::uaccess::access_ok(msg_namelen_ptr as usize, 4)
            {
                // SAFETY: pointers validated with access_ok; exception-table copy.
                unsafe {
                    put_sockaddr_family(&socket, msg_name_ptr, msg_namelen_ptr, port, addr);
                }
            }
        }
    }

    // W3: msg_flags (offset 48 in msghdr) — MSG_TRUNC when a UDP datagram
    // was longer than the provided buffer space.
    // (datagram_truncated was measured pre-receive; see above.)
    // SAFETY: msg_ptr validated with access_ok(64); put_user is the
    // exception-table copy path.
    unsafe {
        let _ = crate::arch::riscv64::uaccess::put_user(msg_ptr.add(48) as *mut u32, out_flags);
    }

    bytes_read as i64
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

    // P0-1: real AF_UNIX socketpair — two pre-connected fds (STREAM pairs
    // get a bidirectional buffer pair, DGRAM pairs per-message delivery).
    let (fd0, fd1) = match crate::net::unix::unix_socketpair(_type_) {
        Ok(pair) => pair,
        Err(e) => return e as i64,
    };
    // SAFETY: sv was access_ok(8)-validated above; put_user is the
    // exception-table copy path.
    unsafe {
        let _ = crate::arch::riscv64::uaccess::put_user(sv as *mut i32, fd0 as i32);
        let _ = crate::arch::riscv64::uaccess::put_user(sv.add(1) as *mut i32, fd1 as i32);
    }
    0
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
/// struct msghdr is 56 bytes on 64-bit; mmsghdr = 64 bytes (4-byte msg_len + 4 pad)
///
/// W3 (ABI): first-failure semantics — an error on the FIRST message is
/// returned as the negative errno; after ≥1 successful sends the count is
/// returned (returning 0 here lied "no messages" under the Linux ABI).
pub fn sys_sendmmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msgvec = args[1] as *const u8;
    let vlen = args[2] as u32;
    let flags = args[3] as i32;

    if msgvec.is_null() || vlen == 0 {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msgvec as usize, vlen as usize * 64) {
        return -(errno::EFAULT as i64);
    }

    // P0-1/P0-2: AF_UNIX / AF_NETLINK — route each mmsghdr through the
    // single-message sys_sendmsg (an mmsghdr's first 56 bytes ARE an
    // msghdr; msg_len is written back at offset 56).
    if crate::net::unix::unix_file_of(fd as usize).is_some()
        || crate::net::netlink::netlink_file_of(fd as usize).is_some()
    {
        let mut total_sent = 0u32;
        for i in 0..vlen as usize {
            // SAFETY: msgvec access_ok-validated above; mm within range.
            let mm = unsafe { msgvec.add(i * 64) };
            let ret = sys_sendmsg([
                fd as u64,
                mm as usize as u64,
                flags as u64,
                0,
                0,
                0,
            ]);
            if ret < 0 {
                if total_sent == 0 {
                    return ret;
                }
                break;
            }
            // SAFETY: mm within validated range; put_user copies 4 bytes.
            unsafe {
                let _ = crate::arch::riscv64::uaccess::put_user(mm.add(56) as *mut u32, ret as u32);
            }
            total_sent += 1;
            if ret == 0 {
                break;
            }
        }
        return total_sent as i64;
    }

    let (socket, file_nonblock) = match socket_file_of(fd as usize) {
        Some(s) => s,
        None => return -(errno::ENOTSOCK as i64),
    };
    let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
    let deadline = socket.sndtimeo_deadline();

    let mut total_sent = 0u32;
    for i in 0..vlen as usize {
        // mmsghdr: msghdr (56 bytes) + msg_len (4 bytes)
        // SAFETY: msgvec validated with access_ok; mm offset within validated range.
        let mm = unsafe { msgvec.add(i * 64) };
        // msghdr layout: msg_name(8), msg_namelen(4), msg_iov(8), msg_iovlen(8),
        //                 msg_control(8), msg_controllen(8), msg_flags(4) = 48 bytes
        // SAFETY: mm validated; reading iovec fields at known offsets.
        let msg_name_ptr = unsafe {
            crate::arch::riscv64::uaccess::get_user::<*const u8>(mm as *const *const u8)
                .unwrap_or(core::ptr::null())
        };
        let msg_iov_ptr = unsafe { get_user_usize(mm.add(16) as usize) };
        let msg_iovlen = unsafe { get_user_usize(mm.add(24) as usize) };
        // R20-2: bound the iovec count (UIO_MAXIOV) — an unbounded
        // user u64 here looped the kernel over wild pointers (each
        // iteration a raw kernel deref of msg_iov_ptr+j*16). Stop the
        // whole batch, mirroring Linux's -EMSGSIZE on __sys_sendmmsg.
        if msg_iovlen > 1024 {
            if total_sent == 0 {
                return -(errno::EMSGSIZE as i64);
            }
            break;
        }
        // R20-3: range-check the iovec array before raw deref (see
        // sys_sendmsg); return partial success on a bad one.
        if !crate::arch::riscv64::uaccess::access_ok(msg_iov_ptr as usize, msg_iovlen * 16) {
            if total_sent == 0 {
                return -(errno::EFAULT as i64);
            }
            return total_sent as i64;
        }

        // Gather data from iovec
        let mut buf = alloc::vec::Vec::new();
        let mut total_len = 0usize;
        for j in 0..msg_iovlen {
            // SAFETY: iovec fields at validated offset; iov_base validated below.
            let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(j * 16)) };
            let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(j * 16 + 8)) };
            if iov_len > 0 {
                if !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
                    if total_sent == 0 {
                        return -(errno::EFAULT as i64);
                    }
                    return total_sent as i64; // Return partial success
                }
                // R7-D4: bound the aggregate (see sys_sendmsg).
                // R32-B2: check the RUNNING total inside the loop — the
                // old per-iov-only check let each message gather up to
                // 1024 × 256KB before sending (heap over-allocation).
                if total_len.saturating_add(iov_len) > crate::syscall::io::RW_CHUNK.saturating_mul(4)
                {
                    if total_sent == 0 {
                        return -(errno::EMSGSIZE as i64);
                    }
                    return total_sent as i64;
                }
                // SAFETY: iov_base validated with access_ok; gather through the
                // exception-table copy path (SUM=0 safe).
                let start = buf.len();
                buf.resize(start + iov_len, 0);
                if unsafe {
                    crate::arch::riscv64::uaccess::copy_from_user(
                        buf.as_mut_ptr().add(start),
                        iov_base as *const u8,
                        iov_len,
                    )
                } > 0
                {
                    if total_sent == 0 {
                        return -(errno::EFAULT as i64);
                    }
                    return total_sent as i64;
                }
                total_len += iov_len;
            }
        }

        if total_len == 0 {
            break;
        }

        // W3: UDP above 65507 → EMSGSIZE (first failure semantics).
        if socket.sock_type == crate::net::socket::SocketType::Udp
            && total_len > crate::net::udp::UDP_MAX_DATAGRAM
        {
            if total_sent == 0 {
                return -(errno::EMSGSIZE as i64);
            }
            break;
        }

        // W3: per-message msg_name destination (same parse as sendmsg).
        // P1 IPv6: family-aware parse (v4-mapped normalizes to V4).
        let dest_addr = if !msg_name_ptr.is_null()
            && crate::arch::riscv64::uaccess::access_ok(msg_name_ptr as usize, 16)
        {
            match parse_sockaddr_inet(msg_name_ptr) {
                Ok(ParsedSockAddr::V4 { addr, port }) => {
                    Some((crate::net::ipv6::IpAddr::V4(addr), port))
                }
                Ok(ParsedSockAddr::V6 { addr, port }) => {
                    Some((crate::net::ipv6::IpAddr::V6(addr), port))
                }
                Err(_) => None,
            }
        } else {
            None
        };

        let sent = match crate::net::socket::socket_send_ctl(
            &socket,
            &buf,
            dest_addr,
            nonblock,
            deadline,
        ) {
            Ok(n) => n,
            // W3: first failure with nothing sent returns the errno.
            Err(e) => {
                if total_sent == 0 {
                    return e as i64;
                }
                break;
            }
        };
        // Write msg_len in mmsghdr
        // SAFETY: mm offset within validated msgvec range; put_user is the
        // exception-table copy path.
        unsafe {
            let _ = crate::arch::riscv64::uaccess::put_user(mm.add(56) as *mut u32, sent as u32);
        }
        total_sent += 1;
    }
    total_sent as i64
}

/// sys_recvmmsg - Receive multiple messages (NR 243)
///
/// # Arguments
/// - args[0]: fd - socket fd
/// - args[1]: msgvec - pointer to mmsghdr array
/// - args[2]: vlen - number of messages
/// - args[3]: flags - flags
/// - args[4]: timeout - pointer to timespec
///
/// W3 (ABI): first-failure semantics (error on message 0 returns the
/// errno; ≥1 received returns the count — 0 stays reserved for "no
/// messages"); the `timeout` timespec bounds the whole receive (Linux
/// recvmmsg contract); blocking/O_NONBLOCK/MSG_DONTWAIT honored.
pub fn sys_recvmmsg(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let msgvec = args[1] as *mut u8;
    let vlen = args[2] as u32;
    let flags = args[3] as i32;
    let timeout = args[4] as *const u8;

    if msgvec.is_null() || vlen == 0 {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(msgvec as usize, vlen as usize * 64) {
        return -(errno::EFAULT as i64);
    }

    // P0-1/P0-2: AF_UNIX / AF_NETLINK — route each mmsghdr through the
    // single-message sys_recvmsg (msg_len written back at offset 56; the
    // recvmmsg batch timeout is approximated by SO_RCVTIMEO here).
    if crate::net::unix::unix_file_of(fd as usize).is_some()
        || crate::net::netlink::netlink_file_of(fd as usize).is_some()
    {
        let mut total_recv = 0u32;
        for i in 0..vlen as usize {
            // SAFETY: msgvec access_ok-validated above; mm within range.
            let mm = unsafe { msgvec.add(i * 64) };
            let ret = sys_recvmsg([
                fd as u64,
                mm as usize as u64,
                flags as u64,
                0,
                0,
                0,
            ]);
            if ret < 0 {
                if total_recv == 0 {
                    return ret;
                }
                break;
            }
            // SAFETY: mm within validated range; put_user copies 4 bytes.
            unsafe {
                let _ = crate::arch::riscv64::uaccess::put_user(mm.add(56) as *mut u32, ret as u32);
            }
            total_recv += 1;
            if ret == 0 {
                break; // EOF
            }
        }
        return total_recv as i64;
    }

    let (socket, file_nonblock) = match socket_file_of(fd as usize) {
        Some(s) => s,
        None => return -(errno::ENOTSOCK as i64),
    };
    let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;

    // W3: recvmmsg's own timeout overrides SO_RCVTIMEO for the batch.
    let mut deadline = socket.rcvtimeo_deadline();
    if !timeout.is_null() && crate::arch::riscv64::uaccess::access_ok(timeout as usize, 16) {
        let mut ts = [0u8; 16];
        // SAFETY: timeout validated with access_ok(16).
        if unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(ts.as_mut_ptr(), timeout, 16)
        } == 0
        {
            let sec = i64::from_ne_bytes(ts[0..8].try_into().unwrap());
            let nsec = i64::from_ne_bytes(ts[8..16].try_into().unwrap());
            if sec < 0 || nsec < 0 {
                return -(errno::EINVAL as i64);
            }
            let us = (sec as u64).saturating_mul(1_000_000)
                + (nsec as u64) / 1_000;
            if us == 0 {
                // Zero timeout = poll once (deadline already due).
                deadline = Some(crate::drivers::timer::get_jiffies());
            } else {
                deadline = Some(crate::drivers::timer::get_jiffies() + (us / 10_000).max(1));
            }
        }
    }

    let mut total_recv = 0u32;
    for i in 0..vlen as usize {
        // SAFETY: msgvec validated with access_ok; mm offset within validated range.
        let mm = unsafe { msgvec.add(i * 64) };
        // SAFETY: mm validated; reading iovec fields at known offsets.
        let msg_iov_ptr = unsafe { get_user_usize(mm.add(16) as usize) };
        let msg_iovlen = unsafe { get_user_usize(mm.add(24) as usize) };

        // Calculate total buffer size
        let mut total_buf_len = 0usize;
        // R20-2: bound the iovec count (UIO_MAXIOV) — see sys_sendmmsg.
        if msg_iovlen > 1024 {
            if total_recv == 0 {
                return -(errno::EMSGSIZE as i64);
            }
            break;
        }
        // R20-3: range-check the iovec array before raw deref (see
        // sys_sendmsg); return partial success on a bad one.
        if !crate::arch::riscv64::uaccess::access_ok(msg_iov_ptr as usize, msg_iovlen * 16) {
            if total_recv == 0 {
                return -(errno::EFAULT as i64);
            }
            return total_recv as i64;
        }
        for j in 0..msg_iovlen {
            // SAFETY: iovec fields at validated offset; iov_base validated below.
            let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(j * 16)) };
            let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(j * 16 + 8)) };
            if iov_len > 0 && !crate::arch::riscv64::uaccess::access_ok(iov_base, iov_len) {
                if total_recv == 0 {
                    return -(errno::EFAULT as i64);
                }
                return total_recv as i64;
            }
            total_buf_len += iov_len;
            // R7-D4: bound the aggregate (see sys_recvmsg).
            if total_buf_len > crate::syscall::io::RW_CHUNK.saturating_mul(4) {
                if total_recv == 0 {
                    return -(errno::EMSGSIZE as i64);
                }
                return total_recv as i64;
            }
        }

        if total_buf_len == 0 {
            break;
        }

        let mut buf = alloc::vec![0u8; total_buf_len];
        let (bytes_read, _src) =
            match crate::net::socket::socket_recv_ctl(&socket, &mut buf, nonblock, deadline) {
                Ok(r) => r,
                Err(e) => {
                    // W3: first would-block/error with nothing received
                    // returns the errno (0 stays "no messages").
                    if total_recv == 0 {
                        return e as i64;
                    }
                    break;
                }
            };
        // Scatter data back to iovecs
        let mut offset = 0usize;
        for j in 0..msg_iovlen {
            if offset >= bytes_read { break; }
            // SAFETY: iovec fields at validated offset; copy_len bounds the write.
            let iov_base = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(j * 16)) };
            let iov_len = unsafe { get_user_usize(msg_iov_ptr.wrapping_add(j * 16 + 8)) };
            let copy_len = core::cmp::min(iov_len, bytes_read - offset);
            if copy_len > 0 {
                // R20-3: exception-table copy — see sys_recvmsg.
                let uncopied = unsafe {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        iov_base as *mut u8,
                        buf.as_ptr().add(offset),
                        copy_len,
                    )
                };
                if uncopied > 0 {
                    if total_recv > 0 {
                        return total_recv as i64;
                    }
                    return -(errno::EFAULT as i64);
                }
                offset += copy_len;
            }
        }
        // SAFETY: mm offset within validated msgvec range; put_user is the
        // exception-table copy path.
        unsafe {
            let _ = crate::arch::riscv64::uaccess::put_user(mm.add(56) as *mut u32, bytes_read as u32);
        }
        total_recv += 1;
        if bytes_read == 0 {
            break; // EOF
        }
    }
    total_recv as i64
}

/// sys_accept4 - Accept connection (with flags)
///
/// # Arguments
/// - args[0]: fd - socket file descriptor
/// - args[1]: addr - pointer to sockaddr (output)
/// - args[2]: addrlen - pointer to address length (input/output)
/// - args[3]: flags - SOCK_CLOEXEC, SOCK_NONBLOCK
///
/// W3: SOCK_CLOEXEC / SOCK_NONBLOCK are honored (socket_create_accepted
/// applies them to the new fd); unknown flag bits are EINVAL like Linux.
pub fn sys_accept4(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let addr_ptr = args[1] as *mut u8;
    let addrlen_ptr = args[2] as *mut u32;
    let flags = args[3] as i32;

    const KNOWN_ACCEPT_FLAGS: i32 =
        crate::net::socket::SOCK_CLOEXEC_FLAG | crate::net::socket::SOCK_NONBLOCK_FLAG;
    if flags & !KNOWN_ACCEPT_FLAGS != 0 {
        return -(errno::EINVAL as i64);
    }

    sys_accept_common(fd, flags, addr_ptr, addrlen_ptr)
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
    let flags = args[3] as i32;
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

    // P0-1: AF_UNIX — receive into a kernel buffer, report the sender path.
    if let Some((usock, file_nonblock)) = crate::net::unix::unix_file_of(fd) {
        let stage = len.min(crate::syscall::io::RW_CHUNK);
        let mut kbuf = alloc::vec::Vec::new();
        if kbuf.try_reserve_exact(stage).is_err() {
            return -(errno::ENOMEM as i64);
        }
        kbuf.resize(stage, 0);
        let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
        let deadline = usock.rcvtimeo_deadline();
        return match crate::net::unix::unix_recv_ctl(&usock, kbuf.as_mut_slice(), nonblock, deadline)
        {
            Ok(r) => {
                if r.len > 0 {
                    // SAFETY: buf_ptr validated with access_ok(len) above;
                    // r.len <= stage <= len.
                    if unsafe {
                        crate::arch::riscv64::uaccess::copy_to_user(buf_ptr, kbuf.as_ptr(), r.len)
                    } != 0
                    {
                        return -(errno::EFAULT as i64);
                    }
                }
                if let Some(src) = r.src {
                    if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
                        if crate::arch::riscv64::uaccess::access_ok(
                            addr_ptr as usize,
                            crate::net::unix::SOCKADDR_UN_LEN,
                        ) && crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4)
                        {
                            // SAFETY: pointers validated with access_ok.
                            unsafe {
                                crate::net::unix::put_sockaddr_un(
                                    addr_ptr,
                                    addrlen_ptr,
                                    Some(&src),
                                );
                            }
                        }
                    }
                }
                r.len as i64
            }
            Err(e) => e as i64,
        };
    }

    // P0-2: AF_NETLINK — one rtnetlink message + sockaddr_nl source.
    if let Some((nlsock, file_nonblock)) = crate::net::netlink::netlink_file_of(fd) {
        let stage = len.min(crate::syscall::io::RW_CHUNK);
        let mut kbuf = alloc::vec::Vec::new();
        if kbuf.try_reserve_exact(stage).is_err() {
            return -(errno::ENOMEM as i64);
        }
        kbuf.resize(stage, 0);
        let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
        let deadline = nlsock.rcvtimeo_deadline();
        return match crate::net::netlink::netlink_recv(&nlsock, kbuf.as_mut_slice(), nonblock, deadline)
        {
            Ok(n) => {
                if n > 0 {
                    // SAFETY: buf_ptr validated with access_ok(len) above.
                    if unsafe {
                        crate::arch::riscv64::uaccess::copy_to_user(buf_ptr, kbuf.as_ptr(), n)
                    } != 0
                    {
                        return -(errno::EFAULT as i64);
                    }
                }
                if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
                    if crate::arch::riscv64::uaccess::access_ok(addr_ptr as usize, 12)
                        && crate::arch::riscv64::uaccess::access_ok(addrlen_ptr as usize, 4)
                    {
                        // SAFETY: pointers validated with access_ok.
                        unsafe {
                            crate::net::netlink::put_sockaddr_nl(addr_ptr, addrlen_ptr);
                        }
                    }
                }
                n as i64
            }
            Err(e) => e as i64,
        };
    }

    // Get socket through the per-process fd table (review NET-C3)
    let socket = match crate::net::socket::get_socket_from_fd(fd) {
        Some(s) => s,
        None => return -(errno::ENOTSOCK as i64),
    };

    // W3: blocking / MSG_DONTWAIT / O_NONBLOCK semantics (the socket used
    // to be permanently non-blocking — recvfrom returned EAGAIN forever).
    let file_nonblock = {
        let fdtable = match crate::sched::get_current_fdtable() {
            Some(t) => t,
            None => return -(errno::EBADF as i64),
        };
        match fdtable.get_file(fd) {
            Some(f) => (f.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK) != 0,
            None => return -(errno::EBADF as i64),
        }
    };
    let nonblock = file_nonblock || (flags & MSG_DONTWAIT) != 0;
    let deadline = socket.rcvtimeo_deadline();

    // R35: receive into a KERNEL buffer and copy to user after the socket
    // layer returns — the old raw `from_raw_parts_mut(buf_ptr, len)` ran
    // writes to user memory inside Socket::recv → TcpSocket::recv while
    // holding TCP_TABLE_LOCK (uaccess exceptions / page-fault detours in
    // the critical section). Staged at RW_CHUNK like sys_read (SYSA-C1): a
    // partial return is POSIX-legal for stream sockets, and a huge len
    // can never OOM the heap. try_reserve_exact turns OOM into ENOMEM.
    let stage = len.min(crate::syscall::io::RW_CHUNK);
    let mut kbuf = alloc::vec::Vec::new();
    if kbuf.try_reserve_exact(stage).is_err() {
        return -(errno::ENOMEM as i64);
    }
    kbuf.resize(stage, 0);

    // W3: the blocking engine also drains loopback between attempts (the
    // explicit ethernet_poll the old code did is folded in).
    let (bytes_read, src_addr) =
        match crate::net::socket::socket_recv_ctl(&socket, kbuf.as_mut_slice(), nonblock, deadline)
        {
            Ok(r) => r,
            Err(e) => return e as i64,
        };

    if bytes_read > 0 {
        // SAFETY: buf_ptr validated with access_ok(len) above;
        // bytes_read <= stage <= len.
        if unsafe {
            crate::arch::riscv64::uaccess::copy_to_user(
                buf_ptr,
                kbuf.as_ptr(),
                bytes_read,
            )
        } != 0
        {
            return -(errno::EFAULT as i64);
        }
    }
    // If address pointer is provided, write source address
    if let Some((addr, port)) = src_addr {
        if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
            // SAFETY: addr_ptr/addrlen_ptr validated with access_ok; exception-table copy.
            // P1 IPv6: family-aware shape (v6 sockets get sockaddr_in6).
            unsafe {
                put_sockaddr_family(&socket, addr_ptr, addrlen_ptr, port, addr);
            }
        }
    }
    bytes_read as i64
}
