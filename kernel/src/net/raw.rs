//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SOCK_RAW (AF_INET) + AF_PACKET sockets — P0-2 (network configuration
//! tooling path).
//!
//! Two socket kinds share this module (both are "raw datagram" sockets
//! with their own file ops, mirroring the netlink module):
//!
//! - AF_INET SOCK_RAW, protocol IPPROTO_ICMP (1): busybox ping's socket.
//!   sendto carries a COMPLETE ICMP packet (checksum by userspace); the
//!   kernel prefixes the IP header and transmits. Inbound ICMP is fanned
//!   out whole (header + payload) with a sockaddr_in source. Protocol
//!   IPPROTO_RAW (255) sockets are accepted as ioctl carriers
//!   (udhcpc's udhcp_read_interface opens one for SIOCGIFADDR et al) but
//!   cannot send.
//! - AF_PACKET SOCK_DGRAM (cooked): busybox udhcpc's DISCOVER-phase
//!   socket. sendto carries a COMPLETE IP packet; the destination MAC
//!   comes from the sockaddr_ll (sll_addr, e.g. ff:ff:ff:ff:ff:ff for
//!   DHCP broadcasts) and the frame goes straight to the wire. Inbound
//!   Ethernet frames matching the bound protocol/ifindex are delivered
//!   as cooked packets (link-layer header stripped) with a sockaddr_ll
//!   source — exactly Linux packet_recvmsg semantics.
//!
//! recv/read returns exactly one packet per call.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::file::{File, FileFlags, FileOps};
use crate::net::buffer::{alloc_skb, kfree_skb, SkBuff};
use crate::net::socket::{SOCK_CLOEXEC_FLAG, SOCK_NONBLOCK_FLAG, SOCK_TYPE_MASK};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

// ============================================================================
// Constants
// ============================================================================

pub const AF_PACKET: i32 = 17;
pub const SOCK_RAW: i32 = 3;
pub const SOCK_DGRAM: i32 = 2;
pub const IPPROTO_ICMP: i32 = 1;
pub const IPPROTO_RAW: i32 = 255;

/// sizeof(struct sockaddr_ll)
const SOCKADDR_LL_LEN: usize = 20;
/// sizeof(struct sockaddr_in)
const SOCKADDR_IN_LEN: usize = 16;

/// sockaddr_ll packet types
const PACKET_HOST: u8 = 0;
const PACKET_BROADCAST: u8 = 1;
const PACKET_MULTICAST: u8 = 2;

// ============================================================================
// Shared socket state
// ============================================================================

/// One received raw/packet datagram: payload + source sockaddr blob
/// (sockaddr_in for raw, sockaddr_ll for packet; already user-layout).
struct RawMsg {
    data: Vec<u8>,
    /// Source sockaddr (family + body), ready to copy to msg_name.
    src_addr: [u8; SOCKADDR_LL_LEN],
    src_len: u8,
}

/// Common raw-socket state (shared by both kinds).
pub struct RawSocket {
    /// AF_INET raw: IPPROTO_ICMP/IPPROTO_RAW; AF_PACKET: wire ethertype
    /// (host order, e.g. 0x0800); 0 = all protocols. Atomic: bind() writes
    /// it while the RX fanout reads it lock-free.
    pub protocol: core::sync::atomic::AtomicI32,
    /// Socket kind switch.
    pub kind: RawKind,
    /// AF_PACKET: bound interface index (0 = any). AF_INET raw: unused.
    pub ifindex: core::sync::atomic::AtomicI32,
    /// Receive queue (one datagram per recv).
    recv_queue: Spinlock<VecDeque<RawMsg>>,
    /// Closed flag (EOF for blocked receivers).
    closed: Spinlock<bool>,
    /// Wait queue for blocking recv.
    pub wait_queue: WaitQueueHead,
    /// Stored socket options (SOL_SOCKET subset).
    pub options: Spinlock<RawSockOptions>,
    /// W3 SO_RCVTIMEO in microseconds (0 = infinite); atomic so the RX
    /// fanout path never takes the options lock.
    pub rcvtimeo_us: core::sync::atomic::AtomicU64,
    /// P2 IP_TTL (0 = system default 64).
    pub ttl: Spinlock<u8>,
}

/// Which raw family this socket belongs to.
#[derive(PartialEq, Clone, Copy)]
pub enum RawKind {
    Inet,
    Packet,
}

/// Options accepted/stored on raw sockets.
pub struct RawSockOptions {
    pub broadcast: bool,
}

impl Default for RawSockOptions {
    fn default() -> Self {
        Self { broadcast: false }
    }
}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for RawSocket {}

impl RawSocket {
    fn new(kind: RawKind, protocol: i32) -> Self {
        Self {
            protocol: core::sync::atomic::AtomicI32::new(protocol),
            kind,
            ifindex: core::sync::atomic::AtomicI32::new(0),
            recv_queue: Spinlock::new(VecDeque::new()),
            closed: Spinlock::new(false),
            wait_queue: WaitQueueHead::new(),
            options: Spinlock::new(RawSockOptions::default()),
            rcvtimeo_us: core::sync::atomic::AtomicU64::new(0),
            ttl: Spinlock::new(0),
        }
    }

    /// W3: SO_RCVTIMEO as an absolute jiffies deadline.
    pub fn rcvtimeo_deadline(&self) -> Option<u64> {
        let us = self.rcvtimeo_us.load(core::sync::atomic::Ordering::Relaxed);
        if us == 0 {
            return None;
        }
        Some(crate::drivers::timer::get_jiffies() + (us / 10_000).max(1))
    }

    fn recv_ready(&self) -> bool {
        !self.recv_queue.lock().is_empty() || *self.closed.lock()
    }

    fn push_msg(&self, msg: RawMsg) {
        self.recv_queue.lock().push_back(msg);
        self.wait_queue.wake_up_all();
    }

    /// Kernel RX fanout: deliver one datagram (payload + sockaddr blob).
    fn deliver(&self, data: &[u8], src_addr: [u8; SOCKADDR_LL_LEN], src_len: u8) {
        // Bound the queue so a socket nobody reads cannot exhaust memory.
        {
            let q = self.recv_queue.lock();
            if q.len() >= 64 {
                return;
            }
        }
        self.push_msg(RawMsg { data: data.to_vec(), src_addr, src_len });
    }
}

/// One blocking wait round (same discipline as netlink.rs).
fn raw_wait_round(sock: &Arc<RawSocket>, deadline: Option<u64>) -> Result<(), i32> {
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return Err(-11),
    };
    sock.wait_queue.prepare_to_wait(current, false, true);

    if sock.recv_ready() {
        sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Ok(());
    }
    if crate::signal::signal_pending() {
        sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Err(-4); // EINTR
    }

    let timer_id = deadline
        .map(|dl| crate::timer::add_timer_wakeup(dl, crate::sched::get_current_pid()))
        .unwrap_or(0);
    if deadline.is_some() && timer_id == 0 {
        sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Err(-12); // ENOMEM
    }

    crate::arch::cpu::restore_irq(true);
    crate::sched::schedule();

    if timer_id != 0 {
        crate::timer::del_timer(timer_id);
    }
    sock.wait_queue.finish_wait(current);

    if crate::signal::signal_pending() {
        return Err(-4);
    }
    if let Some(dl) = deadline {
        if crate::drivers::timer::get_jiffies() >= dl {
            return Err(-11);
        }
    }
    Ok(())
}

// ============================================================================
// Registries (kernel-side fanout targets)
// ============================================================================

static RAW_SOCKETS: Spinlock<Vec<WeakArc>> = Spinlock::new(Vec::new());
static PACKET_SOCKETS: Spinlock<Vec<WeakArc>> = Spinlock::new(Vec::new());

/// Wrapper so one Vec type serves both registries.
struct WeakArc(Weak<RawSocket>);
use alloc::sync::Weak;

/// Drop closed listeners from a registry.
fn gc_listeners(listeners: &mut Vec<WeakArc>) {
    listeners.retain(|w| w.0.strong_count() > 0);
}

// ============================================================================
// Kernel TX/RX entry points
// ============================================================================

/// Deliver an inbound ICMP packet (full header + payload, starting at the
/// ICMP header) to every raw ICMP socket. Called from icmp_rcv.
pub fn icmp_input(skb: &SkBuff, src_ip: u32) {
    // SAFETY: skb.data and skb.len describe a valid byte range.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };
    let mut src = [0u8; SOCKADDR_LL_LEN];
    src[0..2].copy_from_slice(&(2u16).to_le_bytes()); // AF_INET
    src[4..8].copy_from_slice(&src_ip.to_be_bytes());
    let mut listeners = RAW_SOCKETS.lock();
    gc_listeners(&mut listeners);
    for w in listeners.iter() {
        if let Some(sock) = w.0.upgrade() {
            let proto = sock.protocol.load(core::sync::atomic::Ordering::Relaxed);
            if sock.kind == RawKind::Inet && (proto == 0 || proto == IPPROTO_ICMP) {
                sock.deliver(data, src, SOCKADDR_IN_LEN as u8);
            }
        }
    }
}

/// Deliver an inbound Ethernet frame to AF_PACKET sockets.
/// `ifindex` is the ingress device (1 = lo, 2 = eth0 in the presented
/// index space). SOCK_DGRAM cooked semantics: the delivered payload
/// starts at the network-layer header (Ethernet header stripped).
pub fn packet_input(ifindex: u32, skb: &SkBuff) {
    // SAFETY: skb.data and skb.len describe a valid byte range.
    let frame = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };
    if frame.len() < crate::net::ethernet::ETH_HLEN {
        return;
    }
    let hdr = match crate::net::ethernet::EthHdr::from_bytes(frame) {
        Some(h) => h,
        None => return,
    };
    let wire_proto = u16::from_be(hdr.h_proto);
    let payload = &frame[crate::net::ethernet::ETH_HLEN..];
    if payload.is_empty() {
        return;
    }

    let pkttype = if hdr.is_broadcast() {
        PACKET_BROADCAST
    } else if hdr.is_multicast() {
        PACKET_MULTICAST
    } else {
        PACKET_HOST
    };
    // sockaddr_ll { family, protocol(be), ifindex, hatype, pkttype, halen, addr[8] }
    let mut src = [0u8; SOCKADDR_LL_LEN];
    src[0..2].copy_from_slice(&(AF_PACKET as u16).to_le_bytes());
    src[2..4].copy_from_slice(&wire_proto.to_be_bytes());
    src[4..8].copy_from_slice(&(ifindex as i32).to_le_bytes());
    src[8..10].copy_from_slice(&1u16.to_le_bytes()); // ARPHRD_ETHER
    src[10] = pkttype;
    src[11] = crate::net::ethernet::ETH_ALEN as u8;
    src[12..18].copy_from_slice(&hdr.h_source);

    let mut listeners = PACKET_SOCKETS.lock();
    gc_listeners(&mut listeners);
    for w in listeners.iter() {
        if let Some(sock) = w.0.upgrade() {
            if sock.kind != RawKind::Packet {
                continue;
            }
            let proto = sock.protocol.load(core::sync::atomic::Ordering::Relaxed);
            let ifix = sock.ifindex.load(core::sync::atomic::Ordering::Relaxed);
            let proto_ok = proto == 0 || proto == wire_proto as i32;
            let if_ok = ifix == 0 || ifix == ifindex as i32;
            if proto_ok && if_ok {
                sock.deliver(payload, src, SOCKADDR_LL_LEN as u8);
            }
        }
    }
}

/// AF_PACKET sendto: `buf` is a complete network-layer packet (busybox
/// udhcpc builds IP+UDP+DHCP with checksums itself). The Ethernet
/// destination comes from the user sockaddr_ll sll_addr.
pub fn packet_sendto(sock: &Arc<RawSocket>, buf: &[u8], dest_mac: [u8; 6], wire_proto: u16) -> Result<usize, i32> {
    if buf.is_empty() {
        return Ok(0);
    }
    let wire_proto = if wire_proto == 0 {
        sock.protocol.load(core::sync::atomic::Ordering::Relaxed) as u16
    } else {
        wire_proto
    };
    let mut skb = match alloc_skb((buf.len() + crate::net::ethernet::ETH_HLEN) as u32) {
        Some(s) => s,
        None => return Err(-12), // ENOMEM
    };
    if skb.skb_put_data(buf).is_err() {
        kfree_skb(skb);
        return Err(-5);
    }
    let src_mac = crate::net::ethernet::eth_dev_mac();
    let proto = crate::net::buffer::EthProtocol::from_u16(wire_proto)
        .ok_or(-22)?; // EINVAL — only known ethertypes on the wire
    if crate::net::ethernet::eth_push_header(&mut skb, dest_mac, src_mac, proto).is_err() {
        kfree_skb(skb);
        return Err(-5);
    }
    match crate::net::ethernet::transmit_to_device(skb) {
        0 => Ok(buf.len()),
        _ => Err(-5), // EIO
    }
}

/// AF_INET raw ICMP sendto: `buf` is a complete ICMP packet; the kernel
/// adds the IP header and routes via the normal path.
pub fn raw_sendto(sock: &Arc<RawSocket>, buf: &[u8], dest_ip: u32) -> Result<usize, i32> {
    if buf.is_empty() {
        return Ok(0);
    }
    let proto = sock.protocol.load(core::sync::atomic::Ordering::Relaxed);
    if sock.kind != RawKind::Inet || proto != IPPROTO_ICMP {
        // IPPROTO_RAW sockets are ioctl carriers only in this stack.
        return Err(-95); // EOPNOTSUPP
    }
    let mut skb = match alloc_skb(buf.len() as u32) {
        Some(s) => s,
        None => return Err(-12),
    };
    if skb.skb_put_data(buf).is_err() {
        kfree_skb(skb);
        return Err(-5);
    }
    let ttl = *sock.ttl.lock();
    match crate::net::ipv4::ipv4_send_src_ttl(skb, 0, dest_ip, 1, ttl) {
        Ok(()) => Ok(buf.len()),
        Err(()) => Err(-5),
    }
}

// ============================================================================
// Send / recv plumbing (syscall side)
// ============================================================================

/// sendto/send/write: dispatch on the socket kind. For AF_PACKET the
/// destination sockaddr_ll supplies the MAC; AF_INET uses the sockaddr_in.
pub fn raw_send(sock: &Arc<RawSocket>, buf: &[u8], dest: Option<RawDest>) -> Result<usize, i32> {
    match sock.kind {
        RawKind::Inet => {
            let ip = match dest {
                Some(RawDest::Inet(ip)) => ip,
                _ => return Err(-22), // EINVAL — no destination
            };
            raw_sendto(sock, buf, ip)
        }
        RawKind::Packet => {
            let (mac, proto) = match dest {
                Some(RawDest::Packet { mac, proto }) => (mac, proto),
                _ => return Err(-22),
            };
            let fallback = sock.protocol.load(core::sync::atomic::Ordering::Relaxed) as u16;
            packet_sendto(sock, buf, mac, proto.unwrap_or(fallback))
        }
    }
}

/// Parsed sendto destination.
pub enum RawDest {
    /// AF_INET: destination IPv4 (host order).
    Inet(u32),
    /// AF_PACKET: link-layer destination MAC + ethertype override.
    Packet { mac: [u8; 6], proto: Option<u16> },
}

/// recvmsg/recvfrom/read: pop one datagram, optionally filling `src_out`
/// (user sockaddr copy is done by the syscall layer) — returns
/// (bytes, RawMsg source) so callers can build sockaddr_in/sockaddr_ll.
pub fn raw_recv(
    sock: &Arc<RawSocket>,
    buf: &mut [u8],
    nonblock: bool,
    deadline: Option<u64>,
) -> Result<(usize, Option<([u8; SOCKADDR_LL_LEN], u8)>), i32> {
    loop {
        if let Some(msg) = sock.recv_queue.lock().pop_front() {
            let n = msg.data.len().min(buf.len());
            buf[..n].copy_from_slice(&msg.data[..n]);
            let src = Some((msg.src_addr, msg.src_len));
            return Ok((n, src));
        }
        if *sock.closed.lock() {
            return Ok((0, None)); // EOF
        }
        if nonblock {
            return Err(-11); // EAGAIN
        }
        raw_wait_round(sock, deadline)?;
    }
}

/// close(): mark EOF and wake blocked receivers.
pub fn raw_close(sock: &Arc<RawSocket>) {
    *sock.closed.lock() = true;
    sock.wait_queue.wake_up_all();
}

/// bind(): AF_PACKET stores ifindex/protocol from sockaddr_ll; AF_INET
/// raw accepts and ignores (no local raw bind model).
pub fn raw_bind(sock: &Arc<RawSocket>, ifindex: i32, protocol: i32) {
    if sock.kind == RawKind::Packet {
        sock.ifindex.store(ifindex, core::sync::atomic::Ordering::Relaxed);
        if protocol != 0 {
            sock.protocol.store(protocol, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

// ============================================================================
// File operations
// ============================================================================

/// Recover a strong Arc<RawSocket> from a File's private_data.
/// SAFETY: the caller must have identity-checked the File against
/// RAW_OPS first.
unsafe fn raw_of_file(file: &File) -> Option<Arc<RawSocket>> {
    let ptr = (*file.private_data.get())?;
    let socket_ptr = ptr as *const RawSocket;
    Arc::increment_strong_count(socket_ptr);
    Some(Arc::from_raw(socket_ptr))
}

fn raw_file_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: ops identity was verified — created by raw_socket_install.
    let socket = match unsafe { raw_of_file(file) } {
        Some(s) => s,
        None => return -9,
    };
    let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;
    let deadline = socket.rcvtimeo_deadline();
    match raw_recv(&socket, buf, nonblock, deadline) {
        Ok((n, _)) => n as isize,
        Err(e) => e as isize,
    }
}

fn raw_file_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: ops identity was verified (see raw_file_read).
    let socket = match unsafe { raw_of_file(file) } {
        Some(s) => s,
        None => return -9,
    };
    // write() has no destination: AF_PACKET broadcasts to the bound
    // protocol, AF_INET raw refuses.
    match socket.kind {
        RawKind::Packet => {
            let proto = socket.protocol.load(core::sync::atomic::Ordering::Relaxed) as u16;
            match packet_sendto(&socket, buf, crate::net::ethernet::ETH_BROADCAST, proto) {
                Ok(n) => n as isize,
                Err(e) => e as isize,
            }
        }
        RawKind::Inet => -22, // EINVAL — needs sendto with a destination
    }
}

fn raw_file_close(file: &File) -> i32 {
    // SAFETY: private_data came from Arc::into_raw(Arc<RawSocket>).
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    unsafe { *file.private_data.get() = None; }
    // SAFETY: reconstruct the leaked Arc so close runs and the ref drops.
    let socket = unsafe { Arc::from_raw(ptr as *const RawSocket) };
    raw_close(&socket);
    0
}

fn raw_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    // SAFETY: ops identity was verified (see raw_file_read).
    let socket = match unsafe { raw_of_file(file) } {
        Some(s) => s,
        None => return POLLERR,
    };
    let mut ready = 0u16;
    if events & POLLIN != 0 && socket.recv_ready() {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 {
        ready |= POLLOUT | POLLWRNORM;
    }
    ready
}

/// Shared file ops for both raw socket kinds.
pub static RAW_OPS: FileOps = FileOps {
    read: Some(raw_file_read),
    write: Some(raw_file_write),
    lseek: None,
    close: Some(raw_file_close),
    poll: Some(raw_file_poll),
};

// ============================================================================
// Socket creation / fd plumbing
// ============================================================================

fn raw_socket_install(socket: &Arc<RawSocket>, nonblock: bool, cloexec: bool) -> Result<usize, i32> {
    let flags = FileFlags::O_RDWR | if nonblock { FileFlags::O_NONBLOCK } else { 0 };
    let file = Arc::new(File::new(FileFlags::new(flags)));
    file.set_ops(&RAW_OPS);
    file.set_private_data(Arc::into_raw(socket.clone()) as *mut u8);

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => {
            unwind_raw_file(&file);
            return Err(-9);
        }
    };
    let fd = match fdtable.alloc_fd() {
        Some(f) => f,
        None => {
            unwind_raw_file(&file);
            return Err(-24);
        }
    };
    if fdtable.install_fd(fd, file.clone()).is_err() {
        unwind_raw_file(&file);
        return Err(-24);
    }
    if cloexec {
        fdtable.set_fd_cloexec(fd, true);
    }
    Ok(fd)
}

fn unwind_raw_file(file: &Arc<File>) {
    // SAFETY: the raw pointer came from Arc::into_raw above; sole owner.
    let ptr = unsafe { *file.private_data.get() };
    if let Some(ptr) = ptr {
        unsafe { drop(Arc::from_raw(ptr as *const RawSocket)); }
    }
}

/// socket(AF_PACKET, SOCK_DGRAM, htons(ethertype)) — cooked packet socket.
pub fn packet_socket_create(type_: i32, protocol: i32) -> Result<usize, i32> {
    const KNOWN_TYPE_FLAGS: i32 = SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG;
    if type_ & !(SOCK_TYPE_MASK | KNOWN_TYPE_FLAGS) != 0 {
        return Err(-22);
    }
    if type_ & SOCK_TYPE_MASK != SOCK_DGRAM {
        return Err(-94); // ESOCKTNOSUPPORT — SOCK_RAW packets later
    }
    let nonblock = (type_ & SOCK_NONBLOCK_FLAG) != 0;
    let cloexec = (type_ & SOCK_CLOEXEC_FLAG) != 0;
    // The protocol argument is network-byte-order on the wire
    // (htons(ETH_P_IP) arrives as 0x0008); normalize to host order 0x0800.
    let host_proto = if protocol == 0 {
        0
    } else {
        (protocol as u16).swap_bytes() as i32 // network -> host order
    };
    let socket = Arc::new(RawSocket::new(RawKind::Packet, host_proto));
    let fd = raw_socket_install(&socket, nonblock, cloexec)?;
    PACKET_SOCKETS.lock().push(WeakArc(Arc::downgrade(&socket)));
    Ok(fd)
}

/// socket(AF_INET, SOCK_RAW, protocol) — ICMP ping socket (1) or the
/// IPPROTO_RAW (255) ioctl carrier udhcpc opens for SIOCGIFADDR.
pub fn raw_socket_create(type_: i32, protocol: i32) -> Result<usize, i32> {
    const KNOWN_TYPE_FLAGS: i32 = SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG;
    if type_ & !(SOCK_TYPE_MASK | KNOWN_TYPE_FLAGS) != 0 {
        return Err(-22);
    }
    if type_ & SOCK_TYPE_MASK != SOCK_RAW {
        return Err(-94);
    }
    // Linux inet_create's protocol lookup never binds protocol 0
    // (IPPROTO_IP) to the SOCK_RAW protosw: the wild-case falls through
    // the list and returns EPROTONOSUPPORT even for CAP_NET_RAW holders
    // (LTP socket01 "raw open as non-root" runs as root and still expects
    // EPROTONOSUPPORT for socket(PF_INET, SOCK_RAW, 0)).
    if protocol != IPPROTO_ICMP && protocol != IPPROTO_RAW {
        return Err(-93); // EPROTONOSUPPORT
    }
    let nonblock = (type_ & SOCK_NONBLOCK_FLAG) != 0;
    let cloexec = (type_ & SOCK_CLOEXEC_FLAG) != 0;
    let socket = Arc::new(RawSocket::new(RawKind::Inet, protocol));
    let fd = raw_socket_install(&socket, nonblock, cloexec)?;
    RAW_SOCKETS.lock().push(WeakArc(Arc::downgrade(&socket)));
    Ok(fd)
}

/// Resolve a process fd to its RawSocket (identity-checked).
pub fn raw_socket_from_fd(fd: usize) -> Option<Arc<RawSocket>> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;
    {
        let ops = unsafe { *file.ops.get() };
        match ops {
            Some(ops) if core::ptr::eq(ops, &RAW_OPS) => {}
            _ => return None,
        }
    }
    // SAFETY: ops identity confirmed; private_data is a leaked Arc.
    unsafe { raw_of_file(&file) }
}

/// (RawSocket, file O_NONBLOCK) for a fd.
pub fn raw_file_of(fd: usize) -> Option<(Arc<RawSocket>, bool)> {
    let socket = raw_socket_from_fd(fd)?;
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;
    let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;
    Some((socket, nonblock))
}

// ============================================================================
// User sockaddr helpers
// ============================================================================

/// Write the bound sockaddr (sockaddr_ll for AF_PACKET, sockaddr_in for
/// raw) to user (getsockname).
/// SAFETY: addr_ptr/addrlen_ptr access_ok-validated for 20/4 bytes.
pub unsafe fn put_sockaddr_raw(
    sock: &Arc<RawSocket>,
    addr_ptr: *mut u8,
    addrlen_ptr: *mut u32,
) {
    use crate::arch::uaccess::{copy_to_user, put_user};
    match sock.kind {
        RawKind::Packet => {
            let proto = sock.protocol.load(core::sync::atomic::Ordering::Relaxed);
            let ifix = sock.ifindex.load(core::sync::atomic::Ordering::Relaxed);
            let mut buf = [0u8; SOCKADDR_LL_LEN];
            buf[0..2].copy_from_slice(&(AF_PACKET as u16).to_le_bytes());
            buf[2..4].copy_from_slice(&((proto as u16).to_be_bytes()));
            buf[4..8].copy_from_slice(&ifix.to_le_bytes());
            buf[11] = 0; // unbound address
            let _ = copy_to_user(addr_ptr, buf.as_ptr(), SOCKADDR_LL_LEN);
            let _ = put_user(addrlen_ptr, SOCKADDR_LL_LEN as u32);
        }
        RawKind::Inet => {
            let mut buf = [0u8; SOCKADDR_IN_LEN];
            buf[0..2].copy_from_slice(&2u16.to_le_bytes()); // AF_INET
            let _ = copy_to_user(addr_ptr, buf.as_ptr(), SOCKADDR_IN_LEN);
            let _ = put_user(addrlen_ptr, SOCKADDR_IN_LEN as u32);
        }
    }
}

/// Copy a received source sockaddr to the user msg_name buffer, honoring
/// the user's namelen. SAFETY: addr_ptr access_ok-validated for
/// user_namelen bytes.
pub unsafe fn copy_src_to_user(
    src: ([u8; SOCKADDR_LL_LEN], u8),
    addr_ptr: *mut u8,
    user_namelen: u32,
    addrlen_ptr: *mut u32,
) {
    use crate::arch::uaccess::copy_to_user;
    let (addr, len) = src;
    let copy = (len as usize).min(user_namelen as usize);
    if copy > 0 {
        let _ = copy_to_user(addr_ptr, addr.as_ptr(), copy);
    }
    if !addrlen_ptr.is_null() {
        let _ = crate::arch::uaccess::put_user(addrlen_ptr, len as u32);
    }
}
