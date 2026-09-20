//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Socket Abstraction Layer

use alloc::sync::Arc;
use alloc::collections::VecDeque;
use crate::sync::spinlock::Spinlock;
use core::cell::UnsafeCell;

use crate::fs::file::{File, FileFlags, FileOps, FdTable};

// ============================================================================
// Socket Type Definitions
// ============================================================================

/// Address family
pub const AF_INET: i32 = 2;

/// Socket types
pub const SOCK_STREAM: i32 = 1;  // TCP
pub const SOCK_DGRAM: i32 = 2;   // UDP

/// Protocols
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

// ============================================================================
// Socket Structures
// ============================================================================

/// Socket type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Tcp,
    Udp,
}

/// Socket address
#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

impl SockAddrIn {
    /// Parse from raw bytes
    pub fn from_bytes(data: &[u8]) -> Option<&Self> {
        if data.len() < 16 {
            return None;
        }
        // SAFETY: data is at least 16 bytes (size_of::<SockAddrIn>), and the
        // pointer is valid for that duration since it comes from a slice reference.
        unsafe {
            Some(&*(data.as_ptr() as *const SockAddrIn))
        }
    }

    /// Get port number (host byte order)
    pub fn port(&self) -> u16 {
        u16::from_be(self.sin_port)
    }

    /// Get IP address (host byte order)
    pub fn addr(&self) -> u32 {
        u32::from_be(self.sin_addr)
    }
}

/// Receive buffer packet
#[derive(Clone)]
pub struct RecvPacket {
    /// Data
    pub data: alloc::vec::Vec<u8>,
    /// Source address
    pub src_addr: u32,
    /// Source port
    pub src_port: u16,
}

/// Socket states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// Not connected
    Unconnected,
    /// Connecting
    Connecting,
    /// Connected
    Connected,
    /// Listening
    Listening,
    /// Closing
    Closing,
}

/// Unified Socket structure
pub struct Socket {
    /// Socket type
    pub sock_type: SocketType,
    /// Socket state
    pub state: Spinlock<SocketState>,
    /// Local port
    pub local_port: Spinlock<u16>,
    /// Local IP
    pub local_addr: Spinlock<u32>,
    /// Remote port
    pub remote_port: Spinlock<u16>,
    /// Remote IP
    pub remote_addr: Spinlock<u32>,
    /// Receive buffer
    pub recv_queue: Spinlock<VecDeque<RecvPacket>>,
    /// Whether bound
    pub bound: Spinlock<bool>,
    /// TCP index (for TCP socket table lookup)
    pub tcp_fd: Spinlock<Option<i32>>,
    /// UDP index (for UDP socket table lookup)
    pub udp_fd: Spinlock<Option<i32>>,
    /// Slot index in SOCKET_TABLE (for cleanup on close)
    table_slot: Spinlock<Option<usize>>,
}

// SAFETY: Socket uses Spinlocks for all mutable shared state.
unsafe impl Sync for Socket {}

impl Socket {
    /// Create a new Socket
    pub fn new(sock_type: SocketType) -> Self {
        Self {
            sock_type,
            state: Spinlock::new(SocketState::Unconnected),
            local_port: Spinlock::new(0),
            local_addr: Spinlock::new(0),
            remote_port: Spinlock::new(0),
            remote_addr: Spinlock::new(0),
            recv_queue: Spinlock::new(VecDeque::new()),
            bound: Spinlock::new(false),
            tcp_fd: Spinlock::new(None),
            udp_fd: Spinlock::new(None),
            table_slot: Spinlock::new(None),
        }
    }

    /// Bind to address
    pub fn bind(&self, addr: u32, port: u16) -> Result<(), i32> {
        // R32-N9: propagate the protocol-layer return code — both tcp_bind
        // and udp_bind now return EADDRINUSE on port conflicts and the old
        // code swallowed it, reporting success for a bind that never took.
        match self.sock_type {
            SocketType::Tcp => {
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let tcp_fd = self.tcp_fd.lock().ok_or(-9)?;
                let ret = crate::net::tcp::tcp_bind(tcp_fd, port);
                if ret != 0 {
                    return Err(ret);
                }
            }
            SocketType::Udp => {
                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let udp_fd = self.udp_fd.lock().ok_or(-9)?;
                let ret = crate::net::udp::udp_bind(udp_fd, addr, port);
                if ret != 0 {
                    return Err(ret);
                }
            }
        }

        *self.local_addr.lock() = addr;
        *self.local_port.lock() = port;
        *self.bound.lock() = true;
        Ok(())
    }

    /// Listen for connections
    pub fn listen(&self, backlog: i32) -> Result<(), i32> {
        if self.sock_type != SocketType::Tcp {
            return Err(-95); // EOPNOTSUPP
        }

        let tcp_fd = self.tcp_fd.lock().ok_or(-9)?;
        let ret = crate::net::tcp::tcp_listen(tcp_fd, backlog as u32);
        if ret == 0 {
            *self.state.lock() = SocketState::Listening;
            Ok(())
        } else {
            Err(ret)
        }
    }

    /// Connect to remote address
    pub fn connect(&self, addr: u32, port: u16) -> Result<(), i32> {
        *self.remote_addr.lock() = addr;
        *self.remote_port.lock() = port;

        match self.sock_type {
            SocketType::Tcp => {
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let tcp_fd = self.tcp_fd.lock().ok_or(-9)?;
                *self.state.lock() = SocketState::Connecting;
                let ret = crate::net::tcp::tcp_connect(tcp_fd, addr, port);
                if ret == 0 {
                    *self.state.lock() = SocketState::Connected;
                    Ok(())
                } else {
                    *self.state.lock() = SocketState::Unconnected;
                    Err(ret)
                }
            }
            SocketType::Udp => {
                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let udp_fd = self.udp_fd.lock().ok_or(-9)?;
                // R24 (HIGH-6): go through the locked entry point — the raw
                // udp_socket_get() &mut raced the NetRx softirq's udp_rcv.
                let _ = crate::net::udp::udp_connect(udp_fd, addr, port);
                *self.state.lock() = SocketState::Connected;
                Ok(())
            }
        }
    }

    /// Send data
    pub fn send(&self, buf: &[u8], dest_addr: Option<(u32, u16)>) -> Result<usize, i32> {
        match self.sock_type {
            SocketType::Tcp => {
                let state = *self.state.lock();
                if state != SocketState::Connected {
                    return Err(-32); // EPIPE
                }
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let tcp_fd = self.tcp_fd.lock().ok_or(-9)?;
                // R23-3 (send-only, leaf-scoped): the table lock protects
                // the send_buffer/retrans_queue mutation against the RX
                // softirq's process_ack on the same socket. NOT taken at
                // fn entry — recv's TCP branch re-enters tcp_rcv which
                // already holds it (the R23 gate deadlock).
                //
                // R35 (chain-2 fix): tx_packets used to emit every
                // mss-sized segment INLINE under TCP_TABLE_LOCK (up to 180
                // virtio completion spins — 10M+50M iterations each — for
                // one 256KB write). The deferred TcpTxBatch is reserved
                // HERE, outside the lock (try_reserve: OOM is a clean
                // ENOMEM, not the under-lock alloc_error_handler panic of
                // the R34 wedge class), and emitted after the lock drops.
                //
                // Sizing: tx_packets drains the send buffer up to the
                // usable window, which may hold MORE than this call's
                // accepted prefix (leftovers from earlier sends while the
                // window was closed) — so stage accept + one full window
                // (TCP_MAX_WINDOW = 64KB) of payload. With the sys_write
                // chunking (RW_CHUNK = 64KB) the typical reservation is
                // ~128KB, freed at the end of the syscall.
                let accept = core::cmp::min(buf.len(), crate::net::tcp::TcpSocket::TCP_SEND_MAX_CHUNK);
                let window_slack = crate::net::tcp::TCP_MAX_WINDOW as usize;
                let stage_bytes = accept + window_slack;
                let mut tx = crate::net::tcp::TcpTxBatch::new();
                if !tx.reserve(
                    stage_bytes / crate::net::tcp::TCP_DEFAULT_MSS as usize + 2,
                    stage_bytes,
                ) {
                    return Err(-12); // ENOMEM — lock never taken
                }
                let ret = {
                    let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
                    match crate::net::tcp::tcp_socket_get(tcp_fd) {
                        Some(socket) => match socket.send(buf, &mut tx) {
                            Ok(len) => Ok(len),
                            Err(_) => Err(-5), // EIO
                        },
                        None => Err(-9), // EBADF
                    }
                };
                tx.emit_all();
                ret
            }
            SocketType::Udp => {
                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let udp_fd = self.udp_fd.lock().ok_or(-9)?;

                if let Some((addr, port)) = dest_addr {
                    // Explicit destination: use udp_sendto (review NET-M5 —
                    // both branches used to call udp_send, which requires a
                    // connected socket and always returned ENOTCONN).
                    // R24 (HIGH-6): the raw udp_socket_get() presence probe
                    // raced the table — udp_sendto already returns EBADF.
                    let ret = crate::net::udp::udp_sendto(udp_fd, buf, addr, port);
                    if ret >= 0 {
                        Ok(ret as usize)
                    } else {
                        Err(ret as i32)
                    }
                } else {
                    let ret = crate::net::udp::udp_send(udp_fd, buf);
                    if ret >= 0 {
                        Ok(ret as usize)
                    } else {
                        Err(ret as i32)
                    }
                }
            }
        }
    }

    /// Receive data
    pub fn recv(&self, buf: &mut [u8]) -> Result<(usize, Option<(u32, u16)>), i32> {
        match self.sock_type {
            SocketType::Tcp => {
                let state = *self.state.lock();
                // R32-N28: only an UNCONNECTED socket is ENOTCONN. `Closing`
                // (set by shutdown(SHUT_WR/SHUT_RDWR)) must keep the read
                // half alive — the old `!= Connected` gate made recv() fail
                // with ENOTCONN right after shutdown(SHUT_WR), losing all
                // still-in-flight peer data; the buffered data (or EOF once
                // drained) is delivered by the TcpSocket::recv paths below.
                if state == SocketState::Unconnected {
                    return Err(-107); // ENOTCONN
                }
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let tcp_fd = self.tcp_fd.lock().ok_or(-9)?;

                // R34: irqsave — this plain lock is also taken by
                // enqueue_packet() from the NetRx softirq; a timer IRQ at
                // irq_exit on THIS CPU while a syscall holds the plain lock
                // would make the softirq spin on it forever (the holder is
                // the interrupted syscall below it — same-CPU permanent
                // wedge).
                let mut queue = self.recv_queue.lock_irqsave();
                if let Some(packet) = queue.pop_front() {
                    let len = packet.data.len().min(buf.len());
                    buf[..len].copy_from_slice(&packet.data[..len]);
                    return Ok((len, Some((packet.src_addr, packet.src_port))));
                }

                // R24 (R23-3 completion): recv MUTATES the protocol socket
                // (recv_buffer pop_front + windowed ACK) and was the last
                // unlocked writer — it raced tcp_rcv's enqueue_data push on
                // the same VecDeque. Leaf-scoped like send: nothing below
                // re-enters tcp_rcv (ethernet_poll runs before Socket::recv
                // in every syscall caller; TcpSocket::recv's ACK goes to
                // loopback-queue/virtio-xmit only).
                //
                // R35 (chain-2 fix): the window-update ACK is recorded
                // into a deferred TcpTxBatch reserved outside the lock and
                // emitted after it drops — the ACK used to run the virtio
                // completion spin under TCP_TABLE_LOCK.
                let mut tx = crate::net::tcp::TcpTxBatch::new();
                let _ = tx.reserve(1, 0);
                let result = {
                    let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
                    match crate::net::tcp::tcp_socket_get(tcp_fd) {
                        Some(socket) => match socket.recv(buf, buf.len(), &mut tx) {
                            Ok(len) if len > 0 => {
                                Some(Ok((len, Some((socket.remote_ip, socket.remote_port)))))
                            }
                            // R22-4: zero-length read on a half/RST-closed
                            // connection is EOF — returning EAGAIN here made
                            // read() loops spin forever.
                            Ok(0) => Some(Ok((0, None))),
                            _ => None,
                        },
                        None => None,
                    }
                };
                tx.emit_all();
                if let Some(r) = result {
                    return r;
                }

                Err(-11) // EAGAIN
            }
            SocketType::Udp => {
                // R34: irqsave — pairs with enqueue_packet() from the NetRx
                // softirq (same-CPU plain-lock reentrancy wedge; see the TCP
                // branch comment above).
                let mut queue = self.recv_queue.lock_irqsave();
                if let Some(packet) = queue.pop_front() {
                    let len = packet.data.len().min(buf.len());
                    buf[..len].copy_from_slice(&packet.data[..len]);
                    return Ok((len, Some((packet.src_addr, packet.src_port))));
                }

                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let udp_fd = self.udp_fd.lock().ok_or(-9)?;
                let len = crate::net::udp::udp_recv(udp_fd, buf, buf.len());
                if len > 0 {
                    Ok((len as usize, None))
                } else {
                    Err(-11) // EAGAIN
                }
            }
        }
    }

    /// Accept connection (TCP only)
    pub fn accept(&self) -> Result<Arc<Socket>, i32> {
        if self.sock_type != SocketType::Tcp {
            return Err(-95); // EOPNOTSUPP
        }

        let state = *self.state.lock();
        if state != SocketState::Listening {
            return Err(-22); // EINVAL
        }

        let _tcp_fd = self.tcp_fd.lock().ok_or(-9)?;

        Err(-11) // EAGAIN
    }

    /// Enqueue packet to receive buffer
    pub fn enqueue_packet(&self, packet: RecvPacket) {
        // R34: irqsave — runs from the NetRx softirq; the syscall-side
        // reader (Socket::recv) holds this lock across its empty-check, and
        // a plain lock here would same-CPU deadlock against it at irq_exit.
        self.recv_queue.lock_irqsave().push_back(packet);
    }

    /// Close socket
    pub fn close(&self) -> i32 {
        match self.sock_type {
            SocketType::Tcp => {
                if let Some(tcp_fd) = *self.tcp_fd.lock() {
                    // R24 (R23-3 completion): close() mutates the protocol
                    // state machine (send_fin changes state and pushes the
                    // retrans_queue) — serialize against tcp_rcv/timer_tick
                    // like every other writer. Same shape as tcp_rcv's own
                    // send-under-lock. tcp_socket_free takes the lock itself,
                    // so the actual free happens after we drop ours.
                    //
                    // R35 (chain-2 fix): the FIN is recorded into a deferred
                    // TcpTxBatch reserved outside the lock and emitted after
                    // it drops — send_fin used to run the virtio completion
                    // spin under TCP_TABLE_LOCK.
                    let mut free_now = false;
                    let mut tx = crate::net::tcp::TcpTxBatch::new();
                    let _ = tx.reserve(1, 0);
                    {
                        let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
                        if let Some(socket) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                            socket.close(&mut tx);
                            // Only free immediately if connection is fully closed.
                            if socket.state == crate::net::tcp::TcpState::TCP_CLOSE {
                                // R21-N4: drop our pin; free only when the
                                // timer side already reaped to CLOSE and no
                                // other fd holds a reference.
                                let prev = socket
                                    .user_refs
                                    .swap(0, core::sync::atomic::Ordering::AcqRel);
                                free_now = prev <= 1;
                            } else {
                                // R32-B7: still closing (FIN_WAIT1/LAST_ACK/
                                // ...). NEVER free now — the FIN may be lost
                                // and needs the retransmit machinery, and the
                                // peer needs TIME_WAIT-side time. The timer
                                // tick's orphan paths (FIN_WAIT timeout,
                                // retrans exhaustion, B8 CLOSE_WAIT timeout)
                                // transition the slot to CLOSE, and the tick
                                // sweep frees it once user_refs hits 0.
                                if socket.user_refs.load(core::sync::atomic::Ordering::Acquire)
                                    > 0
                                {
                                    // Unpin the accepted-socket reference
                                    // (never wraps: pinned slots hold >= 1).
                                    socket.user_refs.fetch_sub(
                                        1,
                                        core::sync::atomic::Ordering::AcqRel,
                                    );
                                }
                                // Client/listener slots have no parent_fd;
                                // mark them so the R24 sweep reaps the CLOSE
                                // corpse instead of mistaking it for a fresh
                                // pre-connect slot (which it must keep).
                                socket.orphaned = true;
                            }
                        }
                    }
                    tx.emit_all();
                    if free_now {
                        crate::net::tcp::tcp_socket_free(tcp_fd);
                    }
                }
            }
            SocketType::Udp => {
                if let Some(udp_fd) = *self.udp_fd.lock() {
                    crate::net::udp::udp_socket_free(udp_fd);
                }
            }
        }
        0
    }
}

// ============================================================================
// Socket File Operations
// ============================================================================

fn socket_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data was set during socket creation to a valid Arc<Socket> pointer.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -9, // EBADF
    };
    // SAFETY: ptr is a valid Arc<Socket> pointer set during file creation.
    let socket = unsafe { &*(ptr as *const Socket) };

    match socket.recv(buf) {
        Ok((len, _)) => len as isize,
        Err(e) => e as isize,
    }
}

fn socket_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: private_data was set during socket creation to a valid Arc<Socket> pointer.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -9, // EBADF
    };
    // SAFETY: ptr is a valid Arc<Socket> pointer set during file creation.
    let socket = unsafe { &*(ptr as *const Socket) };

    match socket.send(buf, None) {
        Ok(len) => len as isize,
        Err(e) => e as isize,
    }
}

fn socket_close(file: &File) -> i32 {
    // SAFETY: private_data was set during socket creation from Arc::into_raw.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };

    // Reconstruct the Arc that was leaked via into_raw so it can be properly
    // dropped.  This consumes the raw pointer, so we must clear private_data
    // first to prevent a second reconstruction.
    // SAFETY: ptr was created by Arc::into_raw(Arc::clone(&socket)) in
    // sys_socket_create, so it is a valid Arc with a +1 strong count.
    unsafe { *file.private_data.get() = None; }
    let socket_arc = unsafe { Arc::from_raw(ptr as *const Socket) };

    socket_arc.close();

    // Free from SOCKET_TABLE to drop the table's Arc.
    // SAFETY: table_slot was set during sys_socket_create; SOCKET_TABLE is
    // protected by Spinlock.
    if let Some(idx) = *socket_arc.table_slot.lock() {
        unsafe {
            SOCKET_TABLE.lock().free(idx);
        }
    }

    // socket_arc drops here, releasing the private_data reference.
    0
}

fn socket_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    // SAFETY: private_data was set during socket creation.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr is a valid Arc<Socket> pointer.
    let socket = unsafe { &*(ptr as *const Socket) };

    if events & POLLIN != 0 {
        // R34: irqsave — same recv_queue reentrancy discipline as recv()
        // (enqueue_packet runs from the NetRx softirq).
        let mut readable = !socket.recv_queue.lock_irqsave().is_empty();
        // R22-4: TCP data lands in the protocol table's recv_buffer, not
        // recv_queue — poll never reported readable and clients spun.
        // R32-N6: take the table lock while peeking at the protocol slot —
        // the NetRx softirq and the timer tick mutate recv_buffer/state
        // under it, and this read raced both (leaf-scoped, nothing below
        // re-enters tcp_rcv, same shape as Socket::send).
        if !readable {
            if let Some(tcp_fd) = *socket.tcp_fd.lock() {
                let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
                if let Some(ts) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                    readable = !ts.recv_buffer.is_empty();
                    if !readable
                        && (ts.state == crate::net::tcp::TcpState::TCP_CLOSE_WAIT
                            || ts.state == crate::net::tcp::TcpState::TCP_CLOSE)
                    {
                        // peer finished: readable-as-EOF + HUP
                        ready |= 0x0010 /* POLLHUP */;
                        readable = true;
                    }
                }
            }
        }
        // R24: R22-4's fix never covered UDP — datagrams also land in the
        // protocol table (UdpSocket.recv_buffer), so UDP poll was always
        // not-ready and poll-based readers spun or slept forever.
        if !readable {
            if let Some(udp_fd) = *socket.udp_fd.lock() {
                readable = crate::net::udp::udp_poll_readable(udp_fd);
            }
        }
        if readable {
            ready |= POLLIN | POLLRDNORM;
        }
    }

    if events & POLLOUT != 0 {
        let state = *socket.state.lock();
        if state == SocketState::Connected {
            ready |= POLLOUT | POLLWRNORM;
        }
    }

    ready
}

/// Socket file operations
pub static SOCKET_OPS: FileOps = FileOps {
    read: Some(socket_read),
    write: Some(socket_write),
    lseek: None,
    close: Some(socket_close),
    poll: Some(socket_file_poll),
};

// ============================================================================
// Socket Creation and Management
// ============================================================================

/// Global socket table
struct SocketTable {
    sockets: alloc::vec::Vec<Option<Arc<Socket>>>,
}

impl SocketTable {
    const fn new() -> Self {
        Self {
            sockets: alloc::vec::Vec::new(),
        }
    }

    fn alloc(&mut self, socket: Arc<Socket>) -> Result<usize, ()> {
        for (i, slot) in self.sockets.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(socket);
                return Ok(i);
            }
        }

        let fd = self.sockets.len();
        self.sockets.push(Some(socket));
        Ok(fd)
    }

    fn get(&self, fd: usize) -> Option<Arc<Socket>> {
        self.sockets.get(fd)?.clone()
    }

    fn free(&mut self, fd: usize) {
        if fd < self.sockets.len() {
            self.sockets[fd] = None;
        }
    }
}

static mut SOCKET_TABLE: Spinlock<SocketTable> = Spinlock::new(SocketTable::new());

/// Create socket and return file descriptor
pub fn sys_socket_create(domain: i32, type_: i32, protocol: i32) -> Result<usize, i32> {
    if domain != AF_INET {
        return Err(-97); // EAFNOSUPPORT
    }

    let sock_type = match type_ {
        SOCK_STREAM => {
            if protocol != 0 && protocol != IPPROTO_TCP {
                return Err(-22); // EINVAL
            }
            SocketType::Tcp
        }
        SOCK_DGRAM => {
            if protocol != 0 && protocol != IPPROTO_UDP {
                return Err(-22); // EINVAL
            }
            SocketType::Udp
        }
        _ => return Err(-94), // ESOCKTNOSUPPORT
    };

    let proto_fd = match sock_type {
        SocketType::Tcp => crate::net::tcp::tcp_socket_alloc()?,
        SocketType::Udp => crate::net::udp::udp_socket_alloc()?,
    };

    let socket = Arc::new(Socket::new(sock_type));
    match sock_type {
        SocketType::Tcp => *socket.tcp_fd.lock() = Some(proto_fd),
        SocketType::Udp => *socket.udp_fd.lock() = Some(proto_fd),
    }

    let file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDWR)));
    file.set_ops(&SOCKET_OPS);
    // Clone Arc and convert to raw pointer to keep the Socket alive
    // independently of the socket table entry.
    file.set_private_data(Arc::into_raw(Arc::clone(&socket)) as *mut u8);

    // R14-14 (MED-10): unwind the proto slot and the raw Arc reference on
    // fdtable failures — each error exit used to leak BOTH permanently.
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => {
            unwind_socket_creation(&file, sock_type, proto_fd);
            return Err(-9); // EBADF
        }
    };
    let fd = match fdtable.alloc_fd() {
        Some(f) => f,
        None => {
            unwind_socket_creation(&file, sock_type, proto_fd);
            return Err(-24); // EMFILE
        }
    };
    if fdtable.install_fd(fd, file.clone()).is_err() {
        unwind_socket_creation(&file, sock_type, proto_fd);
        return Err(-24);
    }

    // SAFETY: SOCKET_TABLE is a global protected by Spinlock; we hold the lock.
    let slot = unsafe {
        SOCKET_TABLE.lock().alloc(socket.clone())
    };
    if let Ok(idx) = slot {
        *socket.table_slot.lock() = Some(idx);
    }

    Ok(fd)
}

/// R14-14: unwind a failed socket creation — free the protocol slot and
/// the raw Arc<Socket> reference stashed in the (soon-dropped) File.
fn unwind_socket_creation(file: &Arc<File>, sock_type: SocketType, proto_fd: i32) {
    match sock_type {
        SocketType::Tcp => crate::net::tcp::tcp_socket_free(proto_fd),
        SocketType::Udp => crate::net::udp::udp_socket_free(proto_fd),
    }
    // SAFETY: the Option<*mut u8> in private_data came from Arc::into_raw
    // above; we are the sole owner (the File is about to drop, no ops ran).
    let ptr = unsafe { *file.private_data.get() };
    if let Some(ptr) = ptr {
        unsafe { drop(Arc::from_raw(ptr as *const Socket)); }
    }
}

/// Get socket from file descriptor
pub fn get_socket(fd: usize) -> Option<Arc<Socket>> {
    // SAFETY: SOCKET_TABLE is a global protected by Spinlock; we hold the lock.
    unsafe { SOCKET_TABLE.lock().get(fd) }
}

/// Create a process fd for an accepted TCP connection.
///
/// `tcp_fd` is the protocol-table index returned by tcp_accept(); the
/// connection already lives in that slot — this only wraps it into a
/// Socket + File + process fd (review NET-C4: the old path returned a
/// protocol index to userspace as if it were an fd).
pub fn socket_create_accepted(tcp_fd: i32) -> Result<usize, i32> {
    let socket = Arc::new(Socket::new(SocketType::Tcp));
    *socket.tcp_fd.lock() = Some(tcp_fd);
    // R21-N4: pin the protocol slot against timer-side reaping, and copy
    // the endpoint fields — R24: both under TCP_TABLE_LOCK and revalidated,
    // closing the window where tcp_accept returned an index, the peer then
    // RST'd, and the timer freed the slot (user_refs still 0) before we
    // pinned it — the fd would wrap a freed/reused protocol slot.
    {
        let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
        match crate::net::tcp::tcp_socket_get(tcp_fd) {
            Some(ts) if ts.state != crate::net::tcp::TcpState::TCP_CLOSE => {
                ts.user_refs.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                // Copy the connection's local/remote endpoints for
                // getsockname/peername.
                *socket.local_port.lock() = ts.local_port;
                *socket.local_addr.lock() = ts.local_ip;
                *socket.remote_port.lock() = ts.remote_port;
                *socket.remote_addr.lock() = ts.remote_ip;
            }
            // Connection died (RST / timeout) between accept and pinning —
            // abort instead of wrapping a doomed/freed slot.
            _ => return Err(-103), // ECONNABORTED
        }
    }
    *socket.state.lock() = SocketState::Connected;

    let file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDWR)));
    file.set_ops(&SOCKET_OPS);
    file.set_private_data(Arc::into_raw(Arc::clone(&socket)) as *mut u8);

    // R22-5: unwind on fdtable failures — the raw Arc + the protocol-
    // slot pin (R21-N4) leak on every error exit otherwise.
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => {
            unwind_accepted(&file, tcp_fd);
            return Err(-9);
        }
    };
    let fd = match fdtable.alloc_fd() {
        Some(f) => f,
        None => {
            unwind_accepted(&file, tcp_fd);
            return Err(-24);
        }
    };
    if fdtable.install_fd(fd, file.clone()).is_err() {
        unwind_accepted(&file, tcp_fd);
        return Err(-24);
    }

    // SAFETY: SOCKET_TABLE is a global protected by Spinlock; we hold the lock.
    let slot = unsafe {
        SOCKET_TABLE.lock().alloc(socket.clone())
    };
    if let Ok(idx) = slot {
        *socket.table_slot.lock() = Some(idx);
    }

    Ok(fd)
}

/// R22-5: release the raw Arc<Socket> reference and drop the protocol
/// slot's user pin on a failed accepted-socket fd install.
fn unwind_accepted(file: &Arc<File>, tcp_fd: i32) {
    // R32-N6: serialize against the timer sweep / tcp_rcv (same discipline
    // as Socket::close — leaf-scoped, no RX re-entry below).
    // tcp_socket_free takes the lock itself, so the free happens after the
    // guard drops (same shape as Socket::close).
    let free_now = {
        let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
        match crate::net::tcp::tcp_socket_get(tcp_fd) {
            Some(ts) => {
                let prev = ts.user_refs.swap(0, core::sync::atomic::Ordering::AcqRel);
                prev <= 1
            }
            None => false,
        }
    };
    if free_now {
        crate::net::tcp::tcp_socket_free(tcp_fd);
    }
    let ptr = unsafe { *file.private_data.get() };
    if let Some(ptr) = ptr {
        unsafe { drop(Arc::from_raw(ptr as *const Socket)); }
    }
}

/// Get socket from file descriptor (via File private_data)
pub fn get_socket_from_fd(fd: usize) -> Option<Arc<Socket>> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;

    // SAFETY: private_data was set during socket creation from
    // Arc::as_ptr(&socket). The Arc is owned by SOCKET_TABLE (one strong
    // count). We increment the strong count so the returned Arc is an independent
    // reference.
    let ptr = unsafe { *file.private_data.get() }?;
    let socket_ptr = ptr as *const Socket;
    unsafe {
        Arc::increment_strong_count(socket_ptr);
        Some(Arc::from_raw(socket_ptr))
    }
}

/// Resolve a process fd to its TCP protocol-table index.
/// The syscall layer must NEVER pass a process fd directly to
/// tcp_socket_get()/udp_socket_get() — those index global protocol tables
/// whose slots have no relation to per-process fd numbers (review NET-C3).
pub fn tcp_proto_fd(fd: usize) -> Option<i32> {
    let socket = get_socket_from_fd(fd)?;
    if socket.sock_type != SocketType::Tcp {
        return None;
    }
    let proto = *socket.tcp_fd.lock();
    proto
}

/// Resolve a process fd to its UDP protocol-table index.
pub fn udp_proto_fd(fd: usize) -> Option<i32> {
    let socket = get_socket_from_fd(fd)?;
    if socket.sock_type != SocketType::Udp {
        return None;
    }
    let proto = *socket.udp_fd.lock();
    proto
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sockaddr_in() {
        let addr = SockAddrIn {
            sin_family: 2,
            sin_port: 8080u16.to_be(),
            sin_addr: 0x7F000001u32.to_be(),
            sin_zero: [0; 8],
        };

        assert_eq!(addr.port(), 8080);
        assert_eq!(addr.addr(), 0x7F000001);
    }

    #[test]
    fn test_socket_type() {
        let tcp_socket = Socket::new(SocketType::Tcp);
        assert_eq!(tcp_socket.sock_type, SocketType::Tcp);
        assert_eq!(*tcp_socket.state.lock(), SocketState::Unconnected);

        let udp_socket = Socket::new(SocketType::Udp);
        assert_eq!(udp_socket.sock_type, SocketType::Udp);
    }
}
