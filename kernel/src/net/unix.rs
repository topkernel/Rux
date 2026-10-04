//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AF_UNIX (local IPC) sockets — P0-1 (desktop IPC base).
//!
//! Supported semantics:
//! - SOCK_STREAM (byte stream)
//! - SOCK_SEQPACKET (connection-mode, record boundaries preserved,
//!   SO_TYPE = SOCK_SEQPACKET, MSG_TRUNC on short recvs)
//! - SOCK_DGRAM (connectionless datagrams with per-message boundaries)
//! - bind()/listen()/connect()/accept() against the global name table
//! - abstract names (sun_path[0] == 0): address IS the name, no
//!   filesystem node, visible in /proc/net/unix as "@name"
//! - Linux autobind: an unbound socket that connects() (or a DGRAM that
//!   sends) is bound to a unique abstract name "\0%05x"
//! - socketpair() — a pre-connected pair (two fds, no name-table entry)
//! - SCM_RIGHTS fd passing through sendmsg/recvmsg cmsg data
//! - SCM_CREDENTIALS: kernel-filled {pid,uid,gid} of the sender
//! - blocking send/recv/accept via the W3 wait-queue discipline
//!   (prepare_to_wait → re-check → schedule → finish_wait)
//! - poll: Linux semantics — listeners report only POLLIN (never
//!   POLLHUP); a connected STREAM/SEQPACKET socket whose peer CLOSED
//!   reports POLLHUP plus a readable POLLIN while data remains
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
use core::sync::atomic::{AtomicU64, Ordering};

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
    /// Byte stream (SOCK_STREAM)
    Stream,
    /// Connection-mode record stream (SOCK_SEQPACKET): send requires a
    /// connection like STREAM, but receive preserves record boundaries
    /// like DGRAM.
    Seqpacket,
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
    /// Bound name (display/reporting: the sun_path given at bind for
    /// filesystem sockets; the registry key itself for abstract names).
    pub bound_name: Spinlock<Option<String>>,
    /// Name-table registration key. For abstract sockets this is the
    /// abstract name (== bound_name); for filesystem sockets it is the
    /// socket node's inode identity "#<fs_id>:<ino>" (see fs_reg_key) —
    /// the table key by which connects find this socket. None = unbound.
    reg_name: Spinlock<Option<String>>,
    /// The socket node's inode, pinned for the lifetime of the binding.
    /// The reg key IS the inode identity, so keeping the Arc prevents
    /// (fs_id, ino) from being recycled while this socket is bound —
    /// the same reason Linux's unix socket holds its inode.
    node_inode: Spinlock<Option<alloc::sync::Arc<crate::fs::inode::Inode>>>,
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
    /// This socket's close() ran (poll: peer sees POLLHUP — the Linux
    /// sk_shutdown = SHUTDOWN_MASK condition, distinct from a mere
    /// peer shutdown(SHUT_WR) which only yields EOF-readable).
    dead: Spinlock<bool>,
    /// Our write half was shut (shutdown(SHUT_WR) / SHUT_RDWR)
    shut_wr: Spinlock<bool>,
    /// W3-style wait queue for blocking recv/send/accept
    pub wait_queue: WaitQueueHead,
    /// Stored socket options (shared shape with the AF_INET layer)
    pub options: Spinlock<SocketOptions>,
    /// Creator's credentials — reported to the peer by SO_PEERCRED /
    /// SCM_CREDENTIALS. Snapshot at socket creation (like Linux
    /// sk_peer_cred semantics for the connecting side).
    pub creds: Spinlock<UnixCred>,
    /// Identity of this socket's open file description (File.file_id),
    /// stamped when the fd is installed. Data-arrival points use it to
    /// call epoll_notify_file() so edge-triggered (EPOLLET) watchers see
    /// every enqueue — a snapshot-only ET model would swallow arrivals
    /// that land between a drain and the next epoll_wait (Xorg registers
    /// its client sockets EPOLLET; missing the edge wedges the server).
    /// 0 = no fd installed yet.
    pub file_id: AtomicU64,
}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for UnixSocket {}

impl UnixSocket {
    pub fn new(kind: UnixKind) -> Self {
        Self {
            kind,
            state: Spinlock::new(UnixState::Unconnected),
            bound_name: Spinlock::new(None),
            reg_name: Spinlock::new(None),
            node_inode: Spinlock::new(None),
            peer: Spinlock::new(None),
            dgram_peer: Spinlock::new(None),
            accept_queue: Spinlock::new(VecDeque::new()),
            backlog: Spinlock::new(16),
            recv_queue: Spinlock::new(VecDeque::new()),
            eof: Spinlock::new(false),
            dead: Spinlock::new(false),
            shut_wr: Spinlock::new(false),
            wait_queue: WaitQueueHead::new(),
            options: Spinlock::new(SocketOptions::new()),
            creds: Spinlock::new(current_unix_cred()),
            file_id: AtomicU64::new(0),
        }
    }

    /// SO_PEERCRED: the connected peer's {pid, uid, gid}.
    /// None = not connected (Linux fails getsockopt with ENOTCONN).
    pub fn peer_cred(&self) -> Option<UnixCred> {
        let peer = self.peer_arc()?;
        let c = *peer.creds.lock();
        Some(c)
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
            UnixKind::Stream | UnixKind::Seqpacket => {
                // Linux unix_writable(): SHUTDOWN (either our SHUT_WR or
                // the peer's close setting SHUTDOWN_MASK) silences EPOLLOUT.
                if *self.shut_wr.lock() || self.peer_gone() {
                    return false;
                }
                match self.peer_arc() {
                    Some(peer) => {
                        let cap = self.options.lock().sndbuf as usize;
                        peer.queued_bytes() < cap
                    }
                    // No peer link (listener/unconnected): send() would
                    // return ENOTCONN/EPIPE immediately — "ready".
                    None => true,
                }
            }
            // Datagram send never blocks on this socket's own state (the
            // target's queue is checked at send time; a dead target is an
            // immediate error).
            UnixKind::Dgram => true,
        }
    }

    /// Did the connected peer fully CLOSE (poll POLLHUP condition)?
    ///
    /// Linux sets the surviving socket's sk_shutdown to SHUTDOWN_MASK when
    /// the peer of a SOCK_STREAM/SOCK_SEQPACKET socket is released; that is
    /// exactly the EPOLLHUP condition. A mere peer shutdown(SHUT_WR) is NOT
    /// "gone" — only EOF-readable. The `dead` flag distinguishes the two;
    /// a dead Weak link (fd dropped entirely) counts as gone too.
    fn peer_gone(&self) -> bool {
        match self.peer.lock().as_ref() {
            // No peer link (listener / unconnected): not "gone", just alone.
            None => false,
            Some(w) => match w.upgrade() {
                Some(peer) => *peer.dead.lock(),
                // Freed — the last fd of the peer dropped: gone.
                None => true,
            },
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

/// Snapshot the current task's credentials for SO_PEERCRED/SCM_CREDENTIALS.
fn current_unix_cred() -> UnixCred {
    match crate::sched::current() {
        Some(task) => {
            let c = task.cred();
            UnixCred {
                pid: crate::process::current_pid() as i32,
                uid: c.uid,
                gid: c.gid,
            }
        }
        None => UnixCred { pid: 0, uid: 0, gid: 0 },
    }
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

/// Key-resolution forensics switch (bind/connect table keys). Off in
/// production builds; flip for alias-path debugging.
const UNIX_KEY_DEBUG: bool = false;

/// Truncate a key for a console debug line.
fn key_shown(s: &str) -> &str {
    &s[..s.len().min(64)]
}

/// Resolve a filesystem socket path to its name-table registration key:
/// the socket NODE's inode identity "#<fs_id>:<ino>".
///
/// Linux matches filesystem unix addresses by INODE
/// (unix_find_socket_byinode), never by the literal path bytes — every
/// path that resolves to the same dentry names the same socket: symlink
/// aliases (/var/run -> /run on every distro: dbus-daemon binds the
/// config address /run/dbus/system_bus_socket while GLib's GDBus
/// hardcodes /var/run/dbus/system_bus_socket), relative paths, ".."
/// segments, hardlinks. The old raw-string registry made the GLib
/// connect miss the daemon's entry — connect() returned ECONNREFUSED
/// against a healthy listener, gnome-session aborted ("Failed to connect
/// to system bus: Could not connect: Connection refused"), and the
/// daemon looked "dead" from the outside while it kept serving the whole
/// time (socket file present, no SIGDEATH, /proc/net/unix healthy).
///
/// Returns None when the path does not resolve (no node at that path):
/// callers treat that as "no listener" (ECONNREFUSED) — exactly Linux,
/// where a path with no socket node simply names no socket. Abstract
/// names ('\0'-prefixed) never come through here.
///
/// Must be called OUTSIDE the UNIX_TABLE lock (the walk takes VFS locks).
fn fs_reg_key(raw: &str) -> Option<String> {
    const PATH_LIMIT: usize = 4096;
    if raw.len() > PATH_LIMIT {
        return None;
    }
    let vp = crate::fs::vfs::path_lookup(raw, crate::fs::vfs::LOOKUP_FOLLOW).ok()?;
    let inode = vp.inode?;
    Some(alloc::format!("#{}:{}", inode.fs_id, inode.ino))
}

/// /proc/net/unix snapshot: one line per named socket, Linux layout
/// `Num RefCount Protocol Flags Type St Path` where abstract names are
/// printed with a leading '@' in place of the NUL.
pub fn proc_net_unix_snapshot() -> Vec<u8> {
    let mut out = String::from("Num       RefCount Protocol Flags    Type St Path\n");
    let table = UNIX_TABLE.lock();
    for (_reg_key, sock) in table.iter() {
        // Display the address the socket was bound with (sun_path or
        // @abstract); the registration key is an internal inode identity.
        let name = sock
            .bound_name
            .lock()
            .clone()
            .unwrap_or_else(|| String::from("?"));
        let typ: u16 = match sock.kind {
            UnixKind::Stream => 1,
            UnixKind::Seqpacket => 5,
            UnixKind::Dgram => 2,
        };
        let st: u8 = match *sock.state.lock() {
            UnixState::Unconnected => 1,  // SS_UNCONNECTED
            UnixState::Connecting => 2,   // SS_CONNECTING
            UnixState::Connected => 3,    // SS_CONNECTED
            UnixState::Listening => 7,    // SS_LISTENING (sk_state TCP_LISTEN)
            UnixState::Closed => 4,       // SS_DISCONNECTING
        };
        let path = if let Some(rest) = name.strip_prefix('\0') {
            alloc::format!("@{}", rest)
        } else {
            name.clone()
        };
        out += &alloc::format!(
            "{:016x}: 00000002 00000000 00000000 {:04x} {:02x} {}\n",
            Arc::as_ptr(sock) as usize,
            typ,
            st,
            path
        );
    }
    out.into_bytes()
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
    if sock.bound_name.lock().is_some() || sock.reg_name.lock().is_some() {
        return Err(-22); // EINVAL — already bound
    }
    // Filesystem-path bind: Linux bind(2) creates a S_IFSOCK inode at
    // sun_path (visible to stat/chmod/ls; abstract names skip this).
    // dbus-daemon chmod()s the socket path after bind — without the node
    // the daemon fails its setup. The node is created through the VFS
    // (symlinks in the walk are followed — a bind through /var/run/...
    // lands on /run/...), and the NAME TABLE is keyed by that inode's
    // identity (see fs_reg_key) so every alias path that resolves to the
    // same node connects to this socket — the Linux inode-matching rule.
    let mut key = addr.key.clone();
    if !addr.key.starts_with('\0') {
        let inode = match create_socket_node(&addr.key) {
            Ok(i) => i,
            // EEXIST on the path = EADDRINUSE (stale socket file).
            Err(-17) => return Err(-98),
            Err(e) => return Err(e),
        };
        key = alloc::format!("#{}:{}", inode.fs_id, inode.ino);
        // Pin the inode: the reg key is its identity, and holding the Arc
        // keeps (fs_id, ino) from being recycled while we are bound.
        *sock.node_inode.lock() = Some(inode);
    }
    let mut table = UNIX_TABLE.lock();
    if table.contains_key(&key) {
        drop(table);
        // Name taken with our node freshly created (the holder's node was
        // unlinked while it stayed bound). Undo the stray node so bind
        // does not litter the fs.
        if !addr.key.starts_with('\0') {
            let _ = crate::fs::vfs::vfs_unlink(&addr.key);
            *sock.node_inode.lock() = None;
        }
        return Err(-98); // EADDRINUSE
    }
    if UNIX_KEY_DEBUG {
        crate::pr_info!(
            "unix: BIND raw={:?} key={:?} by pid={}",
            key_shown(&addr.key),
            key_shown(&key),
            crate::process::current_pid()
        );
    }
    // bound_name stays the sun_path the caller gave (display/reporting);
    // the registration key is separate for filesystem sockets.
    *sock.bound_name.lock() = Some(addr.key.clone());
    *sock.reg_name.lock() = Some(key.clone());
    table.insert(key, sock.clone());
    Ok(())
}

/// Filesystem paths are registry keys without the leading NUL marker that
/// abstract addresses carry.
#[allow(dead_code)]
fn is_fs_path(key: &str) -> bool {
    !key.starts_with('\0')
}

/// Create a socket node (S_IFSOCK) at `path`, the way Linux bind(2) does.
/// Same create-and-retype pattern as FIFO mknod: create a regular file,
/// then set its mode word to S_IFSOCK|perm via setattr.
/// Returns the retyped inode — the socket's name-table identity.
fn create_socket_node(
    path: &str,
) -> Result<alloc::sync::Arc<crate::fs::inode::Inode>, i32> {
    // O_WRONLY|O_CREAT|O_EXCL — the transient fd is closed immediately.
    match crate::fs::file_open(path, 0o1 | 0o100 | 0o200 | 0o1000, 0o777) {
        Ok(fd) => {
            // SAFETY: fd is a valid open descriptor from file_open above.
            let inode = unsafe {
                let file = match crate::fs::get_file_fd(fd) {
                    Some(f) => f,
                    None => {
                        crate::fs::close_file_fd(fd);
                        return Err(-5); // EIO
                    }
                };
                match (*file.inode.get()).as_ref() {
                    Some(i) => i.clone(),
                    None => {
                        crate::fs::close_file_fd(fd);
                        return Err(-5); // EIO
                    }
                }
            };
            // Retype to S_IFSOCK (mode bits: 0777 & ~umask was applied at
            // create; keep the permission bits, swap the type field).
            let full_mode =
                crate::fs::inode::InodeMode::S_IFSOCK | (inode.mode.bits() & 0o777);
            let ret = inode.op_setattr(
                crate::fs::inode::setattr_attr::ATTR_MODE,
                full_mode as u64,
                0,
            );
            // SAFETY: fd is a valid open descriptor from file_open above.
            unsafe { crate::fs::close_file_fd(fd); }
            if ret != 0 {
                return Err(ret);
            }
            Ok(inode)
        }
        Err(e) => Err(e),
    }
}

/// listen(): mark a STREAM/SEQPACKET socket as a listener.
pub fn unix_listen(sock: &Arc<UnixSocket>, backlog: i32) -> Result<(), i32> {
    if !matches!(sock.kind, UnixKind::Stream | UnixKind::Seqpacket) {
        return Err(-95); // EOPNOTSUPP
    }
    // Linux autobinds an unbound listener to a unique abstract name.
    if sock.bound_name.lock().is_none() {
        autobind(sock);
    }
    if backlog > 0 {
        *sock.backlog.lock() = backlog as usize;
    }
    *sock.state.lock() = UnixState::Listening;
    Ok(())
}

/// connect(): STREAM/SEQPACKET — hook up with a listener; DGRAM — set the
/// default destination.
///
/// `nonblock` is the connecting fd's O_NONBLOCK — it governs the
/// full-backlog behavior exactly like Linux (see below).
pub fn unix_connect(
    sock: &Arc<UnixSocket>,
    addr: &UnixAddr,
    nonblock: bool,
) -> Result<(), i32> {
    if *sock.shut_wr.lock() {
        return Err(-32); // EPIPE
    }
    match sock.kind {
        UnixKind::Stream | UnixKind::Seqpacket => {
            if *sock.state.lock() == UnixState::Connected {
                return Err(-106); // EISCONN
            }
            // Filesystem paths resolve to the node's inode identity, so
            // alias paths (symlinks: /var/run vs /run) find the listener
            // (Linux matches by inode — see fs_reg_key). An unresolvable
            // path has no socket node: ECONNREFUSED.
            let lookup_key = match fs_reg_key(&addr.key) {
                Some(k) => k,
                None => return Err(-111), // ECONNREFUSED — no node
            };
            let server = match lookup(&lookup_key) {
                Some(s) => s,
                None => {
                    if UNIX_KEY_DEBUG {
                        crate::pr_info!(
                            "unix: connect REFUSED no-entry key={:?} raw={:?} by pid={}",
                            key_shown(&lookup_key),
                            key_shown(&addr.key),
                            crate::process::current_pid()
                        );
                    }
                    return Err(-111); // ECONNREFUSED
                }
            };
            if *server.state.lock() != UnixState::Listening {
                return Err(-111); // ECONNREFUSED
            }
            // Backlog full? Linux unix_stream_connect: a blocking
            // connect WAITS for the listener to accept (unix_wait_for_peer)
            // and a non-blocking one gets EAGAIN — never an instant
            // ECONNREFUSED. Refusing outright made every client of a
            // momentarily-stalled listener (dbus-daemon forking an
            // activation helper under load, so its main loop pauses and
            // the accept queue backs up) see "Connection refused" — GLib
            // treats that as bus death and gnome-session aborts. Once the
            // daemon drains the queue the same connect succeeds, which is
            // why the bus "self-heals" minutes later.
            loop {
                let q_len = server.accept_queue.lock().len();
                let bk = *server.backlog.lock();
                if q_len < bk {
                    break;
                }
                if *server.state.lock() != UnixState::Listening {
                    return Err(-111); // listener died while we waited
                }
                if nonblock {
                    return Err(-11); // EAGAIN
                }
                // Blocking: sleep on the listener's queue until an accept
                // pops a child (room) or the listener closes.
                let s = server.clone();
                unix_wait_round(
                    &s,
                    &|| {
                        s.accept_queue.lock().len() < *s.backlog.lock()
                            || *s.state.lock() != UnixState::Listening
                    },
                    None,
                )?;
            }
            // Linux autobind: an unbound connecting client gets a unique
            // abstract name so the server can address/report it.
            if sock.bound_name.lock().is_none() {
                autobind(sock);
            }
            // Server-side child: connected to us, queued for accept().
            let child = Arc::new(UnixSocket::new(sock.kind));
            *child.state.lock() = UnixState::Connected;
            *child.peer.lock() = Some(Arc::downgrade(sock));
            // The child inherits the server's bound name for
            // getsockname/recvfrom reporting.
            *child.bound_name.lock() = server.bound_name.lock().clone();
            // SO_PEERCRED: the child IS the server endpoint, so clients
            // asking for the peer's creds must see the server's, not the
            // connecting client's (the child was allocated inside the
            // client's connect() syscall context).
            *child.creds.lock() = *server.creds.lock();
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
            // Resolved to the node's inode identity for the same
            // alias-path reasons as STREAM.
            let lookup_key = match fs_reg_key(&addr.key) {
                Some(k) => k,
                None => return Err(-111), // ECONNREFUSED — no node
            };
            if lookup(&lookup_key).is_none() {
                return Err(-111); // ECONNREFUSED
            }
            // Autobind an abstract name if unbound (Linux semantics: a
            // connected DGRAM socket needs a reply address).
            if sock.bound_name.lock().is_none() {
                autobind(sock);
            }
            *sock.dgram_peer.lock() = Some(lookup_key);
            *sock.state.lock() = UnixState::Connected;
            Ok(())
        }
    }
}

/// Autobind an abstract address (Linux format: "\0" + 5 hex digits).
fn autobind(sock: &Arc<UnixSocket>) {
    static ABSTRACT_COUNTER: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(1);
    loop {
        let n = ABSTRACT_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let key = String::from(alloc::format!("\0{:05x}", n));
        let mut table = UNIX_TABLE.lock();
        if !table.contains_key(&key) {
            table.insert(key.clone(), sock.clone());
            // Abstract names ARE their own registration key.
            *sock.bound_name.lock() = Some(key.clone());
            *sock.reg_name.lock() = Some(key);
            return;
        }
    }
}

/// accept(): pop one established child (EAGAIN when none pending).
pub fn unix_accept(sock: &Arc<UnixSocket>) -> Result<Arc<UnixSocket>, i32> {
    if !matches!(sock.kind, UnixKind::Stream | UnixKind::Seqpacket) {
        return Err(-95); // EOPNOTSUPP
    }
    if *sock.state.lock() != UnixState::Listening {
        return Err(-22); // EINVAL
    }
    let child = sock.accept_queue.lock().pop_front().ok_or(-11)?; // EAGAIN
    // An accept freed backlog room — wake blocking connects that are
    // waiting for space on this listener (they re-check and enqueue).
    sock.wait_queue.wake_up_all();
    Ok(child)
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
            UnixKind::Stream | UnixKind::Seqpacket => {
                // Connection-mode: an explicit destination is EISCONN.
                if dest.is_some() {
                    return Err(-106); // EISCONN
                }
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
                // Linux autobinds an unbound DGRAM sender so the receiver
                // has a reply address.
                if sock.bound_name.lock().is_none() {
                    autobind(sock);
                }
                if let Some(a) = dest {
                    // sendto resolves to the node's inode identity — a
                    // symlink alias reaches the same DGRAM target.
                    let key = match fs_reg_key(&a.key) {
                        Some(k) => k,
                        None => return Err(-111), // ECONNREFUSED — no node
                    };
                    match lookup(&key) {
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
                // Readiness rose for the TARGET's file description: re-arm
                // edge-triggered epoll watchers and wake blocked waiters.
                // Every enqueue is a potential fresh edge (Linux re-queues
                // the epi from the socket's data_ready callback on every
                // arrival, not just empty→nonempty transitions).
                let fid = target.file_id.load(Ordering::Acquire);
                if fid != 0 {
                    crate::syscall::misc::epoll_notify_file(fid);
                }
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
        UnixKind::Dgram | UnixKind::Seqpacket => {
            // One record per recv (boundaries preserved); a short buffer
            // truncates and reports MSG_TRUNC.
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
        UnixKind::Stream | UnixKind::Seqpacket => {
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
            // EOF is a readability edge for the peer's watchers too.
            let fid = peer.file_id.load(Ordering::Acquire);
            if fid != 0 {
                crate::syscall::misc::epoll_notify_file(fid);
            }
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
    // Peer's poll() starts reporting POLLHUP (Linux SHUTDOWN_MASK on peer
    // release) — set before the wake below so a racing poll sees it.
    *sock.dead.lock() = true;
    // Remove our name-table entry (the registration key — the inode
    // identity for filesystem sockets, the abstract name otherwise).
    if let Some(name) = sock.reg_name.lock().take() {
        let mut table = UNIX_TABLE.lock();
        // Only remove if the entry still points at US (a replaced entry
        // after a re-bind must survive).
        if table
            .get(&name)
            .map(|s| Arc::ptr_eq(s, sock))
            .unwrap_or(false)
        {
            table.remove(&name);
            // NOTE: the filesystem node created at bind() is deliberately
            // NOT unlinked here — Linux keeps the socket file after the
            // last fd closes (the classic stale-socket-file behavior; a
            // re-bind over it gets EADDRINUSE until it is unlinked).
            // Auto-removing it broke LTP bind04/bind05, whose cleanup
            // unlink(2)s the path AFTER closing the sockets and expects
            // it to still exist.
        }
    }
    // Peer gets drain-then-EOF semantics.
    if let Some(peer) = sock.peer_arc() {
        *peer.eof.lock() = true;
        peer.wait_queue.wake_up_all();
        // Peer close raises the peer's POLLIN|POLLHUP readiness — an edge
        // its EPOLLET watchers must see (Xorg detects dead clients this way).
        let fid = peer.file_id.load(Ordering::Acquire);
        if fid != 0 {
            crate::syscall::misc::epoll_notify_file(fid);
        }
    }
    // Drop the pinned socket-node inode (its identity was our reg key).
    *sock.node_inode.lock() = None;
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
        // Listener with pending connections is readable. A listener NEVER
        // reports POLLHUP (Linux: no sk_pair, no SHUTDOWN_MASK) — GNOME
        // main loops treat an unexpected POLLHUP as fatal.
        if *socket.state.lock() == UnixState::Listening && socket.accept_ready() {
            ready |= POLLIN | POLLRDNORM;
        }
    }
    if events & POLLOUT != 0 {
        if socket.send_ready() {
            ready |= POLLOUT | POLLWRNORM;
        }
    }
    // Peer of a connection-mode socket fully CLOSED (or its fd dropped):
    // POLLHUP — unconditionally, while queued data stays readable through
    // POLLIN (Linux sets SHUTDOWN_MASK at peer release time, not after the
    // queue drains). POLLHUP/POLLERR are reported regardless of `events`.
    if matches!(socket.kind, UnixKind::Stream | UnixKind::Seqpacket) && socket.peer_gone() {
        ready |= POLLHUP;
        if events & POLLIN != 0 {
            ready |= POLLIN | POLLRDNORM; // data/EOF still readable
        }
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
    // Stamp the socket with its file description's identity so data
    // arrival can re-arm edge-triggered epoll watchers (see unix_send).
    socket
        .file_id
        .store(file.file_id, Ordering::Release);

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
        SOCK_STREAM => UnixKind::Stream,
        SOCK_SEQPACKET => UnixKind::Seqpacket,
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
        SOCK_STREAM => UnixKind::Stream,
        SOCK_SEQPACKET => UnixKind::Seqpacket,
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
            UnixKind::Seqpacket => SOCK_SEQPACKET,
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
