//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AF_UNIX (local IPC) sockets — P0-1 (desktop IPC base).
//!
//! Supported semantics:
//! - SOCK_STREAM (byte stream; SOCK_SEQPACKET is served by the same path
//!   and reports SO_TYPE = SOCK_STREAM)
//! - SOCK_DGRAM (connectionless datagrams with per-message boundaries)
//! - bind()/listen()/connect()/accept() against the global name table
//! - socketpair() — a pre-connected pair (two fds, no name-table entry)
//! - SCM_RIGHTS fd passing through sendmsg/recvmsg cmsg data
//! - blocking send/recv/accept via the W3 wait-queue discipline
//!   (prepare_to_wait → re-check → schedule → finish_wait)
//! - poll: POLLIN (readable) / POLLOUT (writable) / POLLHUP (peer gone)
//!
//! Locking model:
//! - UNIX_TABLE guards the namespace (bind/connect/close).
//! - Each socket's recv_queue/accept_queue/state/eof are independent
//!   Spinlocks; a sender only ever takes the TARGET's recv_queue lock
//!   (one lock at a time — no ABBA between two sockets).
//! - Peer links are `Weak` on both sides: the fds (File::private_data
//!   raw Arcs) are the only strong owners, so "peer closed" is simply
//!   "Weak::upgrade fails" and no reference cycle can leak a pair.
//! - Wakeup ownership: receivers wait on their OWN queue for data,
//!   senders wait on the TARGET's queue for room. Both conditions are
//!   re-checked by every wake_up_all on that queue (spurious wakes are
//!   harmless — the wait round returns Ok and the caller re-checks).

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::fs::file::{File, FileFlags, FileOps};
use crate::net::socket::{SocketOptions, SOCK_CLOEXEC_FLAG, SOCK_NONBLOCK_FLAG, SOCK_TYPE_MASK};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

/// Address family
pub const AF_UNIX: i32 = 1;

/// Socket types (values from asm-generic)
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_SEQPACKET: i32 = 5;

/// struct sockaddr_un: sa_family_t + sun_path[108]
pub const UNIX_PATH_MAX: usize = 108;
/// sizeof(struct sockaddr_un)
pub const SOCKADDR_UN_LEN: usize = 110;

/// SOL_SOCKET / SCM_RIGHTS (cmsg)
pub const SOL_SOCKET: i32 = 1;
pub const SCM_RIGHTS: i32 = 1;
/// SOL_SOCKET / SCM_CREDENTIALS (cmsg): sender's {pid, uid, gid} triple.
pub const SCM_CREDENTIALS: i32 = 2;

/// struct ucred — the SCM_CREDENTIALS cmsg payload (12 bytes on LP64).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnixCred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// Socket states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnixState {
    Unconnected,
    Connecting,
    Connected,
    Listening,
    Closed,
}

/// Socket kind (internal)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnixKind {
    /// Byte stream (SOCK_STREAM and SOCK_SEQPACKET)
    Stream,
    /// Datagram (SOCK_DGRAM)
    Dgram,
}

/// One queued receive record.
///
/// STREAM sockets preserve the byte stream across segments (recv drains
/// as many segments as the buffer fits); the per-segment structure only
/// exists so SCM_RIGHTS attachments can be delivered with the FIRST recv
/// that touches the corresponding bytes (Linux association semantics).
/// DGRAM sockets deliver exactly one segment per recv.
pub struct UnixSeg {
    /// Undelivered payload bytes of this segment
    pub data: Vec<u8>,
    /// SCM_RIGHTS payload — consumed by the first recv touching this
    /// segment (delivered as cmsg, then cleared from the remainder)
    pub files: Vec<Arc<File>>,
    /// SCM_CREDENTIALS payload — same first-touch delivery as `files`.
    pub cred: Option<UnixCred>,
    /// Sender's bound name (for recvfrom address reporting)
    pub src: Option<String>,
}

/// AF_UNIX socket
pub struct UnixSocket {
    /// Stream or Dgram
    pub kind: UnixKind,
    /// Socket state
    pub state: Spinlock<UnixState>,
    /// Bound name (registry key; abstract names carry the leading NUL)
    pub bound_name: Spinlock<Option<String>>,
    /// Connected peer (STREAM, and DGRAM socketpairs). Weak both ways: no
    /// Arc cycle, and the peer's close() makes upgrade() fail — that IS
    /// the EOF/EPIPE signal.
    peer: Spinlock<Option<Weak<UnixSocket>>>,
    /// Connected default destination name (DGRAM connected via connect())
    pub dgram_peer: Spinlock<Option<String>>,
    /// Listener accept queue (children created by client connect())
    accept_queue: Spinlock<VecDeque<Arc<UnixSocket>>>,
    /// listen() backlog cap
    backlog: Spinlock<usize>,
    /// Receive queue (segments)
    recv_queue: Spinlock<VecDeque<UnixSeg>>,
    /// Peer performed close()/shutdown — drain then EOF
    eof: Spinlock<bool>,
    /// Our write half was shut (shutdown(SHUT_WR) / SHUT_RDWR)
    shut_wr: Spinlock<bool>,
    /// W3-style wait queue for blocking recv/send/accept
    pub wait_queue: WaitQueueHead,
    /// Stored socket options (shared shape with the AF_INET layer)
    pub options: Spinlock<SocketOptions>,
}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for UnixSocket {}

impl UnixSocket {
    pub fn new(kind: UnixKind) -> Self {
        Self {
            kind,
            state: Spinlock::new(UnixState::Unconnected),
            bound_name: Spinlock::new(None),
            peer: Spinlock::new(None),
            dgram_peer: Spinlock::new(None),
            accept_queue: Spinlock::new(VecDeque::new()),
            backlog: Spinlock::new(16),
            recv_queue: Spinlock::new(VecDeque::new()),
            eof: Spinlock::new(false),
            shut_wr: Spinlock::new(false),
            wait_queue: WaitQueueHead::new(),
            options: Spinlock::new(SocketOptions::new()),
        }
    }

    /// W3: SO_RCVTIMEO as an absolute jiffies deadline (None = infinite).
    pub fn rcvtimeo_deadline(&self) -> Option<u64> {
        timeout_to_deadline(self.options.lock().rcvtimeo_us)
    }

    /// W3: SO_SNDTIMEO as an absolute jiffies deadline (None = infinite).
    pub fn sndtimeo_deadline(&self) -> Option<u64> {
        timeout_to_deadline(self.options.lock().sndtimeo_us)
    }

    /// Bytes currently queued for the receiver.
    fn queued_bytes(&self) -> usize {
        let q = self.recv_queue.lock();
        q.iter().map(|s| s.data.len()).sum()
    }

    /// Resolve the connected peer to a strong Arc (None = peer gone).
    fn peer_arc(&self) -> Option<Arc<UnixSocket>> {
        self.peer.lock().as_ref().and_then(|w| w.upgrade())
    }

    /// Would recv() return data / EOF (anything but EAGAIN)?
    fn recv_ready(&self) -> bool {
        if !self.recv_queue.lock().is_empty() {
            return true;
        }
        // EOF (peer closed) is a readable condition — read returns 0.
        *self.eof.lock() || *self.state.lock() == UnixState::Closed
    }

    /// Can send() accept more bytes now (or fail with a real error)?
    fn send_ready(&self) -> bool {
        match self.kind {
            UnixKind::Stream => match self.peer_arc() {
                Some(peer) => {
                    let cap = self.options.lock().sndbuf as usize;
                    peer.queued_bytes() < cap
                }
                // Peer gone: send() returns EPIPE immediately — "ready".
                None => true,
            },
            // Datagram send never blocks on this socket's own state (the
            // target's queue is checked at send time; a dead target is an
            // immediate error).
            UnixKind::Dgram => true,
        }
    }

    /// Does this listener have a pending connection?
    fn accept_ready(&self) -> bool {
        !self.accept_queue.lock().is_empty()
    }
}

/// Convert a microsecond timeout (0 = infinite) to an absolute jiffies
/// deadline (1 jiffy = 10ms). Mirrors socket.rs.
fn timeout_to_deadline(us: u64) -> Option<u64> {
    if us == 0 {
        return None;
    }
    Some(crate::drivers::timer::get_jiffies() + (us / 10_000).max(1))
}

// ============================================================================
// Global namespace
// ============================================================================

/// Global AF_UNIX name table: bound path (or abstract key) → socket.
static UNIX_TABLE: Spinlock<alloc::collections::BTreeMap<String, Arc<UnixSocket>>> =
    Spinlock::new(alloc::collections::BTreeMap::new());

/// Encode a sun_path byte string as a registry key.
///
/// Filesystem paths become the plain path string; abstract names
/// (sun_path[0] == 0) keep their leading NUL so the two namespaces cannot
/// collide.
fn name_key(sun_path: &[u8]) -> Option<String> {
    if sun_path.is_empty() {
        return None;
    }
    if sun_path[0] == 0 {
        // Abstract namespace: keep the leading NUL, strip trailing NULs
        // (an all-NUL name is the empty abstract name — invalid).
        let mut bytes = sun_path.to_vec();
        while bytes.len() > 1 && bytes.last() == Some(&0) {
            bytes.pop();
        }
        if bytes.len() == 1 {
            return None;
        }
        return String::from_utf8(bytes).ok();
    }
    // Filesystem path: NUL-terminated.
    let len = sun_path
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(sun_path.len());
    if len == 0 {
        return None;
    }
    String::from_utf8(sun_path[..len].to_vec()).ok()
}

/// Look up a socket by registry key.
fn lookup(name: &str) -> Option<Arc<UnixSocket>> {
    UNIX_TABLE.lock().get(name).cloned()
}

// ============================================================================
// sockaddr_un parsing / serialization
// ============================================================================

/// A parsed AF_UNIX address.
#[derive(Clone)]
pub struct UnixAddr {
    /// Registry key (filesystem path, or "\0..." for abstract)
    pub key: String,
}

/// Parse a sockaddr_un from its raw bytes (family little-endian).
/// `bytes` must be at least 2 bytes (the family field).
pub fn parse_sockaddr_un(bytes: &[u8]) -> Option<UnixAddr> {
    if bytes.len() < 2 {
        return None;
    }
    let family = u16::from_le_bytes([bytes[0], bytes[1]]);
    if family != AF_UNIX as u16 {
        return None;
    }
    let path_len = (bytes.len() - 2).min(UNIX_PATH_MAX);
    let key = name_key(&bytes[2..2 + path_len])?;
    Some(UnixAddr { key })
}

/// Write a sockaddr_un { AF_UNIX, path } into user memory.
/// `name` = None writes only the family (unbound socket) with len 2.
///
/// SAFETY: `addr_ptr`/`addrlen_ptr` must be access_ok-validated by the
/// caller for up to SOCKADDR_UN_LEN / 4 bytes.
pub unsafe fn put_sockaddr_un(
    addr_ptr: *mut u8,
    addrlen_ptr: *mut u32,
    name: Option<&str>,
) -> bool {
    use crate::arch::riscv64::uaccess::{copy_to_user, put_user};
    let mut buf = [0u8; SOCKADDR_UN_LEN];
    buf[0] = AF_UNIX as u8;
    buf[1] = 0;
    let total = match name {
        Some(path) => {
            // Abstract names carry the leading NUL in the string itself.
            let bytes = path.as_bytes();
            let n = bytes.len().min(UNIX_PATH_MAX);
            buf[2..2 + n].copy_from_slice(&bytes[..n]);
            // Filesystem-style names get a NUL terminator when they fit.
            let with_nul = if !bytes.is_empty() && bytes[0] != 0 && n < UNIX_PATH_MAX {
                n + 1
            } else {
                n
            };
            2 + with_nul
        }
        None => 2,
    };
    if copy_to_user(addr_ptr, buf.as_ptr(), total) != 0 {
        return false;
    }
    let _ = put_user(addrlen_ptr, total as u32);
    true
}

// ============================================================================
// Blocking wait engine (W3 socket_wait_round discipline)
// ============================================================================

/// One blocking wait round on `wait_sock`'s wait queue, re-checking
/// `ready` after registering (prepare → re-check → sleep → finish).
/// Returns Ok(()) when the caller should re-check its condition,
/// Err(errno) to abort (EINTR / timeout-as-EAGAIN / ENOMEM).
fn unix_wait_round(
    wait_sock: &Arc<UnixSocket>,
    ready: &dyn Fn() -> bool,
    deadline: Option<u64>,
) -> Result<(), i32> {
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return Err(-11), // no task context — behave non-blocking
    };

    wait_sock.wait_queue.prepare_to_wait(current, false, true);

    // Re-check AFTER registering: if the condition was met between our
    // check and prepare_to_wait, the waker found an empty queue.
    if ready() {
        wait_sock.wait_queue.finish_wait(current);
        // R36-B2: take ourselves back off the GRQ — we never slept, but a
        // concurrent wake may have enqueued us.
        crate::sched::dequeue_task(&*current);
        return Ok(());
    }

    if crate::signal::signal_pending() {
        wait_sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Err(-4); // EINTR
    }

    let timer_id = deadline
        .map(|dl| crate::timer::add_timer_wakeup(dl, crate::sched::get_current_pid()))
        .unwrap_or(0);
    if deadline.is_some() && timer_id == 0 {
        // Timer pool exhausted — a timed wait would sleep forever.
        wait_sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Err(-12); // ENOMEM
    }

    // R54: schedule() restores the caller's SIE state; re-arm IRQs so
    // ticks/IPIs reach this CPU across the wait.
    crate::arch::riscv64::cpu::restore_irq(true);
    crate::sched::schedule();

    if timer_id != 0 {
        crate::timer::del_timer(timer_id);
    }
    wait_sock.wait_queue.finish_wait(current);

    if crate::signal::signal_pending() {
        return Err(-4); // EINTR
    }
    if let Some(dl) = deadline {
        if crate::drivers::timer::get_jiffies() >= dl {
            // Linux SO_RCVTIMEO/SO_SNDTIMEO expiry surfaces as EAGAIN.
            return Err(-11);
        }
    }
    Ok(())
}

/// Blocking recv engine: try recv, on would-block return EAGAIN
/// (non-blocking) or wait on the socket's own queue and retry.
pub fn unix_recv_ctl(
    sock: &Arc<UnixSocket>,
    buf: &mut [u8],
    nonblock: bool,
    deadline: Option<u64>,
) -> Result<UnixRecvResult, i32> {
    loop {
        match unix_recv(sock, buf) {
            Ok(r) => return Ok(r),
            Err(e) if e != -11 => return Err(e),
            Err(_) => {}
        }
        if nonblock {
            return Err(-11);
        }
        let s = sock.clone();
        unix_wait_round(&s, &|| sock_recv_ready(&s), deadline)?;
    }
}

/// Would recv() return anything (data/EOF/error)? Shared with the wait
/// rounds and poll.
fn sock_recv_ready(sock: &UnixSocket) -> bool {
    sock.recv_ready()
}

// ============================================================================
// Socket operations
// ============================================================================

/// bind(): register this socket under a name.
pub fn unix_bind(sock: &Arc<UnixSocket>, addr: &UnixAddr) -> Result<(), i32> {
    if sock.bound_name.lock().is_some() {
        return Err(-22); // EINVAL — already bound
    }
    let mut table = UNIX_TABLE.lock();
    if table.contains_key(&addr.key) {
        return Err(-98); // EADDRINUSE
    }
    *sock.bound_name.lock() = Some(addr.key.clone());
    table.insert(addr.key.clone(), sock.clone());    Ok(())
}

/// listen(): mark a STREAM socket as a listener.
pub fn unix_listen(sock: &Arc<UnixSocket>, backlog: i32) -> Result<(), i32> {
    if sock.kind != UnixKind::Stream {
        return Err(-95); // EOPNOTSUPP
    }
    if sock.bound_name.lock().is_none() {
        return Err(-22); // EINVAL — Linux autobinds; we require a name
    }
    if backlog > 0 {
        *sock.backlog.lock() = backlog as usize;
    }
    *sock.state.lock() = UnixState::Listening;
    Ok(())
}

/// connect(): STREAM — hook up with a listener; DGRAM — set the default
/// destination.
pub fn unix_connect(sock: &Arc<UnixSocket>, addr: &UnixAddr) -> Result<(), i32> {
    if *sock.shut_wr.lock() {
        return Err(-32); // EPIPE
    }
    match sock.kind {
        UnixKind::Stream => {
            if *sock.state.lock() == UnixState::Connected {
                return Err(-106); // EISCONN
            }
            let server = lookup(&addr.key).ok_or(-111)?; // ECONNREFUSED
            if *server.state.lock() != UnixState::Listening {
                return Err(-111); // ECONNREFUSED
            }
            // Backlog full?
            if server.accept_queue.lock().len() >= *server.backlog.lock() {
                return Err(-111); // ECONNREFUSED (Linux blocks; simplified)
            }
            // Server-side child: connected to us, queued for accept().
            let child = Arc::new(UnixSocket::new(UnixKind::Stream));
            *child.state.lock() = UnixState::Connected;
            *child.peer.lock() = Some(Arc::downgrade(sock));
            // The child inherits the server's bound name for
            // getsockname/recvfrom reporting.
            *child.bound_name.lock() = server.bound_name.lock().clone();
            // Client side: point at the child. The child's Arc is owned by
            // the server's accept queue until accept() installs it as an fd.
            *sock.peer.lock() = Some(Arc::downgrade(&child));
            *sock.state.lock() = UnixState::Connected;
            server.accept_queue.lock().push_back(child);
            server.wait_queue.wake_up_all();
            Ok(())
        }
        UnixKind::Dgram => {
            // The target must exist (Linux checks this for connect()).
            if lookup(&addr.key).is_none() {
                return Err(-111); // ECONNREFUSED
            }
            // Autobind an abstract name if unbound (Linux semantics: a
            // connected DGRAM socket needs a reply address).
            if sock.bound_name.lock().is_none() {
                autobind(sock);
            }
            *sock.dgram_peer.lock() = Some(addr.key.clone());
            *sock.state.lock() = UnixState::Connected;
            Ok(())
        }
    }
}

/// Autobind an abstract address (counter-based unique name).
fn autobind(sock: &Arc<UnixSocket>) {
    static ABSTRACT_COUNTER: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(1);
    loop {
        let n = ABSTRACT_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let key = String::from(alloc::format!("\0{:x}", n));
        let mut table = UNIX_TABLE.lock();
        if !table.contains_key(&key) {
            table.insert(key.clone(), sock.clone());
            *sock.bound_name.lock() = Some(key);
            return;
        }
    }
}

/// accept(): pop one established child (EAGAIN when none pending).
pub fn unix_accept(sock: &Arc<UnixSocket>) -> Result<Arc<UnixSocket>, i32> {
    if sock.kind != UnixKind::Stream {
        return Err(-95); // EOPNOTSUPP
    }
    if *sock.state.lock() != UnixState::Listening {
        return Err(-22); // EINVAL
    }
    sock.accept_queue.lock().pop_front().ok_or(-11) // EAGAIN
}

/// W3: one wait round for a blocking accept — sleeps on the listener's
/// wait queue (woken by client connect / listener close) until a child is
/// pending, the deadline expires, or a signal arrives.
pub fn unix_accept_wait(sock: &Arc<UnixSocket>, deadline: Option<u64>) -> Result<(), i32> {
    unix_wait_round(sock, &|| sock.accept_ready(), deadline)
}

/// The connected peer's bound name (accept/getpeername reporting).
pub fn unix_peer_bound_name(sock: &Arc<UnixSocket>) -> Option<String> {
    sock.peer_arc().and_then(|p| p.bound_name.lock().clone())
}

/// Can `target` accept a segment of `len` bytes (Linux unix-SO_SNDBUF
/// semantics: at least one segment is always accepted)?
fn target_has_room(target: &Arc<UnixSocket>, len: usize, cap: usize) -> bool {
    let queued = target.queued_bytes();
    queued == 0 || queued + len <= cap
}

/// Send one message (segment). `dest` overrides the connected destination
/// (DGRAM sendto). `files` is the SCM_RIGHTS payload (already resolved
/// from the sender's fd table). `cred` is the SCM_CREDENTIALS triple
/// (kernel-filled from the sending task).
pub fn unix_send(
    sock: &Arc<UnixSocket>,
    data: &[u8],
    files: Vec<Arc<File>>,
    cred: Option<UnixCred>,
    dest: Option<&UnixAddr>,
    nonblock: bool,
    deadline: Option<u64>,
) -> Result<usize, i32> {
    if *sock.shut_wr.lock() {
        return Err(-32); // EPIPE
    }
    let cap = sock.options.lock().sndbuf as usize;

    loop {
        // Resolve the receiving end fresh each round (the peer may have
        // closed while we were blocked).
        let target: Arc<UnixSocket> = match sock.kind {
            UnixKind::Stream => {
                let state = *sock.state.lock();
                if state != UnixState::Connected {
                    return Err(-107); // ENOTCONN
                }
                match sock.peer_arc() {
                    Some(p) => p,
                    None => return Err(-32), // EPIPE — peer closed
                }
            }
            UnixKind::Dgram => {
                if let Some(a) = dest {
                    match lookup(&a.key) {
                        Some(t) => t,
                        None => return Err(-111), // ECONNREFUSED
                    }
                } else if let Some(p) = sock.peer_arc() {
                    // socketpair DGRAM: the peer link is the destination.
                    p
                } else if let Some(key) = sock.dgram_peer.lock().clone() {
                    match lookup(&key) {
                        Some(t) => t,
                        None => return Err(-111), // ECONNREFUSED
                    }
                } else {
                    return Err(-89); // EDESTADDRREQ
                }
            }
        };

        if *target.state.lock() == UnixState::Closed {
            return Err(-32); // EPIPE
        }

        {
            let mut q = target.recv_queue.lock();
            let queued: usize = q.iter().map(|s| s.data.len()).sum();
            if queued == 0 || queued + data.len() <= cap {
                let src = sock.bound_name.lock().clone();
                q.push_back(UnixSeg {
                    data: data.to_vec(),
                    files,
                    cred,
                    src,
                });
                drop(q);
                target.wait_queue.wake_up_all();
                return Ok(data.len());
            }
        }

        if nonblock {
            return Err(-11); // EAGAIN
        }
        // Block on the TARGET's queue (room condition) — the receiver's
        // drain (and the target's close) wakes it.
        let t = target.clone();
        unix_wait_round(&t, &|| target_has_room(&t, data.len(), cap), deadline)?;
    }
}

/// Result of a receive: bytes copied, full length of the record (DGRAM
/// truncation reporting), sender name, and SCM_RIGHTS files /
/// SCM_CREDENTIALS triple.
pub struct UnixRecvResult {
    pub len: usize,
    pub orig_len: usize,
    pub src: Option<String>,
    pub files: Vec<Arc<File>>,
    pub cred: Option<UnixCred>,
    pub truncated: bool,
}

fn unix_recv_eof() -> UnixRecvResult {
    UnixRecvResult {
        len: 0,
        orig_len: 0,
        src: None,
        files: Vec::new(),
        cred: None,
        truncated: false,
    }
}

/// Receive one message (STREAM: drain across segments; DGRAM: one
/// segment). Returns EAGAIN when nothing is available.
pub fn unix_recv(sock: &Arc<UnixSocket>, buf: &mut [u8]) -> Result<UnixRecvResult, i32> {
    let mut q = sock.recv_queue.lock();
    match sock.kind {
        UnixKind::Stream => {
            if let Some(mut seg) = q.pop_front() {
                let mut files = Vec::new();
                let mut cred: Option<UnixCred> = None;
                let mut src: Option<String> = None;
                let mut copied = 0usize;
                let mut orig_total = 0usize;
                // Drain this segment (and any following ones) into buf.
                loop {
                    orig_total += seg.data.len();
                    let take = seg.data.len().min(buf.len() - copied);
                    buf[copied..copied + take].copy_from_slice(&seg.data[..take]);
                    copied += take;
                    files.extend(seg.files.drain(..));
                    if cred.is_none() {
                        cred = seg.cred.take();
                    }
                    if let Some(s) = seg.src.take() {
                        src = Some(s);
                    }
                    if copied >= buf.len() {
                        // Keep the remainder (fds cleared — they were
                        // delivered with this recv, first-touch semantics)
                        // for the next recv.
                        if take < seg.data.len() {
                            seg.data.drain(..take);
                            q.push_front(seg);
                        }
                        break;
                    }
                    match q.pop_front() {
                        Some(next) => seg = next,
                        None => break,
                    }
                }
                drop(q);
                // Room may have freed for senders blocked on our queue.
                sock.wait_queue.wake_up_all();
                return Ok(UnixRecvResult {
                    len: copied,
                    orig_len: orig_total,
                    src,
                    files,
                    cred,
                    truncated: false,
                });
            }
        }
        UnixKind::Dgram => {
            if let Some(seg) = q.pop_front() {
                let orig_len = seg.data.len();
                let take = orig_len.min(buf.len());
                buf[..take].copy_from_slice(&seg.data[..take]);
                let files = seg.files;
                let cred = seg.cred;
                let src = seg.src;
                drop(q);
                sock.wait_queue.wake_up_all();
                return Ok(UnixRecvResult {
                    len: take,
                    orig_len,
                    src,
                    files,
                    cred,
                    truncated: take < orig_len,
                });
            }
        }
    }
    drop(q);

    // Empty queue: EOF or error or block.
    if *sock.eof.lock() || *sock.state.lock() == UnixState::Closed {
        return Ok(unix_recv_eof());
    }
    match sock.kind {
        UnixKind::Stream => {
            let state = *sock.state.lock();
            if state == UnixState::Unconnected || state == UnixState::Listening {
                return Err(-107); // ENOTCONN
            }
        }
        UnixKind::Dgram => {}
    }
    Err(-11) // EAGAIN
}

/// shutdown(): SHUT_RD(0) / SHUT_WR(1) / SHUT_RDWR(2).
pub fn unix_shutdown(sock: &Arc<UnixSocket>, how: i32) -> Result<(), i32> {
    if how < 0 || how > 2 {
        return Err(-22); // EINVAL
    }
    if how >= 1 {
        *sock.shut_wr.lock() = true;
        // Tell the peer its recvs will EOF (after draining).
        if let Some(peer) = sock.peer_arc() {
            *peer.eof.lock() = true;
            peer.wait_queue.wake_up_all();
        }
    }
    if how == 0 || how == 2 {
        // SHUT_RD: subsequent recvs return EOF once the queue drains.
        *sock.eof.lock() = true;
    }
    sock.wait_queue.wake_up_all();
    Ok(())
}

/// close(): release the name, mark the peer's EOF, wake waiters.
pub fn unix_close(sock: &Arc<UnixSocket>) {
    *sock.state.lock() = UnixState::Closed;
    // Remove our name-table entry.
    if let Some(name) = sock.bound_name.lock().take() {
        let mut table = UNIX_TABLE.lock();
        // Only remove if the entry still points at US (a replaced entry
        // after a re-bind must survive).
        if table
            .get(&name)
            .map(|s| Arc::ptr_eq(s, sock))
            .unwrap_or(false)
        {
            table.remove(&name);
        }
    }
    // Peer gets drain-then-EOF semantics.
    if let Some(peer) = sock.peer_arc() {
        *peer.eof.lock() = true;
        peer.wait_queue.wake_up_all();
    }
    // Pending children of a dying listener are dropped with their Arcs;
    // their clients see EPIPE on the next send (peer upgrade fails).
    sock.accept_queue.lock().clear();
    // Wake senders blocked on OUR queue and our own waiters.
    sock.wait_queue.wake_up_all();
}

// ============================================================================
// File operations
// ============================================================================

/// Recover a strong Arc<UnixSocket> from a File's private_data.
/// SAFETY: the caller must have identity-checked the File against
/// UNIX_SOCKET_OPS first (unix_socket_from_fd / unix_file_of do).
unsafe fn unix_of_file(file: &File) -> Option<Arc<UnixSocket>> {
    let ptr = (*file.private_data.get())?;
    let socket_ptr = ptr as *const UnixSocket;
    Arc::increment_strong_count(socket_ptr);
    Some(Arc::from_raw(socket_ptr))
}

fn unix_file_nonblock(file: &File) -> bool {
    (file.flags().bits() & FileFlags::O_NONBLOCK) != 0
}

fn unix_file_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: ops identity was verified — this File was created by
    // unix_socket_install with a UnixSocket Arc in private_data.
    let socket = match unsafe { unix_of_file(file) } {
        Some(s) => s,
        None => return -9,
    };
    let nonblock = unix_file_nonblock(file);
    let deadline = socket.rcvtimeo_deadline();
    match unix_recv_ctl(&socket, buf, nonblock, deadline) {
        Ok(r) => r.len as isize,
        Err(e) => e as isize,
    }
}

fn unix_file_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: ops identity was verified (see unix_file_read).
    let socket = match unsafe { unix_of_file(file) } {
        Some(s) => s,
        None => return -9,
    };
    if *socket.shut_wr.lock() {
        return -32; // EPIPE
    }
    let nonblock = unix_file_nonblock(file);
    let deadline = socket.sndtimeo_deadline();
    match unix_send(&socket, buf, Vec::new(), None, None, nonblock, deadline) {
        Ok(n) => n as isize,
        Err(e) => e as isize,
    }
}

fn unix_file_close(file: &File) -> i32 {
    // SAFETY: private_data was installed from Arc::into_raw(Arc<UnixSocket>)
    // and this is the final close of the description.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    unsafe { *file.private_data.get() = None; }
    // Reconstruct the leaked Arc so close-side effects run and the
    // reference drops here.
    // SAFETY: ptr came from Arc::into_raw in unix_socket_install.
    let socket = unsafe { Arc::from_raw(ptr as *const UnixSocket) };
    unix_close(&socket);
    0
}

fn unix_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    // SAFETY: ops identity was verified (see unix_file_read).
    let socket = match unsafe { unix_of_file(file) } {
        Some(s) => s,
        None => return POLLERR,
    };
    let mut ready = 0u16;

    if events & POLLIN != 0 {
        if socket.recv_ready() {
            ready |= POLLIN | POLLRDNORM;
        }
        // Listener with pending connections is readable.
        if *socket.state.lock() == UnixState::Listening && socket.accept_ready() {
            ready |= POLLIN | POLLRDNORM;
        }
    }
    if events & POLLOUT != 0 {
        if socket.send_ready() {
            ready |= POLLOUT | POLLWRNORM;
        }
    }
    // Peer gone and queue drained: EOF readable + HUP.
    if socket.kind == UnixKind::Stream
        && *socket.eof.lock()
        && socket.recv_queue.lock().is_empty()
    {
        ready |= POLLHUP;
        if events & POLLIN != 0 {
            ready |= POLLIN | POLLRDNORM; // EOF is readable
        }
    }
    if *socket.shut_wr.lock() {
        ready |= POLLERR;
    }
    ready
}

/// AF_UNIX socket file operations.
pub static UNIX_SOCKET_OPS: FileOps = FileOps {
    read: Some(unix_file_read),
    write: Some(unix_file_write),
    lseek: None,
    close: Some(unix_file_close),
    poll: Some(unix_file_poll),
};

// ============================================================================
// Socket creation / fd plumbing
// ============================================================================

/// Create the File + fd for a UnixSocket.
/// `nonblock`/`cloexec` come from the socket() type flags.
fn unix_socket_install(
    socket: &Arc<UnixSocket>,
    nonblock: bool,
    cloexec: bool,
) -> Result<usize, i32> {
    let flags = FileFlags::O_RDWR | if nonblock { FileFlags::O_NONBLOCK } else { 0 };
    let file = Arc::new(File::new(FileFlags::new(flags)));
    file.set_ops(&UNIX_SOCKET_OPS);
    file.set_private_data(Arc::into_raw(socket.clone()) as *mut u8);

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => {
            unwind_unix_file(&file);
            return Err(-9); // EBADF
        }
    };
    let fd = match fdtable.alloc_fd() {
        Some(f) => f,
        None => {
            unwind_unix_file(&file);
            return Err(-24); // EMFILE
        }
    };
    if fdtable.install_fd(fd, file.clone()).is_err() {
        unwind_unix_file(&file);
        return Err(-24);
    }
    if cloexec {
        fdtable.set_fd_cloexec(fd, true);
    }
    Ok(fd)
}

/// Release the raw Arc<UnixSocket> stashed in a failed File.
fn unwind_unix_file(file: &Arc<File>) {
    // SAFETY: the raw pointer came from Arc::into_raw above; we are the
    // sole owner (no ops ran, the File is about to drop).
    let ptr = unsafe { *file.private_data.get() };
    if let Some(ptr) = ptr {
        unsafe { drop(Arc::from_raw(ptr as *const UnixSocket)); }
    }
}

/// socket(AF_UNIX, type, protocol) — entry from sys_socket_create.
pub fn unix_socket_create(type_: i32, protocol: i32) -> Result<usize, i32> {
    const KNOWN_TYPE_FLAGS: i32 = SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG;
    if type_ & !(SOCK_TYPE_MASK | KNOWN_TYPE_FLAGS) != 0 {
        return Err(-22); // EINVAL
    }
    let kind = match type_ & SOCK_TYPE_MASK {
        SOCK_STREAM | SOCK_SEQPACKET => UnixKind::Stream,
        SOCK_DGRAM => UnixKind::Dgram,
        _ => return Err(-94), // ESOCKTNOSUPPORT
    };
    if protocol != 0 {
        return Err(-92); // EPROTONOSUPPORT
    }
    let nonblock = (type_ & SOCK_NONBLOCK_FLAG) != 0;
    let cloexec = (type_ & SOCK_CLOEXEC_FLAG) != 0;
    let socket = Arc::new(UnixSocket::new(kind));
    unix_socket_install(&socket, nonblock, cloexec)
}

/// socketpair(AF_UNIX, type, protocol, sv) — a pre-connected pair.
/// Returns the two fds.
pub fn unix_socketpair(type_: i32) -> Result<(usize, usize), i32> {
    const KNOWN_TYPE_FLAGS: i32 = SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG;
    if type_ & !(SOCK_TYPE_MASK | KNOWN_TYPE_FLAGS) != 0 {
        return Err(-22); // EINVAL
    }
    let kind = match type_ & SOCK_TYPE_MASK {
        SOCK_STREAM | SOCK_SEQPACKET => UnixKind::Stream,
        SOCK_DGRAM => UnixKind::Dgram,
        _ => return Err(-94), // ESOCKTNOSUPPORT
    };
    let nonblock = (type_ & SOCK_NONBLOCK_FLAG) != 0;
    let cloexec = (type_ & SOCK_CLOEXEC_FLAG) != 0;

    let a = Arc::new(UnixSocket::new(kind));
    let b = Arc::new(UnixSocket::new(kind));
    *a.state.lock() = UnixState::Connected;
    *b.state.lock() = UnixState::Connected;
    // Weak links both ways — the two fds are the strong owners, so closing
    // one side drops its Arc and the other side's upgrade() starts failing
    // (EOF/EPIPE) without a reference cycle.
    *a.peer.lock() = Some(Arc::downgrade(&b));
    *b.peer.lock() = Some(Arc::downgrade(&a));
    // DGRAM pairs deliver through the peer link too (unix_send checks the
    // peer before the connected name).

    let fd0 = unix_socket_install(&a, nonblock, cloexec)?;
    match unix_socket_install(&b, nonblock, cloexec) {
        Ok(fd1) => Ok((fd0, fd1)),
        Err(e) => {
            // Unwind the first fd so neither side leaks.
            // SAFETY: close_file_fd operates on the current task's fd
            // table; fd0 was just installed by unix_socket_install.
            let _ = unsafe { crate::fs::file::close_file_fd(fd0) };
            Err(e)
        }
    }
}

/// Wrap an accepted child socket into a new process fd (accept4 path).
/// `flags` carries accept4()'s SOCK_CLOEXEC / SOCK_NONBLOCK.
pub fn unix_install_accepted(sock: &Arc<UnixSocket>, flags: i32) -> Result<usize, i32> {
    let nonblock = (flags & SOCK_NONBLOCK_FLAG) != 0;
    let cloexec = (flags & SOCK_CLOEXEC_FLAG) != 0;
    unix_socket_install(sock, nonblock, cloexec)
}

/// Resolve a process fd to its UnixSocket (identity-checked).
pub fn unix_socket_from_fd(fd: usize) -> Option<Arc<UnixSocket>> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;
    {
        let ops = unsafe { *file.ops.get() };
        match ops {
            Some(ops) if core::ptr::eq(ops, &UNIX_SOCKET_OPS) => {}
            _ => return None,
        }
    }
    // SAFETY: ops identity confirmed; private_data is a leaked
    // Arc<UnixSocket> from Arc::into_raw.
    unsafe { unix_of_file(&file) }
}

/// (UnixSocket, file O_NONBLOCK) for a fd.
pub fn unix_file_of(fd: usize) -> Option<(Arc<UnixSocket>, bool)> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;
    {
        let ops = unsafe { *file.ops.get() };
        match ops {
            Some(ops) if core::ptr::eq(ops, &UNIX_SOCKET_OPS) => {}
            _ => return None,
        }
    }
    let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;
    // SAFETY: ops identity confirmed above.
    let socket = unsafe { unix_of_file(&file) }?;
    Some((socket, nonblock))
}

// ============================================================================
// getsockopt / setsockopt (minimal SOL_SOCKET set)
// ============================================================================

/// unix setsockopt — mirrors the AF_INET SOL_SOCKET handling for the
/// options stored in SocketOptions. Returns 0 or a negative errno.
/// `read_i32` reads an int option value from user memory (None = bad).
/// `read_timeval_us` reads a struct timeval as microseconds.
pub fn unix_setsockopt(
    sock: &Arc<UnixSocket>,
    level: i32,
    optname: i32,
    read_i32: &dyn Fn() -> Option<i32>,
    read_timeval_us: &dyn Fn() -> Option<u64>,
) -> i32 {
    const SO_REUSEADDR: i32 = 2;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_RCVTIMEO: i32 = 20;
    const SO_SNDTIMEO: i32 = 21;
    if level != SOL_SOCKET {
        return -97; // ENOPROTOOPT for unknown levels
    }
    match optname {
        SO_RCVTIMEO | SO_SNDTIMEO => {
            let us = match read_timeval_us() {
                Some(us) => us,
                None => return -22,
            };
            let mut opts = sock.options.lock();
            if optname == SO_RCVTIMEO {
                opts.rcvtimeo_us = us;
            } else {
                opts.sndtimeo_us = us;
            }
            0
        }
        SO_REUSEADDR => {
            let v = match read_i32() {
                Some(v) => v,
                None => return -22,
            };
            sock.options.lock().reuseaddr = v != 0;
            0
        }
        SO_SNDBUF | SO_RCVBUF => {
            let v = match read_i32() {
                Some(v) => v,
                None => return -22,
            };
            let mut opts = sock.options.lock();
            let doubled = (v.saturating_mul(2).max(2048)) as u32;
            if optname == SO_SNDBUF {
                opts.sndbuf = doubled;
            } else {
                opts.rcvbuf = doubled;
            }
            0
        }
        _ => -97, // ENOPROTOOPT (SO_TYPE/SO_ERROR are read-only)
    }
}

/// unix getsockopt — returns the i32 option value or a negative errno.
/// Timeval options report their microseconds part only when `timeval` is
/// set by the caller's writeback logic (kept simple: SO_RCVTIMEO /
/// SO_SNDTIMEO report (sec, usec) through the same i64 path in network.rs).
pub fn unix_getsockopt(sock: &Arc<UnixSocket>, level: i32, optname: i32) -> Result<i32, i32> {
    const SO_TYPE: i32 = 3;
    const SO_ERROR: i32 = 4;
    const SO_REUSEADDR: i32 = 2;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_RCVLOWAT: i32 = 18;
    const SO_RCVTIMEO: i32 = 20;
    const SO_SNDTIMEO: i32 = 21;
    const SO_ACCEPTCONN: i32 = 30;
    const SO_PROTOCOL: i32 = 38;
    const SO_DOMAIN: i32 = 39;
    if level != SOL_SOCKET {
        return Err(-97);
    }
    match optname {
        SO_TYPE => Ok(match sock.kind {
            UnixKind::Stream => SOCK_STREAM,
            UnixKind::Dgram => SOCK_DGRAM,
        }),
        SO_ERROR => Ok(sock.options.lock().error),
        SO_REUSEADDR => Ok(sock.options.lock().reuseaddr as i32),
        SO_SNDBUF => Ok(sock.options.lock().sndbuf as i32),
        SO_RCVBUF => Ok(sock.options.lock().rcvbuf as i32),
        SO_RCVLOWAT => Ok(1),
        SO_RCVTIMEO => Ok(sock.options.lock().rcvtimeo_us as i32),
        SO_SNDTIMEO => Ok(sock.options.lock().sndtimeo_us as i32),
        SO_ACCEPTCONN => Ok((*sock.state.lock() == UnixState::Listening) as i32),
        SO_PROTOCOL => Ok(0),
        SO_DOMAIN => Ok(AF_UNIX),
        _ => Err(-97),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_key() {
        assert_eq!(name_key(b"/tmp/sock\0"), Some(String::from("/tmp/sock")));
        assert_eq!(name_key(b"/tmp/sock"), Some(String::from("/tmp/sock")));
        // Abstract: leading NUL preserved.
        assert_eq!(name_key(b"\0abc\0\0"), Some(String::from("\0abc")));
        assert_eq!(name_key(b""), None);
        assert_eq!(name_key(b"\0"), None);
    }

    #[test]
    fn test_parse_sockaddr_un() {
        let mut buf = [0u8; 110];
        buf[0] = AF_UNIX as u8;
        buf[2..11].copy_from_slice(b"/dev/log\0");
        let addr = parse_sockaddr_un(&buf).unwrap();
        assert_eq!(addr.key, "/dev/log");
    }
}
