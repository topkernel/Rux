//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! UDP Protocol

use crate::net::buffer::SkBuff;
use crate::net::ipv4::{route, checksum};
use crate::config::UDP_SOCKET_TABLE_SIZE;

/// UDP header length
pub const UDP_HLEN: usize = 8;

/// UDP maximum data length
pub const UDP_MAX_DATAGRAM: usize = 65507;

/// UDP port number
pub type UdpPort = u16;

/// UDP header
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UdpHdr {
    /// Source port
    pub source: UdpPort,
    /// Destination port
    pub dest: UdpPort,
    /// Length
    pub len: u16,
    /// Checksum
    pub check: u16,
}

impl UdpHdr {
    /// Create UDP header from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<&UdpHdr> {
        if data.len() < UDP_HLEN {
            return None;
        }

        // SAFETY: data has at least UDP_HLEN bytes and is aligned to UdpHdr layout.
        unsafe {
            Some(&*(data.as_ptr() as *const UdpHdr))
        }
    }

    /// Get source port
    pub fn source(&self) -> UdpPort {
        u16::from_be(self.source)
    }

    /// Get destination port
    pub fn dest(&self) -> UdpPort {
        u16::from_be(self.dest)
    }

    /// Get length
    pub fn len(&self) -> u16 {
        u16::from_be(self.len)
    }

    /// Get checksum
    pub fn check(&self) -> u16 {
        u16::from_be(self.check)
    }
}

/// UDP packet
#[derive(Clone)]
pub struct UdpPacket {
    pub data: alloc::vec::Vec<u8>,
    pub src_addr: u32,
    pub src_port: u16,
    /// P1 IPv6: source address for v6 datagrams (unspecified for v4)
    pub src_addr6: crate::net::ipv6::Ipv6Addr,
}

/// UDP Socket structure
#[repr(C)]
pub struct UdpSocket {
    /// Local port
    pub local_port: UdpPort,
    /// Remote port
    pub remote_port: UdpPort,
    /// Remote IP address
    pub remote_ip: u32,
    /// Local IP address
    pub local_ip: u32,
    /// P1 IPv6: this socket operates on pure v6 addresses (a v4-mapped
    /// peer is normalized to the v4 fields at the syscall boundary)
    pub is_v6: bool,
    /// P1 IPv6: local address (unspecified = IN6ADDR_ANY)
    pub local_ip6: crate::net::ipv6::Ipv6Addr,
    /// P1 IPv6: remote address
    pub remote_ip6: crate::net::ipv6::Ipv6Addr,
    /// Whether bound
    pub bound: bool,
    /// Whether connected
    pub connected: bool,
    /// W3: pending protocol error (positive errno) — ICMP errors on a
    /// connected UDP socket (udp_v4_err). Read-and-cleared via SO_ERROR.
    pub pending_error: i32,
    /// P2 SO_BROADCAST mirrored from the VFS layer (udp_set_broadcast):
    /// sendto to a broadcast address without it fails with EACCES.
    pub broadcast: bool,
    /// P2 IP_TTL mirrored from the VFS layer (udp_set_ttl): 0 = default.
    pub ttl: u8,
    /// Receive buffer
    pub recv_buffer: alloc::collections::VecDeque<UdpPacket>,
    /// R24 (MED-9): queued payload bytes — pairs with UDP_RCVBUF_BUDGET.
    pub recv_bytes: usize,
}

impl UdpSocket {
    /// Create new UDP Socket
    pub fn new() -> Self {
        Self {
            local_port: 0,
            remote_port: 0,
            remote_ip: 0,
            // INADDR_ANY: the old hardcoded 192.168.1.100 made the socket
            // match nothing in a slirp/QEMU environment (review NET-H6).
            local_ip: 0,
            is_v6: false,
            local_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            remote_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            bound: false,
            connected: false,
            pending_error: 0,
            broadcast: false,
            ttl: 0,
            recv_buffer: alloc::collections::VecDeque::new(),
            recv_bytes: 0,
        }
    }

    /// Bind to local address/port
    ///
    /// # Arguments
    /// - `ip`: Local IP (0 = INADDR_ANY)
    /// - `port`: Port number
    pub fn bind(&mut self, ip: u32, port: UdpPort) -> Result<(), ()> {
        self.local_ip = ip;
        self.local_port = port;
        self.bound = true;
        Ok(())
    }

    /// Connect to remote address
    ///
    /// # Arguments
    /// - `ip`: IP address
    /// - `port`: Port number
    pub fn connect(&mut self, ip: u32, port: UdpPort) -> Result<(), ()> {
        self.remote_ip = ip;
        self.remote_port = port;
        self.connected = true;
        Ok(())
    }

    /// Disconnect
    pub fn disconnect(&mut self) {
        self.remote_ip = 0;
        self.remote_port = 0;
        self.connected = false;
    }

    /// Enqueue packet to receive buffer.
    ///
    /// R24 (MED-9): drop the datagram once the queued byte budget
    /// (UDP_RCVBUF_BUDGET) is exhausted — an unbounded queue let a remote
    /// flooder OOM the kernel heap. Mirrors Linux's sk_rcvbuf drop behavior.
    pub fn enqueue_packet(&mut self, packet: UdpPacket) {
        if self.recv_bytes + packet.data.len() > UDP_RCVBUF_BUDGET {
            return; // receive buffer full — drop
        }
        self.recv_bytes += packet.data.len();
        self.recv_buffer.push_back(packet);
    }

    /// Dequeue packet from receive buffer
    pub fn dequeue_packet(&mut self) -> Option<UdpPacket> {
        let packet = self.recv_buffer.pop_front()?;
        self.recv_bytes = self.recv_bytes.saturating_sub(packet.data.len());
        Some(packet)
    }
}

/// Global UDP socket table
struct UdpSocketTable {
    sockets: [Option<UdpSocket>; UDP_SOCKET_TABLE_SIZE],
    count: usize,
}

impl UdpSocketTable {
    const fn new() -> Self {
        const NONE: Option<UdpSocket> = None;
        Self {
            sockets: [NONE; UDP_SOCKET_TABLE_SIZE],
            count: 0,
        }
    }

    /// Allocate socket slot. Reuses freed slots before growing.
    fn alloc(&mut self) -> Result<usize, ()> {
        // First try to reuse a freed slot
        for i in 0..self.count {
            if self.sockets[i].is_none() {
                self.sockets[i] = Some(UdpSocket::new());
                return Ok(i);
            }
        }

        // No freed slots; grow the table
        if self.count >= UDP_SOCKET_TABLE_SIZE {
            return Err(());
        }

        let fd = self.count;
        self.sockets[fd] = Some(UdpSocket::new());
        self.count += 1;
        Ok(fd)
    }

    /// Free socket
    fn free(&mut self, fd: usize) {
        if fd < self.count {
            self.sockets[fd] = None;
        }
    }

    /// Get socket
    fn get(&self, fd: usize) -> Option<&UdpSocket> {
        if fd < self.count {
            self.sockets[fd].as_ref()
        } else {
            None
        }
    }

    /// Get mutable socket
    fn get_mut(&mut self, fd: usize) -> Option<&mut UdpSocket> {
        if fd < self.count {
            self.sockets[fd].as_mut()
        } else {
            None
        }
    }
}

/// Global UDP socket table
static mut UDP_SOCKET_TABLE: UdpSocketTable = UdpSocketTable::new();

/// R24 (HIGH-6, mirroring R21-N1's TCP_TABLE_LOCK): the UDP table is
/// mutated concurrently from syscalls (socket/bind/connect/close/send/recv)
/// and the NetRx softirq (udp_rcv) on the 4-CPU kernel. irqsave because the
/// softirq side can run inline at irq_exit.
pub static UDP_TABLE_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

/// R24 (MED-9): per-socket receive budget — enqueue_packet drops new
/// datagrams once the queued byte total exceeds this, so a remote flooder
/// cannot grow recv_buffer without bound (remote OOM).
pub const UDP_RCVBUF_BUDGET: usize = 128 * 1024;

/// Allocate UDP socket
///
/// # Returns
/// Socket file descriptor
pub fn udp_socket_alloc() -> Result<i32, i32> {
    // SAFETY: UDP_SOCKET_TABLE is a global static accessed under
    // UDP_TABLE_LOCK (R24).
    unsafe {
        let _g = UDP_TABLE_LOCK.lock_irqsave();
        match UDP_SOCKET_TABLE.alloc() {
            Ok(fd) => Ok(fd as i32),
            // W3: protocol table exhausted → EMFILE (Linux), not EIO
            Err(_) => Err(-24), // EMFILE
        }
    }
}

/// Free UDP socket
///
/// # Arguments
/// - `fd`: Socket file descriptor
pub fn udp_socket_free(fd: i32) {
    // SAFETY: UDP_SOCKET_TABLE is a global; fd was returned by udp_socket_alloc.
    unsafe {
        let _g = UDP_TABLE_LOCK.lock_irqsave();
        UDP_SOCKET_TABLE.free(fd as usize);
    }
}

/// Get UDP socket
///
/// # Arguments
/// - `fd`: Socket file descriptor
///
/// # Returns
/// Socket reference
pub fn udp_socket_get(fd: i32) -> Option<&'static mut UdpSocket> {
    // SAFETY: UDP_SOCKET_TABLE is a global; caller ensures no concurrent access.
    unsafe {
        UDP_SOCKET_TABLE.get_mut(fd as usize)
    }
}

/// Bind socket to local address and port
///
/// # Arguments
/// - `fd`: Socket file descriptor (UDP protocol-table index)
/// - `ip`: Local IP address (0 = INADDR_ANY)
/// - `port`: Port number
///
/// # Returns
/// 0 on success, error code on failure
pub fn udp_bind(fd: i32, ip: u32, port: UdpPort) -> i32 {
    // R24 (HIGH-6): table leaf lock — serializes against udp_rcv (NetRx
    // softirq) and other syscalls touching the same slot.
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global; fd was returned by udp_socket_alloc.
    unsafe {
        // R32-N9: reject a port already held by another bound socket — the
        // old code accepted every bind and udp_rcv then delivered to
        // whichever slot the scan found first. Port 0 (ephemeral) never
        // conflicts. A specific-address bind may coexist with an
        // INADDR_ANY(0) bind on the same port only if this bind itself is
        // the ANY one (matching Linux's wildcard/exact precedence is not
        // implemented — first binder wins).
        let effective_port = if port == 0 {
            // W3: bind(0) assigns the ephemeral port IMMEDIATELY (Linux
            // semantics — getsockname reports it right after bind) instead
            // of leaving the socket unbound until the first sendto.
            match udp_alloc_ephemeral_port() {
                Some(p) => p,
                None => return -99, // EADDRNOTAVAIL — ephemeral range exhausted
            }
        } else {
            for i in 0..UDP_SOCKET_TABLE.count {
                if i == fd as usize {
                    continue;
                }
                if let Some(s) = UDP_SOCKET_TABLE.sockets[i].as_ref() {
                    if s.bound && s.local_port == port {
                        return -98; // EADDRINUSE
                    }
                }
            }
            port
        };
        if let Some(socket) = UDP_SOCKET_TABLE.get_mut(fd as usize) {
            match socket.bind(ip, effective_port) {
                Ok(()) => 0,
                Err(()) => -5, // EIO
            }
        } else {
            -5 // EBADF
        }
    }
}

/// Next ephemeral port for UDP auto-bind (W3).
static NEXT_UDP_EPHEMERAL_PORT: core::sync::atomic::AtomicU16 =
    core::sync::atomic::AtomicU16::new(32768);
const UDP_EPHEMERAL_PORT_MAX: u16 = 60999;

/// Allocate an unused UDP local port in the ephemeral range.
/// Caller must hold UDP_TABLE_LOCK.
fn udp_alloc_ephemeral_port() -> Option<UdpPort> {
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        for _ in 0..(UDP_EPHEMERAL_PORT_MAX - 32768 + 1) {
            let port = NEXT_UDP_EPHEMERAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let port = if port > UDP_EPHEMERAL_PORT_MAX {
                port % UDP_EPHEMERAL_PORT_MAX + 1024
            } else {
                port
            };
            let in_use = (0..UDP_SOCKET_TABLE.count).any(|i| {
                UDP_SOCKET_TABLE
                    .sockets
                    .get(i)
                    .and_then(|s| s.as_ref())
                    .map(|s| s.bound && s.local_port == port)
                    .unwrap_or(false)
            });
            if !in_use {
                return Some(port);
            }
        }
        None
    }
}

/// W3: locked read of a slot's bound local port (ephemeral bind readback).
pub fn udp_local_port(fd: i32) -> u16 {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe { UDP_SOCKET_TABLE.get(fd as usize).map(|s| s.local_port).unwrap_or(0) }
}

/// W3: read-and-clear the slot's pending error (SO_ERROR semantics).
pub fn udp_take_pending_error(fd: i32) -> i32 {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        match UDP_SOCKET_TABLE.get_mut(fd as usize) {
            Some(s) => core::mem::replace(&mut s.pending_error, 0),
            None => 0,
        }
    }
}

/// Connect a UDP socket to a remote address (R24: locked entry point used
/// by the socket layer instead of raw udp_socket_get).
pub fn udp_connect(fd: i32, ip: u32, port: UdpPort) -> i32 {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global; fd was returned by udp_socket_alloc.
    unsafe {
        match UDP_SOCKET_TABLE.get_mut(fd as usize) {
            Some(socket) => {
                let _ = socket.connect(ip, port);
                0
            }
            None => -9, // EBADF
        }
    }
}

/// Send UDP packet
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
///
/// # Returns
/// Bytes sent on success, error code on failure
pub fn udp_send(fd: i32, buf: &[u8]) -> isize {
    // R24 (HIGH-6): leaf lock — held across the whole send so the &mut into
    // the table cannot race udp_rcv. No RX re-entry below: virtio xmit only
    // DMAs, loopback TX only queues.
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // Get socket
    let socket = match unsafe { UDP_SOCKET_TABLE.get_mut(fd as usize) } {
        Some(s) => s,
        None => return -9, // EBADF
    };

    // Get destination address
    // P1 IPv6: a v6-connected socket sends through the v6 wire path.
    if socket.is_v6 {
        if !socket.connected {
            return -107; // ENOTCONN
        }
        let dest = socket.remote_ip6;
        let port = socket.remote_port;
        return udp_send_locked6(socket, buf, dest, port);
    }

    let (dest_ip, dest_port) = if socket.connected {
        (socket.remote_ip, socket.remote_port)
    } else {
        // Unconnected UDP socket needs destination address specified
        return -107; // ENOTCONN
    };

    udp_send_locked(socket, buf, dest_ip, dest_port)
}

/// Send UDP packet to specified address
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
/// - `dest_ip`: Destination IP address
/// - `dest_port`: Destination port
///
/// # Returns
/// Bytes sent on success, error code on failure
pub fn udp_sendto(fd: i32, buf: &[u8], dest_ip: u32, dest_port: u16) -> isize {
    // R24 (HIGH-6): leaf lock, same rationale as udp_send.
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // Get socket
    let socket = match unsafe { UDP_SOCKET_TABLE.get_mut(fd as usize) } {
        Some(s) => s,
        None => return -9, // EBADF
    };

    udp_send_locked(socket, buf, dest_ip, dest_port)
}

// ============================================================================
// P1 IPv6 UDP
// ============================================================================

/// Bind a UDP slot to a pure v6 local address/port. Mirrors udp_bind
/// (ephemeral assignment, same-family conflict check).
pub fn udp_bind6(fd: i32, ip6: crate::net::ipv6::Ipv6Addr, port: UdpPort) -> i32 {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        let effective_port = if port == 0 {
            match udp_alloc_ephemeral_port() {
                Some(p) => p,
                None => return -99, // EADDRNOTAVAIL
            }
        } else {
            // Conflict check within the same family (v4/wildcard v6 dual
            // binding is out of scope — first binder wins, like udp_bind).
            for i in 0..UDP_SOCKET_TABLE.count {
                if i == fd as usize {
                    continue;
                }
                if let Some(s) = UDP_SOCKET_TABLE.sockets[i].as_ref() {
                    if s.bound && s.local_port == port && s.is_v6 {
                        return -98; // EADDRINUSE
                    }
                }
            }
            port
        };
        match UDP_SOCKET_TABLE.get_mut(fd as usize) {
            Some(socket) => {
                socket.is_v6 = true;
                socket.local_ip6 = ip6;
                socket.local_port = effective_port;
                socket.bound = true;
                0
            }
            None => -5, // EBADF
        }
    }
}

/// Connect a UDP slot to a pure v6 remote (R24 discipline: locked entry).
pub fn udp_connect6(fd: i32, ip6: crate::net::ipv6::Ipv6Addr, port: UdpPort) -> i32 {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        match UDP_SOCKET_TABLE.get_mut(fd as usize) {
            Some(socket) => {
                socket.is_v6 = true;
                socket.remote_ip6 = ip6;
                socket.remote_port = port;
                socket.connected = true;
                0
            }
            None => -9, // EBADF
        }
    }
}

/// Send a UDP datagram over IPv6 (family-aware variant of udp_sendto).
pub fn udp_sendto6(
    fd: i32,
    buf: &[u8],
    dest6: crate::net::ipv6::Ipv6Addr,
    dest_port: u16,
) -> isize {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global; fd was returned by udp_socket_alloc.
    let socket = match unsafe { UDP_SOCKET_TABLE.get_mut(fd as usize) } {
        Some(s) => s,
        None => return -9, // EBADF
    };
    udp_send_locked6(socket, buf, dest6, dest_port)
}

/// Common v6 transmit path (caller holds UDP_TABLE_LOCK). The UDP checksum
/// is MANDATORY over IPv6 (RFC 8200 §8.1) and covers the 128-bit
/// pseudo-header addresses.
fn udp_send_locked6(
    socket: &mut UdpSocket,
    buf: &[u8],
    dest6: crate::net::ipv6::Ipv6Addr,
    dest_port: u16,
) -> isize {
    use crate::net::ipv6::{self, next_header};

    if buf.is_empty() {
        return 0;
    }

    // Multicast / link-local requires no SO_BROADCAST gate (v6 has no
    // broadcast; ff00::/8 is multicast).

    // Implicit ephemeral bind at first send.
    if !socket.bound {
        match udp_alloc_ephemeral_port() {
            Some(p) => {
                socket.local_port = p;
                socket.bound = true;
            }
            None => return -99, // EADDRNOTAVAIL
        }
    }
    socket.is_v6 = true;

    let mut skb = match crate::net::buffer::alloc_skb((UDP_HLEN + buf.len()) as u32) {
        Some(skb) => skb,
        None => return -12, // ENOMEM
    };

    if udp_build_packet(&mut skb, socket.local_port, dest_port, buf).is_err() {
        crate::net::buffer::kfree_skb(skb);
        return -5; // EIO
    }

    // Source: the socket's bound v6 address, else our SLAAC link-local.
    let src6 = if crate::net::ipv6::is_unspecified(&socket.local_ip6) {
        match crate::net::ipv6::get_link_local() {
            Some(ll) => ll,
            None => {
                crate::net::buffer::kfree_skb(skb);
                return -99; // EADDRNOTAVAIL — no v6 source configured
            }
        }
    } else {
        socket.local_ip6
    };

    // Mandatory checksum over header + payload + v6 pseudo-header.
    // SAFETY: skb.data holds UDP_HLEN valid header bytes; payload follows.
    unsafe {
        let hdr = &mut *(skb.data as *mut UdpHdr);
        let total = (UDP_HLEN + buf.len()) as u32;
        hdr.check = 0;
        let seg = core::slice::from_raw_parts(skb.data as *const u8, total as usize);
        let mut csum = ipv6::transport_checksum6(&src6, &dest6, next_header::UDP, seg);
        if csum == 0 {
            csum = 0xFFFF; // 0x0000 would read as "computed 0"
        }
        hdr.check = csum.to_be();
    }

    match ipv6::ipv6_send_hops(skb, &src6, &dest6, next_header::UDP, socket.ttl) {
        Ok(()) => buf.len() as isize,
        Err(_) => -5, // EIO (NS-triggered neighbor loss included)
    }
}

/// Receive with a family-aware source address: v4 datagrams report
/// IpAddr::V4, v6 datagrams report IpAddr::V6. Used by the VFS layer for
/// recvfrom/recvmsg on AF_INET6 sockets.
pub fn udp_recvfrom_ext(
    fd: i32,
    buf: &mut [u8],
) -> Result<(isize, crate::net::ipv6::IpAddr, u16), isize> {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    let socket = match unsafe { UDP_SOCKET_TABLE.get_mut(fd as usize) } {
        Some(s) => s,
        None => return Err(-9), // EBADF
    };

    match socket.dequeue_packet() {
        Some(packet) => {
            let copy_len = packet.data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&packet.data[..copy_len]);
            // v6 sockets report the datagram's v6 source; a v4 datagram
            // (cannot normally match a v6 slot) falls back to its u32.
            let src = if socket.is_v6
                && packet.src_addr6 != crate::net::ipv6::IPV6_ADDR_UNSPECIFIED
            {
                crate::net::ipv6::IpAddr::V6(packet.src_addr6)
            } else {
                crate::net::ipv6::IpAddr::V4(packet.src_addr)
            };
            Ok((copy_len as isize, src, packet.src_port))
        }
        None => {
            if socket.pending_error != 0 {
                Err(-(socket.pending_error as isize))
            } else {
                Err(-11) // EAGAIN
            }
        }
    }
}

/// Receive and process an IPv6 UDP packet (called from ipv6_rcv with the
/// base header pulled). Delivery mirrors udp_rcv: exact/wildcard local
/// match, connected sockets filter the remote.
pub fn udp_rcv6(
    skb: &SkBuff,
    src6: &crate::net::ipv6::Ipv6Addr,
    dst6: &crate::net::ipv6::Ipv6Addr,
) -> Result<(), ()> {
    use crate::net::ipv6::{self, next_header};

    let udp_hdr = udp_parse_packet(skb).ok_or(())?;

    // Checksum is mandatory in v6 — a zero field is a protocol violation
    // (RFC 8200 §8.1) and the datagram is dropped.
    if udp_hdr.check() == 0 {
        return Ok(());
    }
    let data_len = (udp_hdr.len() as usize).saturating_sub(UDP_HLEN);
    // SAFETY: udp_parse_packet validated the length against skb.len.
    let data = if data_len > 0 {
        unsafe { core::slice::from_raw_parts(skb.data.add(UDP_HLEN), data_len) }
    } else {
        &[]
    };
    // Verify over the full segment (header with stored checksum + data):
    // a valid datagram sums to zero.
    // SAFETY: header bytes [0, UDP_HLEN) are validated.
    let seg = unsafe {
        core::slice::from_raw_parts(skb.data as *const u8, UDP_HLEN + data_len)
    };
    if ipv6::transport_checksum6(src6, dst6, next_header::UDP, seg) != 0 {
        return Ok(()); // silently drop
    }

    let src_port = UdpPort::from_be(udp_hdr.source);
    let dest_port = UdpPort::from_be(udp_hdr.dest);

    let mut delivered_fd: Option<i32> = None;
    {
        let _g = UDP_TABLE_LOCK.lock_irqsave();
        // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
        unsafe {
            for i in 0..UDP_SOCKET_TABLE.count {
                if let Some(ref mut socket) = UDP_SOCKET_TABLE.sockets[i] {
                    if socket.is_v6
                        && socket.bound
                        && socket.local_port == dest_port
                        && (crate::net::ipv6::is_unspecified(&socket.local_ip6)
                            || socket.local_ip6 == *dst6)
                        && (!socket.connected
                            || (socket.remote_ip6 == *src6
                                && socket.remote_port == src_port))
                    {
                        let packet = UdpPacket {
                            data: alloc::vec::Vec::from(data),
                            src_addr: 0,
                            src_port: src_port,
                            src_addr6: *src6,
                        };
                        socket.enqueue_packet(packet);
                        delivered_fd = Some(i as i32);
                        break;
                    }
                }
            }
        }
    }

    if let Some(fd) = delivered_fd {
        crate::net::socket::wake_udp_socket(fd);
    }
    // No ICMPv6 port-unreachable generation for undelivered datagrams
    // (P2 — the echo/reply NS/NA core is what P1 needs).

    Ok(())
}

/// Common UDP transmit path (W3). Caller holds UDP_TABLE_LOCK and provides
/// the destination.
///
/// W3 fixes folded in:
/// - implicit bind: an unbound socket gets an ephemeral local port at the
///   first send (the old path left local_port 0 — the wire packet carried
///   source port 0 and a reply could never be routed back);
/// - the skb is sized for the whole datagram (the fixed 1500-byte alloc
///   made every >1472-byte sendto fail skb_put and return EIO — larger
///   datagrams now flow through IPv4 fragmentation);
/// - the UDP checksum is computed on TX (it used to go out as 0).
fn udp_send_locked(socket: &mut UdpSocket, buf: &[u8], dest_ip: u32, dest_port: u16) -> isize {
    if buf.is_empty() {
        return 0;
    }

    // P2 SO_BROADCAST: sending to a broadcast address without the option
    // fails with EACCES (Linux udp_sendmsg ip_mc_sf_allow / EACCES gate).
    // Minimal broadcast set: the limited broadcast 255.255.255.255 and
    // directed subnet broadcasts (host part all-ones, last octet 255) —
    // we have no netmask model for the precise per-interface check.
    if !socket.broadcast
        && (dest_ip == 0xFFFF_FFFF || (dest_ip & 0xFF) == 0xFF)
    {
        return -13; // EACCES
    }

    // W3: implicit ephemeral bind at first send.
    if !socket.bound {
        match udp_alloc_ephemeral_port() {
            Some(p) => {
                socket.local_port = p;
                socket.bound = true;
            }
            None => return -99, // EADDRNOTAVAIL
        }
    }

    // W3: size the skb for the full datagram (max 65507 + 8 header fits u16).
    let mut skb = match crate::net::buffer::alloc_skb((UDP_HLEN + buf.len()) as u32) {
        Some(skb) => skb,
        None => return -12, // ENOMEM
    };

    // Build UDP header + data (udp_build_packet puts data into skb)
    if udp_build_packet(&mut skb, socket.local_port, dest_port, buf).is_err() {
        crate::net::buffer::kfree_skb(skb);
        return -5; // EIO
    }

    // Send to IP layer (source = the socket's bound address; 0 = device)
    let src_ip = if socket.local_ip == 0 {
        crate::net::arp::get_local_ip()
    } else {
        socket.local_ip
    };

    // W3: compute the UDP checksum (TX used to be sent with checksum 0 —
    // RFC 768 allows it, but strict peers/VMs drop those datagrams).
    // SAFETY: skb.data holds UDP_HLEN valid header bytes; payload follows.
    unsafe {
        let hdr = &*(skb.data as *const UdpHdr);
        let payload = core::slice::from_raw_parts(skb.data.add(UDP_HLEN), buf.len());
        let mut csum = udp_checksum(src_ip.to_be(), dest_ip.to_be(), hdr, payload);
        if csum == 0 {
            csum = 0xFFFF; // 0 would mean "no checksum" on the wire
        }
        (*(skb.data as *mut UdpHdr)).check = csum.to_be();
    }

    match crate::net::ipv4::ipv4_send_src_ttl(skb, src_ip, dest_ip, 17, socket.ttl) { // IPPROTO_UDP = 17
        Ok(()) => buf.len() as isize,
        Err(_) => -5, // EIO
    }
}

/// P2 SO_BROADCAST: mirror the socket-layer option into the protocol slot.
pub fn udp_set_broadcast(fd: i32, on: bool) {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global; protected by UDP_TABLE_LOCK.
    unsafe {
        if let Some(s) = UDP_SOCKET_TABLE.get_mut(fd as usize) {
            s.broadcast = on;
        }
    }
}

/// P2 IP_TTL: mirror the per-socket TTL into the protocol slot (0 =
/// system default 64).
pub fn udp_set_ttl(fd: i32, ttl: u8) {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global; protected by UDP_TABLE_LOCK.
    unsafe {
        if let Some(s) = UDP_SOCKET_TABLE.get_mut(fd as usize) {
            s.ttl = ttl;
        }
    }
}

/// Receive UDP packet
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
/// - `len`: Buffer length
///
/// # Returns
/// Bytes received on success, error code on failure
pub fn udp_recv(fd: i32, buf: &mut [u8], _len: usize) -> isize {
    // R24 (HIGH-6): leaf lock around the dequeue.
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // Get socket
    let socket = match unsafe { UDP_SOCKET_TABLE.get_mut(fd as usize) } {
        Some(s) => s,
        None => return -9, // EBADF
    };

    // Get data from receive buffer
    match socket.dequeue_packet() {
        Some(packet) => {
            let copy_len = packet.data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&packet.data[..copy_len]);
            copy_len as isize
        }
        None => {
            // W3: a recorded ICMP error (connected UDP) beats EAGAIN.
            if socket.pending_error != 0 {
                -(socket.pending_error as isize)
            } else {
                -11 // EAGAIN (no data to read)
            }
        }
    }
}

/// Receive UDP packet and return source address
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
/// - `len`: Buffer length
///
/// # Returns
/// (bytes, source_ip, source_port) on success, error code on failure
pub fn udp_recvfrom(fd: i32, buf: &mut [u8], _len: usize) -> Result<(isize, u32, u16), isize> {
    // R24 (HIGH-6): leaf lock around the dequeue.
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // Get socket
    let socket = match unsafe { UDP_SOCKET_TABLE.get_mut(fd as usize) } {
        Some(s) => s,
        None => return Err(-9), // EBADF
    };

    // Get data from receive buffer
    match socket.dequeue_packet() {
        Some(packet) => {
            let copy_len = packet.data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&packet.data[..copy_len]);
            Ok((copy_len as isize, packet.src_addr, packet.src_port))
        }
        None => {
            // W3: a recorded ICMP error (connected UDP) beats EAGAIN.
            if socket.pending_error != 0 {
                Err(-(socket.pending_error as isize))
            } else {
                Err(-11) // EAGAIN
            }
        }
    }
}

/// Poll: does this UDP socket have a queued datagram? (R24 — completes
/// R22-4, which taught poll about the TCP protocol-table recv_buffer but
/// not UDP's; UDP poll never reported POLLIN.)
pub fn udp_poll_readable(fd: i32) -> bool {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        UDP_SOCKET_TABLE
            .get(fd as usize)
            .map(|s| !s.recv_buffer.is_empty())
            .unwrap_or(false)
    }
}

/// W3: length of the next queued datagram (None = empty). recvmsg uses it
/// to report MSG_TRUNC precisely.
pub fn udp_next_dgram_len(fd: i32) -> Option<usize> {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        UDP_SOCKET_TABLE
            .get(fd as usize)
            .and_then(|s| s.recv_buffer.front())
            .map(|p| p.data.len())
    }
}

/// W3: does this UDP slot carry a pending (ICMP) error? Part of the
/// blocking-recv wake condition — without it an error that lands between
/// the recv attempt and prepare_to_wait is a lost wakeup.
pub fn udp_has_error(fd: i32) -> bool {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        UDP_SOCKET_TABLE
            .get(fd as usize)
            .map(|s| s.pending_error != 0)
            .unwrap_or(true)
    }
}

/// P1 /proc/net/udp(+udp6): one protocol-slot snapshot.
#[derive(Debug, Clone, Copy)]
pub struct UdpSlotInfo {
    pub local_ip: u32,
    pub local_port: u16,
    pub remote_ip: u32,
    pub remote_port: u16,
    pub connected: bool,
    pub is_v6: bool,
    pub local_ip6: crate::net::ipv6::Ipv6Addr,
    pub remote_ip6: crate::net::ipv6::Ipv6Addr,
}

/// P1 /proc/net/udp: snapshot every live UDP slot (bound ones only).
pub fn udp_dump() -> alloc::vec::Vec<UdpSlotInfo> {
    let _g = UDP_TABLE_LOCK.lock_irqsave();
    let mut out = alloc::vec::Vec::new();
    // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
    unsafe {
        let table = &UDP_SOCKET_TABLE;
        for i in 0..table.count {
            if let Some(s) = table.sockets[i].as_ref() {
                if !s.bound {
                    continue;
                }
                out.push(UdpSlotInfo {
                    local_ip: s.local_ip,
                    local_port: s.local_port,
                    remote_ip: s.remote_ip,
                    remote_port: s.remote_port,
                    connected: s.connected,
                    is_v6: s.is_v6,
                    local_ip6: s.local_ip6,
                    remote_ip6: s.remote_ip6,
                });
            }
        }
    }
    out
}

/// Calculate UDP checksum
///
/// # Arguments
/// - `shdr`: Source IP address (network byte order)
/// - `dhdr`: Destination IP address (network byte order)
/// - `uhdr`: UDP header
/// - `data`: Data
///
/// # Returns
/// Checksum (network byte order)
pub fn udp_checksum(shdr: u32, dhdr: u32, uhdr: &UdpHdr, data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Pseudo header (12 bytes). Callers pass network-order values; the
    // halves and the on-wire header fields must be converted to host word
    // values before summing (review NET-H3 — raw memory values summed
    // byte-swapped words, so every inbound checksummed UDP datagram was
    // dropped).
    // Source IP (4 bytes)
    sum += u16::from_be((shdr >> 16) as u16) as u32;
    sum += u16::from_be(shdr as u16) as u32;
    // Destination IP (4 bytes)
    sum += u16::from_be((dhdr >> 16) as u16) as u32;
    sum += u16::from_be(dhdr as u16) as u32;
    // Reserved (1 byte) + Protocol (1 byte) + UDP length (2 bytes)
    sum += 17u32; // UDP protocol number (reserved=0, protocol=17)
    sum += u16::from_be(uhdr.len) as u32;

    // UDP header (wire-byte words)
    sum += u16::from_be(uhdr.source) as u32;
    sum += u16::from_be(uhdr.dest) as u32;
    sum += u16::from_be(uhdr.len) as u32;
    sum += 0; // Checksum field (set to 0 first)

    // Data
    let mut i = 0;
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        sum += word;
        i += 2;
    }

    // Handle last byte (if any)
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    // Handle carry
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // Invert
    !sum as u16
}

/// Build UDP packet
///
/// # Arguments
/// - `skb`: SkBuff
/// - `source`: Source port
/// - `dest`: Destination port
/// - `data`: Data
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn udp_build_packet(
    skb: &mut SkBuff,
    source: UdpPort,
    dest: UdpPort,
    data: &[u8],
) -> Result<(), ()> {
    // Allocate space for UDP header
    let ptr = skb.skb_push(UDP_HLEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // UDP_HLEN bytes; writing fields of repr(C) UdpHdr is well-defined.
    unsafe {
        let udp_hdr = &mut *(ptr as *mut UdpHdr);

        // Source port
        udp_hdr.source = source.to_be();

        // Destination port
        udp_hdr.dest = dest.to_be();

        // Length (UDP header + data)
        udp_hdr.len = ((UDP_HLEN + data.len()) as u16).to_be();

        // Checksum (set to 0 first, calculate later)
        udp_hdr.check = 0;
    }

    // Add data
    skb.skb_put_data(data)?;

    Ok(())
}

/// Parse UDP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing UDP packet)
///
/// # Returns
/// UDP header reference, or None if parsing fails
pub fn udp_parse_packet(skb: &SkBuff) -> Option<&UdpHdr> {
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < UDP_HLEN {
        return None;
    }

    let udp_hdr = UdpHdr::from_bytes(data)?;

    // Validate length
    let len = udp_hdr.len();
    if (len as usize) < UDP_HLEN || (len as usize) != data.len() {
        return None;
    }

    Some(udp_hdr)
}

/// Receive and process UDP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing UDP packet)
/// - `src_ip`: Source IP address (host order)
/// - `dest_ip`: Destination IP address (host order)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn udp_rcv(skb: &SkBuff, src_ip: u32, dest_ip: u32) -> Result<(), ()> {
    // Parse UDP header
    let udp_hdr = udp_parse_packet(skb).ok_or(())?;

    // Verify UDP checksum (per RFC 768: checksum=0 means no checksum)
    if udp_hdr.check() != 0 {
        let data_len = (udp_hdr.len() as usize).saturating_sub(UDP_HLEN);
        let data = if data_len > 0 {
            // SAFETY: skb.data + UDP_HLEN is within the skb's valid data range
            // since udp_parse_packet validated the length.
            unsafe {
                let data_ptr = skb.data.add(UDP_HLEN);
                core::slice::from_raw_parts(data_ptr, data_len)
            }
        } else {
            &[]
        };
        // udp_checksum expects network-order IPs (matching the TX callers);
        // udp_rcv receives host-order values from ip_rcv.
        let computed = udp_checksum(src_ip.to_be(), dest_ip.to_be(), udp_hdr, data);
        if computed != udp_hdr.check() {
            // Checksum mismatch, silently drop packet
            return Ok(());
        }
    }

    let src_port = UdpPort::from_be(udp_hdr.source);
    let dest_port = UdpPort::from_be(udp_hdr.dest);

    // Get UDP data (after header)
    let data_len = (udp_hdr.len() as usize).saturating_sub(UDP_HLEN);
    let data = if data_len > 0 {
        // SAFETY: skb.data + UDP_HLEN is within the skb's valid data range
        // since udp_parse_packet validated the length.
        unsafe {
            let data_ptr = skb.data.add(UDP_HLEN);
            core::slice::from_raw_parts(data_ptr, data_len)
        }
    } else {
        &[]
    };

    // Find socket bound to destination port (and optionally destination IP).
    // A socket with local_ip == 0 (INADDR_ANY) accepts packets to any local IP;
    // a socket with a specific local_ip only accepts packets to that IP.
    // R24 (HIGH-6): serialize table iteration/enqueue against syscalls
    // (alloc/free/bind/send/recv) — the NetRx softirq runs concurrently on
    // the 4-CPU kernel. Leaf lock: nothing below re-enters UDP.
    let mut delivered_fd: Option<i32> = None;
    {
        let _g = UDP_TABLE_LOCK.lock_irqsave();
        // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
        unsafe {
            for i in 0..UDP_SOCKET_TABLE.count {
                if let Some(ref mut socket) = UDP_SOCKET_TABLE.sockets[i] {
                    if socket.bound
                        && socket.local_port == dest_port
                        && (socket.local_ip == 0 || socket.local_ip == dest_ip)
                        // W3: a CONNECTED socket only accepts datagrams from
                        // its peer — anything else keeps scanning (Linux
                        // filters remote (addr,port) for connected UDP).
                        && (!socket.connected
                            || (socket.remote_ip == src_ip && socket.remote_port == src_port))
                    {
                        // Put data into socket's receive buffer
                        let packet = UdpPacket {
                            data: alloc::vec::Vec::from(data),
                            src_addr: src_ip,
                            src_port: src_port,
                            src_addr6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
                        };
                        socket.enqueue_packet(packet);
                        delivered_fd = Some(i as i32);
                        break;
                    }
                }
            }
        }
    }

    if let Some(fd) = delivered_fd {
        // W3: wake blocking recv waiters on this socket (after the lock).
        crate::net::socket::wake_udp_socket(fd);
        return Ok(());
    }

    // No socket found bound to this port. If the datagram was addressed to
    // us (this stack does not forward), answer with ICMP port unreachable
    // (W3 — the old code silently dropped, so peers' send-then-recv-echo
    // patterns hung until their timeout).
    let is_local = dest_ip == crate::net::arp::get_local_ip() || (dest_ip >> 24) == 127;
    if is_local && !is_broadcast_addr(dest_ip) {
        // SAFETY: skb.data holds the received UDP header (8 bytes) —
        // udp_parse_packet validated at least UDP_HLEN bytes.
        let udp_hdr8 = unsafe { core::slice::from_raw_parts(skb.data, UDP_HLEN) };
        crate::net::icmp::icmp_send_port_unreach(dest_ip, src_ip, udp_hdr8);
    }

    Ok(())
}

/// W3: 255.255.255.255 check (never answer port-unreach for broadcasts).
fn is_broadcast_addr(ip: u32) -> bool {
    ip == 0xFFFFFFFF
}

/// W3: ICMP error for a connected UDP socket (UDP counterpart of
/// tcp_v4_err). The embedded original header is from OUR outbound
/// datagram: orig_src_* is local, orig_dst_* is the remote peer.
pub fn udp_v4_err(
    icmp_type: u8,
    icmp_code: u8,
    orig_src_ip: u32,
    orig_src_port: u16,
    orig_dst_ip: u32,
    orig_dst_port: u16,
) {
    let mut wake_fd: Option<i32> = None;
    {
        let _g = UDP_TABLE_LOCK.lock_irqsave();
        // SAFETY: UDP_SOCKET_TABLE is a global accessed under UDP_TABLE_LOCK.
        unsafe {
            'scan: for i in 0..UDP_SOCKET_TABLE.count {
                let socket = match UDP_SOCKET_TABLE.sockets[i].as_mut() {
                    Some(s) => s,
                    None => continue,
                };
                if socket.connected
                    && socket.local_port == orig_src_port
                    && socket.remote_port == orig_dst_port
                    && socket.remote_ip == orig_dst_ip
                    && (socket.local_ip == 0 || socket.local_ip == orig_src_ip)
                {
                    if icmp_type == crate::net::icmp::icmp_type::DEST_UNREACH {
                        match icmp_code {
                            crate::net::icmp::icmp_code::PORT_UNREACH => {
                                socket.pending_error = 111; // ECONNREFUSED
                            }
                            crate::net::icmp::icmp_code::HOST_UNREACH => {
                                socket.pending_error = 113; // EHOSTUNREACH
                            }
                            crate::net::icmp::icmp_code::NET_UNREACH => {
                                socket.pending_error = 101; // ENETUNREACH
                            }
                            _ => {}
                        }
                        wake_fd = Some(i as i32);
                    }
                    break 'scan;
                }
            }
        }
    }
    if let Some(fd) = wake_fd {
        crate::net::socket::wake_udp_socket(fd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_udphdr_size() {
        assert_eq!(core::mem::size_of::<UdpHdr>(), 8);
    }

    #[test]
    fn test_udp_socket() {
        let mut socket = UdpSocket::new();
        assert!(!socket.bound);
        assert!(!socket.connected);

        assert!(socket.bind(8080).is_ok());
        assert!(socket.bound);

        assert!(socket.connect(0x7F000001, 80).is_ok());
        assert!(socket.connected);

        socket.disconnect();
        assert!(!socket.connected);
    }

    #[test]
    fn test_udp_socket_alloc() {
        let fd1 = udp_socket_alloc();
        assert!(fd1.is_ok());
        assert_eq!(fd1.unwrap(), 0);

        let fd2 = udp_socket_alloc();
        assert!(fd2.is_ok());
        assert_eq!(fd2.unwrap(), 1);

        udp_socket_free(fd1.unwrap());
        udp_socket_free(fd2.unwrap());
    }

    #[test]
    fn test_udp_checksum() {
        let shdr = 0xC0A80101;
        let dhdr = 0xC0A80102;
        let data = b"Hello, World!";

        let mut uhdr = UdpHdr::default();
        uhdr.source = 1234u16.to_be();
        uhdr.dest = 80u16.to_be();
        uhdr.len = ((UDP_HLEN + data.len()) as u16).to_be();
        uhdr.check = 0;

        let csum = udp_checksum(shdr, dhdr, &uhdr, data);
        assert!(csum != 0 || csum == 0xFFFF);
    }
}
