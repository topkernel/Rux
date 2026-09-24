//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP Protocol

use crate::net::buffer::SkBuff;
use crate::net::ipv4::{route, checksum};
use core::sync::atomic::{AtomicU32, Ordering};
pub use crate::config::TCP_SOCKET_TABLE_SIZE;

/// Global counter for ISN generation to prevent sequence prediction.
static ISN_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Generate an Initial Sequence Number from connection 4-tuple + monotonic inputs.
fn generate_isn(src_ip: u32, src_port: u16, dst_ip: u32, dst_port: u16) -> TcpSeq {
    let base = ISN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let hash = (src_ip.wrapping_mul(31)
        ^ dst_ip.wrapping_mul(37)
        ^ (src_port as u32).wrapping_mul(41)
        ^ (dst_port as u32).wrapping_mul(43))
        .wrapping_add(crate::drivers::timer::get_jiffies() as u32);
    TcpSeq::from_be(hash.wrapping_add(base))
}

/// P1 IPv6: ISN over the 128-bit 4-tuple (FNV-style fold, same shape).
fn generate_isn6(
    src6: &crate::net::ipv6::Ipv6Addr,
    src_port: u16,
    dst6: &crate::net::ipv6::Ipv6Addr,
    dst_port: u16,
) -> TcpSeq {
    let base = ISN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hash: u32 = 0x811c9dc5;
    for &b in src6.iter().chain(dst6.iter()) {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash ^= (src_port as u32).wrapping_mul(41);
    hash ^= (dst_port as u32).wrapping_mul(43);
    hash = hash.wrapping_add(crate::drivers::timer::get_jiffies() as u32);
    TcpSeq::from_be(hash.wrapping_add(base))
}

/// TCP header lengths
pub const TCP_MIN_HLEN: usize = 20;
pub const TCP_MAX_HLEN: usize = 60;

/// TCP maximum window size
pub const TCP_MAX_WINDOW: u16 = 65535;

/// TCP default MSS
pub const TCP_DEFAULT_MSS: u16 = 1460;

/// TCP timer constants - from config
pub const TCP_RTO_MIN_US: u64 = crate::config::TCP_RTO_MIN_US;
pub const TCP_RTO_MAX_US: u64 = crate::config::TCP_RTO_MAX_US;
pub const TCP_RTO_DEFAULT_US: u64 = crate::config::TCP_RTO_DEFAULT_US;
pub const TCP_MAX_RETRIES: u32 = crate::config::TCP_MAX_RETRIES;
pub const TCP_DELACK_TIMEOUT_US: u64 = crate::config::TCP_DELACK_TIMEOUT_US;

/// W3: zero-window persist probe interval (jiffies; 1 jiffy = 10ms).
/// 5s like Linux's initial persist interval (kept fixed — no backoff —
/// for the minimal implementation).
pub const TCP_PERSIST_INTERVAL_JIFFIES: u64 = 500;

/// TCP port number
pub type TcpPort = u16;

/// P1 IPv6: family-aware segment recording on copied endpoints (used by
/// TcpSocket::tx_record and by the retransmit path, which holds a &mut into
/// self.retrans_queue and cannot borrow self again).
#[allow(clippy::too_many_arguments)]
fn tx_record_endpoints(
    is_v6: bool,
    local_ip: u32,
    remote_ip: u32,
    local_ip6: crate::net::ipv6::Ipv6Addr,
    remote_ip6: crate::net::ipv6::Ipv6Addr,
    local_port: u16,
    remote_port: u16,
    tx: &mut TcpTxBatch,
    seq: TcpSeq,
    ack: TcpAck,
    flags: u16,
    window: u16,
    data: &[u8],
) -> bool {
    if is_v6 {
        tx.push6(
            &local_ip6,
            &remote_ip6,
            local_port,
            remote_port,
            seq,
            ack,
            flags,
            window,
            data,
        )
    } else {
        tx.push(
            local_ip,
            remote_ip,
            local_port,
            remote_port,
            seq,
            ack,
            flags,
            window,
            data,
        )
    }
}

/// TCP sequence number
pub type TcpSeq = u32;

/// TCP acknowledgment number
pub type TcpAck = u32;

/// TCP header
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpHdr {
    /// Source port
    pub source: TcpPort,
    /// Destination port
    pub dest: TcpPort,
    /// Sequence number
    pub seq: TcpSeq,
    /// Acknowledgment number
    pub ack_seq: TcpAck,
    /// Data offset + reserved (byte 12 on wire)
    pub dof_res: u8,
    /// TCP flags: FIN SYN RST PSH ACK URG ECE CWR (byte 13 on wire)
    pub flags: u8,
    /// Window size (bytes 14-15 on wire, big-endian)
    pub window: u16,
    /// Checksum
    pub check: u16,
    /// Urgent pointer
    pub urg_ptr: u16,
}

impl TcpHdr {
    /// Create TCP header from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < TCP_MIN_HLEN {
            return None;
        }

        // SAFETY: data has at least TCP_MIN_HLEN bytes; the resulting reference
        // lifetime is 'static because it aliases the skb data which lives until
        // the packet is freed (longer than any per-function borrow).
        unsafe {
            Some(&*(data.as_ptr() as *const TcpHdr))
        }
    }

    /// Get data offset (in 32-bit words)
    pub fn dof(&self) -> u8 {
        self.dof_res >> 4
    }

    /// Get TCP header length (in bytes)
    pub fn header_len(&self) -> usize {
        (self.dof() as usize) * 4
    }

    /// Check SYN flag
    pub fn syn(&self) -> bool {
        (self.flags & 0x02) != 0
    }

    /// Check ACK flag
    pub fn ack(&self) -> bool {
        (self.flags & 0x10) != 0
    }

    /// Check FIN flag
    pub fn fin(&self) -> bool {
        (self.flags & 0x01) != 0
    }

    /// Check RST flag
    pub fn rst(&self) -> bool {
        (self.flags & 0x04) != 0
    }

    /// Check PSH flag
    pub fn psh(&self) -> bool {
        (self.flags & 0x08) != 0
    }

    /// Get window size
    pub fn window(&self) -> u16 {
        u16::from_be(self.window)
    }

    /// Set data offset
    pub fn set_dof(&mut self, dof: u8) {
        self.dof_res = (dof << 4) | (self.dof_res & 0x0F);
    }

    /// Set SYN flag
    pub fn set_syn(&mut self) {
        self.flags |= 0x02;
    }

    /// Set ACK flag
    pub fn set_ack(&mut self) {
        self.flags |= 0x10;
    }

    /// Set FIN flag
    pub fn set_fin(&mut self) {
        self.flags |= 0x01;
    }

    /// Set RST flag
    pub fn set_rst(&mut self) {
        self.flags |= 0x04;
    }

    /// Set PSH flag
    pub fn set_psh(&mut self) {
        self.flags |= 0x08;
    }

    /// Set window size
    pub fn set_window(&mut self, win: u16) {
        self.window = win.to_be();
    }
}

/// TCP states
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum TcpState {
    /// Closed
    TCP_CLOSE = 0,
    /// Listening
    TCP_LISTEN = 1,
    /// SYN sent
    TCP_SYN_SENT = 2,
    /// SYN received
    TCP_SYN_RECV = 3,
    /// Established
    TCP_ESTABLISHED = 4,
    /// FIN wait 1
    TCP_FIN_WAIT1 = 5,
    /// FIN wait 2
    TCP_FIN_WAIT2 = 6,
    /// Close wait
    TCP_CLOSE_WAIT = 7,
    /// Last ACK
    TCP_LAST_ACK = 8,
    /// Time wait
    TCP_TIME_WAIT = 9,
    /// Closing
    TCP_CLOSING = 10,
}

/// TCP send segment (for retransmission queue)
///
/// Stores copy of sent but unacknowledged data. R35: the `new()` helper
/// that did `Vec::from(data)` was removed — tx_packets now try_reserve()s
/// the copy before consuming from the send buffer, so an OOM degrades to
/// a clean stop with the buffer intact instead of hitting
/// alloc_error_handler (panic → CPU parked in `wfi` holding
/// TCP_TABLE_LOCK — the R34 wedge class).
#[derive(Debug, Clone)]
pub struct TcpSendSeg {
    /// Starting sequence number
    pub seq: TcpSeq,
    /// Data length
    pub len: usize,
    /// Data copy
    pub data: alloc::vec::Vec<u8>,
    /// Send timestamp (jiffies)
    pub tx_time: u64,
    /// Retransmit count
    pub retries: u32,
}

/// TCP out-of-order segment (for reassembly queue)
#[derive(Debug, Clone)]
pub struct TcpOooSeg {
    /// Starting sequence number
    pub seq: TcpSeq,
    /// Segment data
    pub data: alloc::vec::Vec<u8>,
}

// ============================================================================
// R35: deferred TX staging (chain-2 fix)
// ============================================================================
//
// The TCP state machine used to emit every segment INLINE while holding
// TCP_TABLE_LOCK (send_ack/send_syn/... → alloc_skb → ipv4_send_src →
// ethernet_send → virtio xmit). virtio xmit waits for device completion
// with a VIRTIO_QUEUE_TIMEOUT_US (10M) spin plus a 50M-iteration
// late-drain loop — seconds per packet under tcg. Every ack / retransmit /
// data segment therefore serialized ALL CPUs' networking (RX softirq,
// timer tick, every socket syscall) against one slow TX, while the table
// lock was held.
//
// New discipline — "decide under the lock, emit after the lock":
//   1. The TCP_TABLE_LOCK holder creates a TcpTxBatch and reserves its
//      capacity BEFORE taking the lock (try_reserve: an OOM here is a
//      clean error, not the alloc_error_handler panic that parks the CPU
//      in `wfi` holding the lock — the R34 wedge signature).
//   2. The state machine RECORDS wire-ready segments into the batch
//      (plain field writes + memcpy into already-reserved capacity —
//      NO allocation can happen, and NO packet is emitted, under the
//      lock).
//   3. After the lock drops, `emit_all()` builds the skbs (alloc_skb is
//      fine here — allocation outside the lock is legal per R34) and
//      drives ipv4_send_src/virtio xmit.
//
// Re-entry safety of the unlocked emit: identical to the Socket::close
// precedent — emitted packets never re-enter tcp_rcv on this CPU
// (loopback_send only enqueues to the LO_BACKLOG drained later by
// ethernet_poll; virtio TX goes to the device), so no recursive
// TCP_TABLE_LOCK acquisition is possible.

/// One wire-ready TX decision (R35). Addresses/ports in host byte order;
/// P2 TCP option template for one outbound segment.
///
/// `kind` bits select what `tcp_build_packet*` appends after the 20-byte
/// header: bit0 MSS, bit1 window scale (SYN only), bit2 timestamps.
/// Batch-wide template — set right before pushing the descriptor(s),
/// mirrored into each TcpTxDesc at push time, reset by emit_all.
#[derive(Debug, Clone, Copy)]
pub struct TcpOptOut {
    pub kind: u8,
    pub mss: u16,
    pub ws_shift: u8,
    pub tsval: u32,
    pub tsecr: u32,
}

impl Default for TcpOptOut {
    fn default() -> Self {
        Self { kind: 0, mss: 0, ws_shift: 0, tsval: 0, tsecr: 0 }
    }
}

impl TcpOptOut {
    pub const KIND_MSS: u8 = 1 << 0;
    pub const KIND_WS: u8 = 1 << 1;
    pub const KIND_TS: u8 = 1 << 2;

    /// Wire length of the enabled options in bytes (SYN layout:
    /// MSS(4) + WS(3)+NOP pad(1) + NOPNOP(2)+TS(10) = 20; data layout:
    /// NOPNOP(2)+TS(10) = 12; padded to a 4-byte multiple).
    fn wire_len(&self, syn: bool) -> usize {
        let mut n = 0usize;
        if syn && self.kind & Self::KIND_MSS != 0 {
            n += 4;
        }
        if syn && self.kind & Self::KIND_WS != 0 {
            n += 4;
        }
        if self.kind & Self::KIND_TS != 0 {
            n += 12;
        }
        (n + 3) & !3
    }

    /// Serialize the options into `buf`; returns the byte count written
    /// (0 when nothing is enabled). `syn` gates MSS/WS to handshake
    /// segments only (RFC 793/1323).
    fn encode(&self, buf: &mut [u8], syn: bool) -> usize {
        let mut n = 0usize;
        let mut put = |buf: &mut [u8], n: &mut usize, b: u8| {
            buf[*n] = b;
            *n += 1;
        };
        if syn && self.kind & Self::KIND_MSS != 0 {
            put(buf, &mut n, 2); // kind: MSS
            put(buf, &mut n, 4); // len
            put(buf, &mut n, (self.mss >> 8) as u8);
            put(buf, &mut n, self.mss as u8);
        }
        if syn && self.kind & Self::KIND_WS != 0 {
            put(buf, &mut n, 3); // kind: WS
            put(buf, &mut n, 3); // len
            put(buf, &mut n, self.ws_shift);
            put(buf, &mut n, 1); // NOP pad to 4
        }
        if self.kind & Self::KIND_TS != 0 {
            put(buf, &mut n, 1); // NOP
            put(buf, &mut n, 1); // NOP
            put(buf, &mut n, 8); // kind: TS
            put(buf, &mut n, 10); // len
            buf[n..n + 4].copy_from_slice(&self.tsval.to_be_bytes());
            n += 4;
            buf[n..n + 4].copy_from_slice(&self.tsecr.to_be_bytes());
            n += 4;
        }
        // Pad with EOL to the 4-byte multiple the caller reserved.
        while n % 4 != 0 {
            put(buf, &mut n, 0);
        }
        n
    }
}

/// `tcp_build_packet`/`ipv4_send_src` convert at emit time exactly like
/// the old inline senders did.
///
/// P1 IPv6: `is_v6` descriptors carry the endpoints in `src_ip6`/`dst_ip6`
/// and are emitted through tcp_build_packet6 + ipv6_send; the u32 fields
/// stay 0 for them.
#[derive(Debug, Clone, Copy)]
pub struct TcpTxDesc {
    /// Source IP (host order)
    pub src_ip: u32,
    /// Destination IP (host order)
    pub dst_ip: u32,
    /// Source port (host order)
    pub src_port: u16,
    /// Destination port (host order)
    pub dst_port: u16,
    /// Sequence number
    pub seq: TcpSeq,
    /// Acknowledgment number
    pub ack: TcpAck,
    /// TCP flags (SYN 0x02, RST 0x04, PSH 0x08, ACK 0x10, FIN 0x01)
    pub flags: u16,
    /// IP_TTL for this segment (P2): 0 = system default (64).
    pub ttl: u8,
    /// Advertised window
    pub window: u16,
    /// Payload offset into the batch arena
    pub off: usize,
    /// Payload length
    pub len: usize,
    /// P1 IPv6: emit through the v6 path
    pub is_v6: bool,
    /// P1 IPv6: source address (network byte order)
    pub src_ip6: crate::net::ipv6::Ipv6Addr,
    /// P1 IPv6: destination address (network byte order)
    pub dst_ip6: crate::net::ipv6::Ipv6Addr,
    /// P2 TCP options to append (see TcpOptOut).
    pub opts: TcpOptOut,
}

/// Batch of pending TX segments (R35).
///
/// Capacity MUST be reserved (outside TCP_TABLE_LOCK) before the state
/// machine appends; `push` never allocates — it fails cleanly instead, and
/// every caller treats that failure as a recoverable drop (peer retransmit
/// / timer re-fire / next-tick deferral), never a panic under the lock.
pub struct TcpTxBatch {
    descs: alloc::vec::Vec<TcpTxDesc>,
    arena: alloc::vec::Vec<u8>,
    /// P2 IP_TTL: stamped into every pushed descriptor (0 = default).
    ttl: u8,
    /// P2 TCP options: template stamped into every pushed descriptor.
    /// Set by SYN/TS senders right before their push; emit_all resets it.
    opts: TcpOptOut,
}

impl TcpTxBatch {
    pub const fn new() -> Self {
        Self {
            descs: alloc::vec::Vec::new(),
            arena: alloc::vec::Vec::new(),
            ttl: 0,
            opts: TcpOptOut { kind: 0, mss: 0, ws_shift: 0, tsval: 0, tsecr: 0 },
        }
    }

    /// Set the IP TTL used for segments recorded from now on (P2 IP_TTL).
    /// Callers holding the TcpSocket set the socket's mirrored value
    /// before pushing; batches that never got a TTL use the system
    /// default at the IPv4 layer.
    pub fn set_ttl(&mut self, ttl: u8) {
        self.ttl = ttl;
    }

    /// P2 TCP options: arm the handshake template (MSS + window scale +
    /// timestamps) for the descriptor(s) pushed next.
    pub fn set_opts_syn(&mut self, mss: u16, ws_shift: u8, tsval: u32) {
        self.opts = TcpOptOut {
            kind: TcpOptOut::KIND_MSS | TcpOptOut::KIND_WS | TcpOptOut::KIND_TS,
            mss,
            ws_shift,
            tsval,
            tsecr: 0,
        };
    }

    /// P2 TCP options: arm the data-segment timestamp template.
    pub fn set_opts_ts(&mut self, tsval: u32, tsecr: u32) {
        self.opts = TcpOptOut {
            kind: TcpOptOut::KIND_TS,
            mss: 0,
            ws_shift: 0,
            tsval,
            tsecr,
        };
    }

    /// P2 TCP options: clear the template (default = no options).
    pub fn clear_opts(&mut self) {
        self.opts = TcpOptOut::default();
    }

    /// Reserve capacity for `ndesc` segments totaling <= `nbytes` of
    /// payload. MUST run OUTSIDE TCP_TABLE_LOCK. Returns false on OOM
    /// (caller decides: clean syscall error, or degraded drop-mode).
    pub fn reserve(&mut self, ndesc: usize, nbytes: usize) -> bool {
        self.descs.try_reserve_exact(ndesc).is_ok() && self.arena.try_reserve_exact(nbytes).is_ok()
    }

    /// Record one segment (memcpy into reserved capacity — no allocation,
    /// safe under TCP_TABLE_LOCK). Returns false when the batch is full
    /// (sizing was a worst-case bound, so this is a can't-happen guard).
    pub fn push(
        &mut self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        seq: TcpSeq,
        ack: TcpAck,
        flags: u16,
        window: u16,
        data: &[u8],
    ) -> bool {
        if self.descs.len() >= self.descs.capacity()
            || self.arena.capacity() - self.arena.len() < data.len()
        {
            return false;
        }
        let off = self.arena.len();
        // Within reserved capacity: pure memcpy, cannot allocate.
        self.arena.extend_from_slice(data);
        self.descs.push(TcpTxDesc {
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            seq,
            ack,
            flags,
            window,
            off,
            len: data.len(),
            ttl: self.ttl,
            is_v6: false,
            src_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            dst_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            opts: self.opts,
        });
        true
    }

    /// P1 IPv6: record a segment with v6 endpoints (see push).
    #[allow(clippy::too_many_arguments)]
    pub fn push6(
        &mut self,
        src_ip6: &crate::net::ipv6::Ipv6Addr,
        dst_ip6: &crate::net::ipv6::Ipv6Addr,
        src_port: u16,
        dst_port: u16,
        seq: TcpSeq,
        ack: TcpAck,
        flags: u16,
        window: u16,
        data: &[u8],
    ) -> bool {
        if self.descs.len() >= self.descs.capacity()
            || self.arena.capacity() - self.arena.len() < data.len()
        {
            return false;
        }
        let off = self.arena.len();
        // Within reserved capacity: pure memcpy, cannot allocate.
        self.arena.extend_from_slice(data);
        self.descs.push(TcpTxDesc {
            src_ip: 0,
            dst_ip: 0,
            src_port,
            dst_port,
            seq,
            ack,
            flags,
            window,
            off,
            len: data.len(),
            ttl: self.ttl,
            is_v6: true,
            src_ip6: *src_ip6,
            dst_ip6: *dst_ip6,
            opts: self.opts,
        });
        true
    }

    /// Record a header-only segment (SYN/ACK/FIN/RST — no payload).
    pub fn push_ctl(
        &mut self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        seq: TcpSeq,
        ack: TcpAck,
        flags: u16,
        window: u16,
    ) -> bool {
        self.push(src_ip, dst_ip, src_port, dst_port, seq, ack, flags, window, &[])
    }

    /// P1 IPv6: record a header-only v6 segment.
    #[allow(clippy::too_many_arguments)]
    pub fn push_ctl6(
        &mut self,
        src_ip6: &crate::net::ipv6::Ipv6Addr,
        dst_ip6: &crate::net::ipv6::Ipv6Addr,
        src_port: u16,
        dst_port: u16,
        seq: TcpSeq,
        ack: TcpAck,
        flags: u16,
        window: u16,
    ) -> bool {
        self.push6(src_ip6, dst_ip6, src_port, dst_port, seq, ack, flags, window, &[])
    }

    /// Remaining descriptor slots.
    pub fn desc_room(&self) -> usize {
        self.descs.capacity() - self.descs.len()
    }

    /// Remaining arena bytes.
    pub fn arena_room(&self) -> usize {
        self.arena.capacity() - self.arena.len()
    }

    /// Append payload bytes straight from the send buffer's drain iterator
    /// into the reserved arena (tx_packets path: one memcpy, no
    /// intermediate Vec, no allocation). Caller must have checked
    /// `arena_room() >= seg_size`. Returns the arena offset of the copy.
    pub fn arena_extend_drain<I: Iterator<Item = u8>>(&mut self, drain: I) -> usize {
        let off = self.arena.len();
        // Within reserved capacity: extend cannot allocate.
        self.arena.extend(drain);
        off
    }

    /// Commit a descriptor for payload placed via `arena_extend_drain`.
    pub fn commit(&mut self, src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16,
                  seq: TcpSeq, ack: TcpAck, flags: u16, window: u16, off: usize, len: usize) {
        self.descs.push(TcpTxDesc {
            src_ip, dst_ip, src_port, dst_port, seq, ack, flags, window, off, len,
            ttl: self.ttl,
            is_v6: false,
            src_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            dst_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            opts: self.opts,
        });
    }

    /// P1 IPv6: commit a v6 descriptor for arena-placed payload.
    #[allow(clippy::too_many_arguments)]
    pub fn commit6(&mut self, src_ip6: &crate::net::ipv6::Ipv6Addr, dst_ip6: &crate::net::ipv6::Ipv6Addr,
                   src_port: u16, dst_port: u16,
                   seq: TcpSeq, ack: TcpAck, flags: u16, window: u16, off: usize, len: usize) {
        self.descs.push(TcpTxDesc {
            src_ip: 0, dst_ip: 0, src_port, dst_port, seq, ack, flags, window, off, len,
            ttl: self.ttl,
            is_v6: true,
            src_ip6: *src_ip6,
            dst_ip6: *dst_ip6,
            opts: self.opts,
        });
    }

    /// Arena slice for a committed descriptor (retrans-copy source).
    pub fn arena_slice(&self, off: usize, len: usize) -> &[u8] {
        &self.arena[off..off + len]
    }

    /// Emit every pending segment. MUST run OUTSIDE TCP_TABLE_LOCK
    /// (alloc_skb + ipv4_send_src → virtio xmit may spin on device
    /// completion — the chain-2 hazard). Individual failures drop that
    /// segment (TCP recovers via peer retransmit / our timers).
    pub fn emit_all(&mut self) {
        for d in self.descs.drain(..) {
            let data = &self.arena[d.off..d.off + d.len];
            let mut skb = match crate::net::buffer::alloc_skb(1500) {
                Some(s) => s,
                None => continue, // R35: allocation failure outside the lock — drop, no panic
            };
            if !data.is_empty() && skb.skb_put_data(data).is_err() {
                skb.free();
                continue;
            }
            // P1 IPv6: v6 descriptors carry the 128-bit endpoints and use
            // the v6 checksum + output path.
            if d.is_v6 {
                if tcp_build_packet6(
                    &mut skb,
                    d.src_port,
                    d.dst_port,
                    d.seq,
                    d.ack,
                    d.flags,
                    d.window,
                    &d.src_ip6,
                    &d.dst_ip6,
                    &d.opts,
                )
                .is_err()
                {
                    skb.free();
                    continue;
                }
                let _ = crate::net::ipv6::ipv6_send_hops(
                    skb,
                    &d.src_ip6,
                    &d.dst_ip6,
                    crate::net::ipv6::next_header::TCP,
                    d.ttl,
                );
                continue;
            }
            if tcp_build_packet(
                &mut skb,
                d.src_port,
                d.dst_port,
                d.seq,
                d.ack,
                d.flags,
                d.window,
                d.src_ip.to_be(),
                d.dst_ip.to_be(),
                &d.opts,
            )
            .is_err()
            {
                skb.free();
                continue;
            }
            let _ = crate::net::ipv4::ipv4_send_src_ttl(skb, d.src_ip, d.dst_ip, 6, d.ttl);
        }
        self.arena.clear();
        self.opts = TcpOptOut::default();
    }
}

impl Default for TcpTxBatch {
    fn default() -> Self {
        Self::new()
    }
}

/// TCP RTT estimator (RFC 6298)
#[derive(Debug, Clone)]
pub struct TcpRttEstimator {
    /// Smoothed RTT (microseconds)
    pub srtt: u64,
    /// RTT variance (microseconds)
    pub rttvar: u64,
    /// Current RTO (microseconds)
    pub rto: u64,
}

impl TcpRttEstimator {
    pub fn new() -> Self {
        Self {
            srtt: 0,
            rttvar: 0,
            rto: TCP_RTO_DEFAULT_US,
        }
    }

    /// Update RTT estimate (RFC 6298)
    ///
    /// # Arguments
    /// - `rtt_sample`: RTT sample (microseconds)
    pub fn update(&mut self, rtt_sample: u64) {
        if self.srtt == 0 {
            // First measurement
            self.srtt = rtt_sample;
            self.rttvar = rtt_sample / 2;
        } else {
            // RFC 6298 algorithm
            let delta = if rtt_sample > self.srtt {
                rtt_sample - self.srtt
            } else {
                self.srtt - rtt_sample
            };
            self.rttvar = (3 * self.rttvar + delta) / 4;
            self.srtt = (7 * self.srtt + rtt_sample) / 8;
        }

        // Calculate RTO = SRTT + 4 * RTTVAR
        self.rto = self.srtt.saturating_add(4 * self.rttvar);
        self.rto = self.rto.clamp(TCP_RTO_MIN_US, TCP_RTO_MAX_US);
    }

    /// RTO exponential backoff
    pub fn backoff(&mut self) {
        self.rto = core::cmp::min(self.rto * 2, TCP_RTO_MAX_US);
    }

    /// Reset RTO (after connection establishment)
    pub fn reset(&mut self) {
        self.rto = TCP_RTO_DEFAULT_US;
    }
}

impl Default for TcpRttEstimator {
    fn default() -> Self {
        Self::new()
    }
}

/// TCP congestion control states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpCongState {
    /// Slow start
    SlowStart,
    /// Congestion avoidance
    CongestionAvoidance,
    /// Fast recovery
    FastRecovery,
}

impl Default for TcpCongState {
    fn default() -> Self {
        TcpCongState::SlowStart
    }
}

/// TCP congestion control (RFC 5681)
#[derive(Debug, Clone)]
pub struct TcpCongestion {
    /// Congestion window (bytes)
    pub cwnd: u32,
    /// Slow start threshold (bytes)
    pub ssthresh: u32,
    /// Current congestion state
    pub state: TcpCongState,
    /// Duplicate ACK count
    pub dup_ack_count: u32,
    /// Recovery point sequence number
    pub recover_seq: TcpSeq,
}

impl TcpCongestion {
    pub fn new(mss: u16) -> Self {
        Self {
            cwnd: mss as u32,      // Initial 1 MSS
            ssthresh: u32::MAX,    // Initially infinite
            state: TcpCongState::SlowStart,
            dup_ack_count: 0,
            recover_seq: 0,
        }
    }

    /// Update congestion window on ACK received
    pub fn on_ack(&mut self, acked_bytes: u32, mss: u16) {
        match self.state {
            TcpCongState::SlowStart => {
                // Slow start: cwnd increases by 1 MSS per ACK
                self.cwnd += mss as u32;
                if self.cwnd >= self.ssthresh {
                    self.state = TcpCongState::CongestionAvoidance;
                }
            }
            TcpCongState::CongestionAvoidance => {
                // Congestion avoidance: cwnd increases by 1 MSS per RTT
                // i.e., each ACK increases by MSS * MSS / cwnd
                let increment = (mss as u32 * mss as u32) / core::cmp::max(self.cwnd, 1);
                self.cwnd += increment;
            }
            TcpCongState::FastRecovery => {
                // Fast recovery: received ACK for new data, end fast recovery
                self.state = TcpCongState::CongestionAvoidance;
            }
        }
    }

    /// Received duplicate ACK
    pub fn on_dup_ack(&mut self, ack: TcpSeq, snd_nxt: TcpSeq, mss: u16) {
        self.dup_ack_count += 1;

        if self.dup_ack_count == 3 && Self::seq_before(ack, snd_nxt) {
            // Fast retransmit: 3 duplicate ACKs
            // Set ssthresh = max(cwnd/2, 2*MSS)
            self.ssthresh = core::cmp::max(self.cwnd / 2, 2 * mss as u32);

            // Set cwnd = ssthresh + 3*MSS
            self.cwnd = self.ssthresh + 3 * mss as u32;

            // Record recovery point
            self.recover_seq = snd_nxt;

            // Enter fast recovery
            self.state = TcpCongState::FastRecovery;
        } else if self.state == TcpCongState::FastRecovery {
            // In fast recovery, received duplicate ACK, increase cwnd
            self.cwnd += mss as u32;
        }
    }

    /// Timeout handling
    pub fn on_timeout(&mut self, mss: u16) {
        // Timeout is severe congestion
        self.ssthresh = core::cmp::max(self.cwnd / 2, 2 * mss as u32);
        self.cwnd = mss as u32; // Reset to 1 MSS
        self.state = TcpCongState::SlowStart;
        self.dup_ack_count = 0;
    }

    /// Reset (new connection)
    pub fn reset(&mut self, mss: u16) {
        self.cwnd = mss as u32;
        self.ssthresh = u32::MAX;
        self.state = TcpCongState::SlowStart;
        self.dup_ack_count = 0;
        self.recover_seq = 0;
    }

    /// Sequence number comparison: a before b
    pub fn seq_before(a: TcpSeq, b: TcpSeq) -> bool {
        ((a as i32) - (b as i32)) < 0
    }
}

impl Default for TcpCongestion {
    fn default() -> Self {
        Self::new(TCP_DEFAULT_MSS)
    }
}

/// TCP timer states
#[derive(Debug, Clone)]
pub struct TcpTimers {
    /// Retransmit timer deadline (jiffies), 0 means inactive
    pub retransmit_deadline: u64,
    /// Delayed ACK timer deadline (jiffies)
    pub delack_deadline: u64,
    /// W3: zero-window persist probe deadline (jiffies), 0 inactive.
    /// Armed by tx_packets when the peer advertises a zero window while
    /// data is queued; the timer tick sends 1-byte probes.
    pub persist_deadline: u64,
    /// R21-N3b: when FIN_WAIT1/2 was entered (jiffies) — bounds orphaned
    /// half-closes against dead peers.
    pub fin_wait_since: u64,
    /// R32-B8: when CLOSE_WAIT was entered (jiffies) — bounds orphaned
    /// half-closed connections (peer FINed, no fd ever claimed the slot)
    /// so the 64-slot table cannot be exhausted by e.g. port scans.
    pub close_wait_since: u64,
    /// R32-N16: SYN retransmission count while in SYN_SENT — a lost SYN
    /// used to leave connect() hanging in SYN_SENT forever (the timer
    /// tick's catch-all arm did nothing for the state).
    pub syn_retries: u32,
    /// P2 SO_KEEPALIVE: keepalive probe deadline (jiffies, 0 = disarmed;
    /// armed by the timer tick for idle ESTABLISHED sockets).
    pub keepalive_deadline: u64,
    /// P2 SO_KEEPALIVE: probes sent without receiving an ACK (reset by
    /// process_ack; keepcnt unanswered probes abort the connection).
    pub keepalive_probes: u32,
}

impl TcpTimers {
    pub fn new() -> Self {
        Self {
            retransmit_deadline: 0,
            fin_wait_since: 0,
            delack_deadline: 0,
            persist_deadline: 0,
            close_wait_since: 0,
            syn_retries: 0,
            keepalive_deadline: 0,
            keepalive_probes: 0,
        }
    }

    /// Start retransmit timer
    pub fn start_retransmit(&mut self, rto_us: u64) {
        let now = crate::drivers::timer::get_jiffies();
        // Microseconds to jiffies (1 jiffy = 10ms = 10_000us)
        let rto_jiffies = (rto_us / 10_000).max(1);
        self.retransmit_deadline = now + rto_jiffies;
    }

    /// Stop retransmit timer
    pub fn stop_retransmit(&mut self) {
        self.retransmit_deadline = 0;
    }

    /// Check if retransmit timer expired
    pub fn retransmit_expired(&self) -> bool {
        if self.retransmit_deadline == 0 {
            return false;
        }
        let now = crate::drivers::timer::get_jiffies();
        now >= self.retransmit_deadline
    }
}

impl Default for TcpTimers {
    fn default() -> Self {
        Self::new()
    }
}

/// TCP Socket structure
///
/// Contains connection state, sequence numbers, reliability mechanisms, etc.
#[repr(C)]
pub struct TcpSocket {
    // === Basic connection info ===
    /// Local port
    pub local_port: TcpPort,
    /// Remote port
    pub remote_port: TcpPort,
    /// Remote IP address
    pub remote_ip: u32,
    /// Local IP address
    pub local_ip: u32,
    /// P1 IPv6: pure v6 connection (v4-mapped peers normalize to the u32
    /// fields at the syscall boundary)
    pub is_v6: bool,
    /// P1 IPv6: local address
    pub local_ip6: crate::net::ipv6::Ipv6Addr,
    /// P1 IPv6: remote address
    pub remote_ip6: crate::net::ipv6::Ipv6Addr,
    /// TCP state
    pub state: TcpState,
    /// Whether bound
    pub bound: bool,

    // === Server (accept) bookkeeping ===
    /// For sockets spawned by an inbound SYN: the protocol-table index of
    /// the LISTEN socket they belong to. accept() matches on this
    /// (review NET-C4).
    pub parent_fd: Option<i32>,
    /// Set once a process fd has been handed out for this connection, so a
    /// second accept() cannot claim the same connection.
    pub accepted: bool,

    // === Sequence number management ===
    /// Send sequence number (next to send)
    pub snd_nxt: TcpSeq,
    /// Send unacknowledged sequence number (earliest unacknowledged)
    pub snd_una: TcpSeq,
    /// Receive sequence number (next expected)
    pub rcv_nxt: TcpSeq,

    // === Sliding window ===
    /// Send window (advertised by peer)
    pub snd_wnd: u16,
    /// Receive window (advertised by us)
    pub rcv_wnd: u16,

    // === Buffers ===
    /// Send buffer (data waiting to be sent)
    pub send_buffer: alloc::collections::VecDeque<u8>,
    /// Receive buffer (received but unread data)
    pub recv_buffer: alloc::collections::VecDeque<u8>,
    /// Retransmit queue (sent but unacknowledged)
    pub retrans_queue: alloc::collections::VecDeque<TcpSendSeg>,
    /// R21-N4: userspace reference count — the timer/state machine may
    /// transition to CLOSE (RST, retrans exhaustion) while a process fd
    /// still wraps this slot; freeing it let alloc() reuse the index and
    /// the stale fd read/write a stranger's connection.
    pub user_refs: core::sync::atomic::AtomicU32,
    /// R32-B7: set when the last userspace fd dropped the slot while the
    /// connection was still closing (FIN_WAIT/LAST_ACK). The timer sweep
    /// frees no-parent (client) CLOSE corpses only when this is set —
    /// without it they are indistinguishable from fresh pre-connect slots
    /// (user_refs==0, no parent_fd) and would leak forever.
    pub orphaned: bool,
    /// W3: pending protocol error (positive errno, 0 = none) — set on RST
    /// (ECONNRESET/ECONNREFUSED), retransmit exhaustion (ETIMEDOUT) and
    /// ICMP errors (tcp_v4_err). Surfaced by recv/send and read-and-cleared
    /// by getsockopt(SO_ERROR).
    pub pending_error: i32,
    /// W3: SO_REUSEADDR mirrored from the VFS layer — participates in the
    /// bind-conflict decision (Linux: both binders opting in may coexist).
    pub reuseaddr: bool,
    /// P2 SO_KEEPALIVE mirrored from the VFS layer (tcp_set_keepalive):
    /// the timer tick arms the idle-probe cycle for ESTABLISHED sockets.
    pub keepalive: bool,
    /// TCP_KEEPIDLE in seconds (Linux default 7200).
    pub ka_idle_s: u32,
    /// TCP_KEEPINTVL in seconds (Linux default 75).
    pub ka_intvl_s: u32,
    /// TCP_KEEPCNT (Linux default 9).
    pub ka_cnt: u32,
    /// P2 IP_TTL mirrored from the VFS layer (tcp_set_ttl): 0 = default.
    pub ttl: u8,
    /// P2 TCP options: peer's window-scale shift parsed from its SYN
    /// (0 = none offered — inbound window used verbatim).
    pub peer_wscale: u8,
    /// P2 TCP options: timestamps negotiated (both SYNs offered TS).
    pub ts_enabled: bool,
    /// P2 TCP options: most recent TSval received from the peer — echoed
    /// back as TSecr on our segments (RFC 7323 §3.2).
    pub ts_recent: u32,
    /// Out-of-order reassembly queue (received but not yet deliverable)
    pub ooo_queue: alloc::collections::VecDeque<TcpOooSeg>,

    // === Reliability mechanisms ===
    /// RTT estimator
    pub rtt_estimator: TcpRttEstimator,
    /// Congestion control
    pub congestion: TcpCongestion,
    /// Timers
    pub timers: TcpTimers,

    // === Connection parameters ===
    /// Maximum segment size
    pub mss: u16,
    /// Initial sequence number
    pub isn: TcpSeq,

    // === Backward compatibility ===
    /// Window size (deprecated, use snd_wnd)
    #[deprecated]
    pub window: u16,
}

impl TcpSocket {
    /// Create new TCP Socket
    pub fn new() -> Self {
        Self {
            local_port: 0,
            remote_port: 0,
            remote_ip: 0,
            local_ip: 0,
            is_v6: false,
            local_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            remote_ip6: crate::net::ipv6::IPV6_ADDR_UNSPECIFIED,
            state: TcpState::TCP_CLOSE,
            bound: false,
            parent_fd: None,
            accepted: false,

            snd_nxt: 0,
            snd_una: 0,
            rcv_nxt: 0,

            snd_wnd: TCP_MAX_WINDOW,
            rcv_wnd: TCP_MAX_WINDOW,

            send_buffer: alloc::collections::VecDeque::new(),
            recv_buffer: alloc::collections::VecDeque::new(),
            retrans_queue: alloc::collections::VecDeque::new(),
            user_refs: core::sync::atomic::AtomicU32::new(0),
            orphaned: false,
            pending_error: 0,
            reuseaddr: false,
            keepalive: false,
            ka_idle_s: 7200,
            ka_intvl_s: 75,
            ka_cnt: 9,
            ttl: 0,
            peer_wscale: 0,
            ts_enabled: false,
            ts_recent: 0,
            ooo_queue: alloc::collections::VecDeque::new(),

            rtt_estimator: TcpRttEstimator::new(),
            congestion: TcpCongestion::new(TCP_DEFAULT_MSS),
            timers: TcpTimers::new(),

            mss: TCP_DEFAULT_MSS,
            isn: 0,

            #[allow(deprecated)]
            window: TCP_MAX_WINDOW,
        }
    }

    /// Bind to port
    ///
    /// # Arguments
    /// - `port`: Port number
    pub fn bind(&mut self, port: TcpPort) -> Result<(), ()> {
        self.local_port = port;
        self.bound = true;
        Ok(())
    }

    // ==================== P1 IPv6: family-aware TX recording ====================

    /// Record a data/ctl segment honoring the socket's address family.
    /// All internal senders route through this so v4 sockets and v6
    /// sockets share one state machine.
    ///
    /// P2 TCP timestamps: negotiated connections stamp every outbound
    /// segment (TSval = now, TSecr = ts_recent).
    #[allow(clippy::too_many_arguments)]
    fn tx_record(
        &self,
        tx: &mut TcpTxBatch,
        seq: TcpSeq,
        ack: TcpAck,
        flags: u16,
        window: u16,
        data: &[u8],
    ) -> bool {
        if self.ts_enabled {
            tx.set_opts_ts(tcp_ts_now(), self.ts_recent);
        } else {
            tx.clear_opts();
        }
        tx_record_endpoints(
            self.is_v6,
            self.local_ip,
            self.remote_ip,
            self.local_ip6,
            self.remote_ip6,
            self.local_port,
            self.remote_port,
            tx,
            seq,
            ack,
            flags,
            window,
            data,
        )
    }

    /// tx_record for a header-only segment.
    fn tx_record_ctl(
        &self,
        tx: &mut TcpTxBatch,
        seq: TcpSeq,
        ack: TcpAck,
        flags: u16,
        window: u16,
    ) -> bool {
        self.tx_record(tx, seq, ack, flags, window, &[])
    }

    /// Listen on port
    ///
    /// # Arguments
    /// - `backlog`: Wait queue length
    pub fn listen(&mut self, _backlog: u32) -> Result<(), ()> {
        if !self.bound {
            return Err(());
        }
        self.state = TcpState::TCP_LISTEN;
        Ok(())
    }

    /// Connect to remote address (active open, three-way handshake)
    ///
    /// # Arguments
    /// - `ip`: IP address
    /// - `port`: Port number
    /// - `tx`: R35 deferred-TX batch (SYN is recorded, emitted after the
    ///   table lock drops — chain-2 fix)
    pub fn connect(&mut self, ip: u32, port: TcpPort, tx: &mut TcpTxBatch) -> Result<(), ()> {
        self.remote_ip = ip;
        self.remote_port = port;

        // Initialize sequence number from connection 4-tuple
        self.snd_nxt = generate_isn(self.local_ip, self.local_port, ip, port);
        self.snd_una = self.snd_nxt;
        self.rcv_nxt = 0; // Will be obtained from SYN-ACK

        // Send SYN packet (first step of three-way handshake)
        self.send_syn(tx)?;
        self.state = TcpState::TCP_SYN_SENT;
        // R32-N16: arm the retransmit timer so the timer tick can
        // retransmit a lost SYN (bounded by syn_retries). Without this a
        // single lost SYN parked the socket in SYN_SENT forever.
        self.timers.start_retransmit(crate::config::TCP_RTO_DEFAULT_US);

        Ok(())
    }

    /// P1 IPv6: active open to a pure v6 remote. Same state machine as
    /// connect(); only the endpoints (and ISN hashing input) differ.
    pub fn connect6(
        &mut self,
        ip6: &crate::net::ipv6::Ipv6Addr,
        port: TcpPort,
        tx: &mut TcpTxBatch,
    ) -> Result<(), ()> {
        self.is_v6 = true;
        self.remote_ip6 = *ip6;
        self.remote_port = port;

        self.snd_nxt = generate_isn6(&self.local_ip6, self.local_port, ip6, port);
        self.snd_una = self.snd_nxt;
        self.rcv_nxt = 0;

        self.send_syn(tx)?;
        self.state = TcpState::TCP_SYN_SENT;
        self.timers.start_retransmit(crate::config::TCP_RTO_DEFAULT_US);

        Ok(())
    }

    /// Re-send the initial SYN (R32-N16, called from the timer tick).
    /// `snd_nxt` still holds the SYN's sequence number in SYN_SENT, so
    /// send_syn() re-emits the identical segment.
    pub fn resend_syn(&self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        self.send_syn(tx)
    }

    /// Send SYN packet (first step of three-way handshake)
    ///
    /// R35: records into `tx` instead of emitting — the virtio TX spin
    /// must never run under TCP_TABLE_LOCK.
    ///
    /// P2: the SYN carries MSS + window scale + timestamps options.
    fn send_syn(&self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        // Our window field stays unscaled (rcv_wnd is a plain u16), so the
        // advertised shift is 0 — offering the option still lets a scaled
        // peer know our window ceiling is exactly 64 KiB.
        // Bypasses tx_record (which resets the option template for data
        // segments — the SYN template set here must survive).
        tx.set_opts_syn(self.mss, 0, tcp_ts_now());
        if tx_record_endpoints(
            self.is_v6,
            self.local_ip,
            self.remote_ip,
            self.local_ip6,
            self.remote_ip6,
            self.local_port,
            self.remote_port,
            tx,
            self.snd_nxt,
            0, // ACK number is 0
            0x0002, // SYN flag
            self.rcv_wnd,
            &[],
        ) {
            Ok(())
        } else {
            // Degraded mode (staging exhausted — sizing makes this a
            // can't-happen): drop; the SYN retransmit timer re-fires.
            Err(())
        }
    }

    /// Send SYN-ACK packet (second step of three-way handshake)
    fn send_synack(&mut self, _ack_seq: TcpSeq, tx: &mut TcpTxBatch) -> Result<(), ()> {
        self.send_synack_packet(tx)?;

        // R14-7 (HIGH-3): the SYN consumes one sequence number. Without
        // this, the peer's rcv_nxt (= ISN+1) mismatched every subsequent
        // server segment, and the first client ACK made in_flight wrap
        // (usable_window 0) — server-side TX permanently blocked.
        self.snd_nxt = self.snd_nxt.wrapping_add(1);

        Ok(())
    }

    /// Emit the SYN-ACK segment itself without touching sequence
    /// accounting (R32-N16). Used both by the handshake and by the
    /// SYN_RECV retransmission path — resending must NOT advance snd_nxt.
    ///
    /// P2: carries MSS + window scale + timestamps options like the SYN.
    fn send_synack_packet(&self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        // Bypasses tx_record — the option template set here must survive
        // (see send_syn).
        tx.set_opts_syn(self.mss, 0, tcp_ts_now());
        if tx_record_endpoints(
            self.is_v6,
            self.local_ip,
            self.remote_ip,
            self.local_ip6,
            self.remote_ip6,
            self.local_port,
            self.remote_port,
            tx,
            self.snd_nxt,
            self.rcv_nxt,
            0x0012, // SYN + ACK flags
            self.rcv_wnd,
            &[],
        ) {
            Ok(())
        } else {
            Err(()) // dropped; client re-SYNs and the SYN_RECV arm resends
        }
    }

    /// Send ACK packet (third step of three-way handshake)
    fn send_ack(&self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        if self.tx_record_ctl(
            tx,
            self.snd_nxt,
            self.rcv_nxt,
            0x0010, // ACK flag
            self.rcv_wnd,
        ) {
            Ok(())
        } else {
            Err(()) // dropped; peer retransmits and we re-ACK
        }
    }

    /// Send FIN+ACK packet
    ///
    /// FIN consumes one sequence number per RFC 793, so snd_nxt is
    /// incremented after sending.
    fn send_fin(&mut self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        self.tx_record_ctl(
            tx,
            self.snd_nxt,
            self.rcv_nxt,
            0x0011, // FIN + ACK flags
            self.rcv_wnd,
        );
        // R35 note: even when the FIN descriptor is dropped (can't-happen
        // staging overflow), the sequence accounting below still runs —
        // the retrans_queue entry keeps the FIN recoverable via the RTO
        // timer, exactly like a wire-level FIN loss.

        // FIN consumes one sequence number (RFC 793)
        let fin_seq = self.snd_nxt;
        self.snd_nxt = self.snd_nxt.wrapping_add(1);

        // R21-N3: arm the retransmit machinery for the FIN itself — a lost
        // FIN (or final ACK) used to leave FIN_WAIT1/2 forever (no seg in
        // retrans_queue, deadline 0): 64 dead closes exhaust the table.
        // R35: try_reserve first — the deque-chunk growth is an allocation
        // under the lock; failure skips only the retrans arming (the
        // FIN_WAIT orphan timeout still bounds the state).
        if self.retrans_queue.try_reserve(1).is_err() {
            return Ok(());
        }
        self.retrans_queue.push_back(TcpSendSeg {
            seq: fin_seq,
            len: 1, // R23-4: FIN consumes one seq — len 1 keeps
                    // remove_acked_segments' (seg_end - ack <= 0) from
                    // retiring it on the data-ACK that precedes the FIN-ACK.
            data: alloc::vec::Vec::new(),
            tx_time: crate::drivers::timer::get_jiffies(),
            retries: 0,
        });
        if self.timers.retransmit_deadline == 0 {
            self.timers.start_retransmit(crate::config::TCP_RTO_DEFAULT_US);
        }

        Ok(())
    }

    /// Start TIME_WAIT timer (reuses retransmit_deadline field)
    fn start_timewait_timer(&mut self) {
        let now = crate::drivers::timer::get_jiffies();
        let tw_jiffies = crate::config::TCP_TIMEWAIT_TIMEOUT_US / 10_000;
        self.timers.retransmit_deadline = now + tw_jiffies;
    }

    /// Send ACK packet (public interface, for timers)
    pub fn send_ack_public(&self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        self.send_ack(tx)
    }

    /// Handle received TCP packet
    ///
    /// R35: `tx` receives every outbound segment this packet triggers
    /// (SYN-ACK/ACK/FIN/retransmit) as deferred descriptors; the caller
    /// emits them after dropping TCP_TABLE_LOCK.
    pub fn handle_packet(&mut self, tcp_hdr: &TcpHdr, data: &[u8], tx: &mut TcpTxBatch) -> Result<(), ()> {
        // Global RST handling (RFC 793 §3.9)
        if tcp_hdr.rst() {
            self.handle_rst_recv();
            return Ok(());
        }

        // W3: parse the MSS option on any SYN (connection's own or the
        // handshake counterpart's) — the peer's advertised MSS below our
        // 1460 default must be adopted or every segment we send above it
        // gets dropped by the peer (SYN options were ignored entirely).
        // P2: also adopt the peer's window-scale shift and negotiate
        // timestamps (RFC 7323: TS is on only when BOTH SYNs offered it).
        if tcp_hdr.syn() {
            if let Some(mss) = tcp_parse_mss(tcp_hdr) {
                if mss < self.mss {
                    self.mss = mss;
                }
            }
            match tcp_parse_wscale(tcp_hdr) {
                Some(ws) => self.peer_wscale = ws,
                None => self.peer_wscale = 0,
            }
            if let Some(tsval) = tcp_parse_tsval(tcp_hdr) {
                self.ts_recent = tsval;
                // ts_enabled stays true only if our SYN also offered TS
                // (we always do); a peer that skipped the option gets no
                // stamped segments from us.
                self.ts_enabled = true;
            } else {
                self.ts_enabled = false;
            }
        } else if self.ts_enabled {
            // RFC 7323: refresh ts_recent from every in-order segment's
            // TSval (PAWS ordering checks are not implemented — minimal
            // echo only).
            if let Some(tsval) = tcp_parse_tsval(tcp_hdr) {
                self.ts_recent = tsval;
            }
        }

        let has_data = !data.is_empty();

        match self.state {
            TcpState::TCP_LISTEN => {
                // Server: receive SYN packet
                if tcp_hdr.syn() && !tcp_hdr.ack() {
                    self.handle_syn_recv(tcp_hdr, tx)?;
                }
            }
            TcpState::TCP_SYN_SENT => {
                // Client: receive SYN-ACK packet
                if tcp_hdr.syn() && tcp_hdr.ack() {
                    self.handle_synack_recv(tcp_hdr, tx)?;
                }
            }
            TcpState::TCP_SYN_RECV => {
                // Server: retransmitted SYN means our SYN-ACK was lost —
                // resend it with the SAME sequence number (R32-N16; the
                // handshake path advances snd_nxt, the resend must not).
                if tcp_hdr.syn() && !tcp_hdr.ack() {
                    let _ = self.send_synack_packet(tx);
                }
                // Server: receive ACK packet
                if tcp_hdr.ack() && !tcp_hdr.syn() {
                    self.handle_ack_recv(tcp_hdr)?;
                }
            }
            TcpState::TCP_ESTABLISHED => {
                // Process ACK first (updates snd_una, snd_wnd, cwnd, rtt)
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    self.process_ack(ack_num, has_data, tcp_hdr.window(), tx);
                }
                // Process data (may accompany FIN)
                if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data, tx)?;
                }
                // Process FIN (may accompany data)
                if tcp_hdr.fin() {
                    self.handle_fin_recv(tx)?;
                }
            }
            TcpState::TCP_FIN_WAIT1 => {
                // Process ACK (only accept if it falls within our send window)
                let mut valid_ack = false;
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    valid_ack = self.process_ack(ack_num, has_data, tcp_hdr.window(), tx);
                }
                if tcp_hdr.fin() && valid_ack {
                    // Simultaneous close: FIN+ACK -> TIME_WAIT
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    let _ = self.send_ack(tx);
                    self.state = TcpState::TCP_TIME_WAIT;
                    self.start_timewait_timer();
                } else if valid_ack {
                    // ACK of our FIN -> FIN_WAIT2
                    self.state = TcpState::TCP_FIN_WAIT2;
                    // Data and/or FIN may follow
                    if !data.is_empty() {
                        self.handle_data_recv(tcp_hdr, data, tx)?;
                    }
                    if tcp_hdr.fin() {
                        self.handle_fin_recv(tx)?;
                    }
                } else if tcp_hdr.fin() {
                    // FIN without ACK -> CLOSING
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    let _ = self.send_ack(tx);
                    self.state = TcpState::TCP_CLOSING;
                } else if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data, tx)?;
                }
            }
            TcpState::TCP_FIN_WAIT2 => {
                // Waiting for FIN from remote
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    self.process_ack(ack_num, has_data, tcp_hdr.window(), tx);
                }
                if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data, tx)?;
                }
                if tcp_hdr.fin() {
                    self.handle_fin_recv(tx)?;
                }
            }
            TcpState::TCP_TIME_WAIT => {
                // R32-N17: the peer retransmitted its FIN (our final ACK
                // was lost) — re-ACK it, otherwise the peer exhausts its
                // FIN retransmissions and aborts the close with an RST.
                //
                // W3 note: a fresh SYN for this 4-tuple while in TIME_WAIT
                // is handled conservatively above (the R32-B12 scan resets
                // dying connections on SYN); full RFC-silent-reopen is not
                // implemented by design.
                if tcp_hdr.fin() {
                    let _ = self.send_ack(tx);
                }
            }
            TcpState::TCP_CLOSING => {
                // Simultaneous close: waiting for ACK of our FIN
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    if self.process_ack(ack_num, has_data, tcp_hdr.window(), tx) {
                        self.state = TcpState::TCP_TIME_WAIT;
                        self.start_timewait_timer();
                    }
                }
            }
            TcpState::TCP_LAST_ACK => {
                // Waiting for ACK of our FIN
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    if self.process_ack(ack_num, has_data, tcp_hdr.window(), tx) {
                        self.state = TcpState::TCP_CLOSE;
                    }
                }
            }
            TcpState::TCP_CLOSE_WAIT => {
                // Remote sent FIN, waiting for application to close
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    self.process_ack(ack_num, has_data, tcp_hdr.window(), tx);
                }
                if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data, tx)?;
                }
            }
            _ => {
                // TCP_CLOSE, TCP_TIME_WAIT etc. — ignore
            }
        }

        Ok(())
    }

    /// Handle received SYN packet (server)
    fn handle_syn_recv(&mut self, tcp_hdr: &TcpHdr, tx: &mut TcpTxBatch) -> Result<(), ()> {
        // Record client's initial sequence number. Sequence numbers are
        // true values internally — convert once at the boundary (review
        // NET-H4; the wire value leaked here and desynced rcv_nxt against
        // every later from_be comparison).
        let client_isn = TcpSeq::from_be(tcp_hdr.seq);
        // remote_ip is already set by caller before handle_packet()
        self.remote_port = TcpPort::from_be(tcp_hdr.source);

        // Initialize our sequence number from connection 4-tuple
        self.snd_nxt = generate_isn(self.local_ip, self.local_port, self.remote_ip, self.remote_port);
        self.snd_una = self.snd_nxt;
        self.rcv_nxt = client_isn.wrapping_add(1);

        // Send SYN-ACK (second step of three-way handshake)
        self.send_synack(self.rcv_nxt, tx)?;
        self.state = TcpState::TCP_SYN_RECV;

        Ok(())
    }

    /// Handle received SYN-ACK packet (client)
    fn handle_synack_recv(&mut self, tcp_hdr: &TcpHdr, tx: &mut TcpTxBatch) -> Result<(), ()> {
        // Check if ACK acknowledges our SYN
        let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
        if ack_num != self.snd_nxt.wrapping_add(1) {
            return Err(()); // ACK incorrect
        }

        // Record server's initial sequence number (wire → true value, NET-H4)
        let server_isn = TcpSeq::from_be(tcp_hdr.seq);
        self.rcv_nxt = server_isn.wrapping_add(1);

        // Update send sequence number
        self.snd_una = self.snd_nxt.wrapping_add(1);
        self.snd_nxt = self.snd_una;

        // Send ACK (third step of three-way handshake)
        self.send_ack(tx)?;
        self.state = TcpState::TCP_ESTABLISHED;
        // R32-N16: handshake complete — stop the SYN retransmit machinery
        // armed in connect().
        self.timers.stop_retransmit();
        self.timers.syn_retries = 0;

        Ok(())
    }

    /// Handle received ACK packet (server)
    fn handle_ack_recv(&mut self, tcp_hdr: &TcpHdr) -> Result<(), ()> {
        // R32-B12: the handshake-completing ACK must acknowledge our SYN
        // exactly (snd_una < ack <= snd_nxt, where snd_nxt == ISN+1 after
        // send_synack). The old code advanced snd_una on ANY ACK — a
        // stale ACK from an earlier connection sharing the 4-tuple (the
        // accept slot-reuse race) falsely completed the handshake.
        let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
        if !self.seq_before(self.snd_una, ack_num) || self.seq_before(self.snd_nxt, ack_num) {
            return Ok(()); // stale or out-of-window ACK — keep waiting
        }
        // Check if ACK acknowledges our SYN-ACK
        // Three-way handshake complete, connection established
        // R14-7: the ACK acknowledges the SYN's sequence number — advance
        // snd_una in lockstep with send_synack's snd_nxt advance.
        self.snd_una = self.snd_una.wrapping_add(1);
        self.state = TcpState::TCP_ESTABLISHED;
        Ok(())
    }

    /// Handle received data (RFC 793 §3.9 window-based acceptance)
    fn handle_data_recv(&mut self, tcp_hdr: &TcpHdr, data: &[u8], tx: &mut TcpTxBatch) -> Result<(), ()> {
        let seq = TcpSeq::from_be(tcp_hdr.seq);
        let seg_len = data.len() as u32;
        let seg_end = seq.wrapping_add(seg_len);

        // Update receive window before any checks
        self.update_rcv_wnd();

        if seg_len == 0 {
            return Ok(());
        }

        let rcv_nxt = self.rcv_nxt;
        let rcv_wnd_end = rcv_nxt.wrapping_add(self.rcv_wnd as u32);

        // Case 1: segment is entirely before the window → already received, send ACK
        if self.seq_before_or_eq(seg_end, rcv_nxt) {
            self.send_ack(tx)?;
            return Ok(());
        }

        // Case 2: segment is entirely after the window → out of window, drop
        if self.seq_after_or_eq(seq, rcv_wnd_end) {
            return Ok(());
        }

        if seq == rcv_nxt {
            // In-order segment → deliver to receive buffer. R35:
            // enqueue_data now returns Err on try_reserve failure — do NOT
            // advance rcv_nxt for data we could not buffer (acking it
            // would silently drop it; leaving rcv_nxt makes the peer
            // retransmit once memory is available).
            if self.enqueue_data(data).is_err() {
                return Ok(());
            }
            self.rcv_nxt = seg_end;

            // Drain any coalescible segments from the OOO queue
            self.drain_ooo_queue();

            // W3 (delack): in-order data arms the delayed-ACK deadline
            // (TCP_DELACK_TIMEOUT_US, 40ms) so back-to-back segments
            // coalesce into one ACK — the constant and the timer-tick arm
            // existed but nothing ever armed the deadline, so every ACK
            // was immediate. PSH (sender flushed its write) still ACKs
            // now; out-of-order/duplicate paths below keep immediate
            // dup-ACKs per RFC 5681.
            if tcp_hdr.psh() {
                self.send_ack(tx)?;
            } else if self.timers.delack_deadline == 0 {
                self.timers.delack_deadline = crate::drivers::timer::get_jiffies()
                    + TCP_DELACK_TIMEOUT_US / 10_000;
            }
        } else {
            // Out-of-order segment within window → buffer and send
            // duplicate ACK. R35: try_reserve instead of Vec::from — an
            // allocation failure under the lock used to hit
            // alloc_error_handler (panic → wfi holding TCP_TABLE_LOCK, the
            // R34 wedge); dropping an OOO segment is TCP-legal (the peer
            // retransmits after the dup-ACK).
            let mut seg_data = alloc::vec::Vec::new();
            if seg_data.try_reserve_exact(data.len()).is_err()
                || self.ooo_queue.try_reserve(1).is_err()
            {
                // Do not ACK buffered-but-dropped bytes — same rationale
                // as the in-order arm above. (The ooo_queue deque-chunk
                // growth is also an under-lock allocation — R35.)
                return Ok(());
            }
            seg_data.extend_from_slice(data);
            self.ooo_queue.push_back(TcpOooSeg {
                seq,
                data: seg_data,
            });

            // Send duplicate ACK to trigger fast retransmit on sender
            self.send_ack(tx)?;
        }

        // Update receive window after buffering
        self.update_rcv_wnd();
        Ok(())
    }

    /// Drain deliverable segments from the out-of-order queue.
    /// Called after an in-order segment fills a gap. Handles partial
    /// overlaps by trimming already-received data from the prefix.
    fn drain_ooo_queue(&mut self) {
        loop {
            // Find first segment that overlaps with rcv_nxt:
            // deliverable if seg.seq <= rcv_nxt < seg.seq + seg.data.len()
            let pos = self.ooo_queue.iter().position(|seg| {
                let seg_end = seg.seq.wrapping_add(seg.data.len() as u32);
                // seg.seq <= rcv_nxt (seg starts at or before our gap)
                let seq_ok = !TcpCongestion::seq_before(self.rcv_nxt, seg.seq);
                // rcv_nxt < seg_end (seg extends past our gap)
                let end_ok = TcpCongestion::seq_before(self.rcv_nxt, seg_end);
                seq_ok && end_ok
            });
            if let Some(idx) = pos {
                let seg = self.ooo_queue.remove(idx).unwrap();
                // Trim any already-received prefix
                let offset = self.rcv_nxt.wrapping_sub(seg.seq) as usize;
                if offset < seg.data.len() {
                    // R35: same try_reserve discipline as the in-order
                    // path — on failure stop draining (rcv_nxt stays put,
                    // the remaining OOO segs stay queued, peer retransmits).
                    if self.enqueue_data(&seg.data[offset..]).is_err() {
                        break;
                    }
                    self.rcv_nxt = self.rcv_nxt.wrapping_add((seg.data.len() - offset) as u32);
                }
            } else {
                break;
            }
        }
    }

    /// Handle received FIN packet
    fn handle_fin_recv(&mut self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        // Update receive sequence number (FIN occupies one sequence number)
        self.rcv_nxt = self.rcv_nxt.wrapping_add(1);

        // Send ACK
        self.send_ack(tx)?;

        // State transition based on current state
        match self.state {
            TcpState::TCP_ESTABLISHED => {
                self.state = TcpState::TCP_CLOSE_WAIT;
                // R32-B8: timestamp the CLOSE_WAIT entry so the timer tick
                // can bound orphaned half-closed connections.
                self.timers.close_wait_since = crate::drivers::timer::get_jiffies();
            }
            TcpState::TCP_FIN_WAIT2 => {
                self.state = TcpState::TCP_TIME_WAIT;
                self.start_timewait_timer();
            }
            TcpState::TCP_CLOSING => {
                self.state = TcpState::TCP_TIME_WAIT;
                self.start_timewait_timer();
            }
            _ => {}
        }

        Ok(())
    }

    /// Handle received RST packet (RFC 793 §3.9)
    fn handle_rst_recv(&mut self) {
        match self.state {
            TcpState::TCP_SYN_SENT => {
                // R34: RFC 793 — in SYN_SENT an RST (with an acceptable
                // ACK, e.g. slirp/host refusing the connection) aborts the
                // connect. The old code ignored it and the SYN retransmit
                // timer kept firing forever (observed as a SYN/RST ping-pong
                // against QEMU user networking).
                // W3: record ECONNREFUSED — a blocking connect()/send()
                // and getsockopt(SO_ERROR) must see WHY.
                self.pending_error = 111; // ECONNREFUSED
                self.state = TcpState::TCP_CLOSE;
                self.send_buffer.clear();
                self.recv_buffer.clear();
                self.retrans_queue.clear();
            }
            TcpState::TCP_SYN_RECV => {
                // If ACK is acceptable, abort connection
                self.pending_error = 104; // ECONNRESET
                self.state = TcpState::TCP_CLOSE;
            }
            TcpState::TCP_ESTABLISHED
            | TcpState::TCP_FIN_WAIT1
            | TcpState::TCP_FIN_WAIT2
            | TcpState::TCP_CLOSE_WAIT => {
                // Abort connection
                self.pending_error = 104; // ECONNRESET
                self.state = TcpState::TCP_CLOSE;
                // Clear buffers
                self.send_buffer.clear();
                self.recv_buffer.clear();
                self.retrans_queue.clear();
            }
            TcpState::TCP_CLOSING
            | TcpState::TCP_LAST_ACK
            | TcpState::TCP_TIME_WAIT => {
                // In these states, just close — no error (the close was
                // locally initiated)
                self.state = TcpState::TCP_CLOSE;
            }
            _ => {
                // TCP_CLOSE, TCP_LISTEN, TCP_SYN_SENT — ignore RST
            }
        }
    }

    /// Send data
    ///
    /// # Arguments
    /// - `data`: Data to send
    /// - `tx`: R35 deferred-TX batch (segments emitted after lock drop)
    pub fn send(&mut self, data: &[u8], tx: &mut TcpTxBatch) -> Result<usize, ()> {
        // Delegate to send_reliable so data goes through congestion control,
        // retransmit queue, and proper window management (fixes H38).
        self.send_reliable(data, tx)
    }

    /// Receive data
    ///
    /// # Arguments
    /// - `buf`: Buffer (kernel memory — the syscall layer copies from
    ///   user OUTSIDE the table lock, R35)
    /// - `len`: Buffer length
    /// - `tx`: R35 deferred-TX batch for the window-update ACK
    pub fn recv(&mut self, buf: &mut [u8], _len: usize, tx: &mut TcpTxBatch) -> Result<usize, ()> {
        // Allow reading in ESTABLISHED and CLOSE_WAIT (peer sent FIN but
        // data may still be buffered). Also FIN_WAIT1/FIN_WAIT2 for half-close.
        match self.state {
            TcpState::TCP_ESTABLISHED
            | TcpState::TCP_CLOSE_WAIT
            | TcpState::TCP_FIN_WAIT1
            | TcpState::TCP_FIN_WAIT2 => {}
            // R34: a VFS-connected socket sitting in TCP_CLOSE is an ABORTED
            // connection (RST in SYN_SENT, retrans exhaustion) — unconnected
            // sockets never reach here (the VFS layer returns ENOTCONN
            // first). Surfacing EOF matches the R22-4 convention ("zero
            // length read on a half/RST-closed connection is EOF");
            // previously recv spun EAGAIN forever on a dead connection.
            TcpState::TCP_CLOSE => return Ok(0),
            _ => return Err(()),
        }

        // Read data from receive buffer
        let mut read = 0;
        while read < buf.len() && !self.recv_buffer.is_empty() {
            if let Some(byte) = self.recv_buffer.pop_front() {
                buf[read] = byte;
                read += 1;
            }
        }

        // Update receive window and notify peer if space freed up
        if read > 0 {
            self.update_rcv_wnd();
            let _ = self.send_ack(tx);
        }

        Ok(read)
    }

    /// Put data into receive buffer
    ///
    /// # Arguments
    /// - `data`: Received data
    ///
    /// # Returns
    /// Err(()) when the growth reservation failed (R35): the old per-byte
    /// `push_back` (and even a single extend) reallocates under
    /// TCP_TABLE_LOCK — an OOM there hits `alloc_error_handler`, panics,
    /// and parks the CPU in `wfi` holding the table lock (the R34 wedge
    /// signature). try_reserve turns that into a clean segment drop; the
    /// caller leaves rcv_nxt unadvanced so the peer retransmits.
    pub fn enqueue_data(&mut self, data: &[u8]) -> Result<(), ()> {
        if self.recv_buffer.try_reserve(data.len()).is_err() {
            return Err(());
        }
        // Single extend within reserved capacity: one memcpy, no
        // allocation (the old loop reallocated per few bytes — up to
        // log2(len) heap ops per segment under the lock).
        self.recv_buffer.extend(data.iter().copied());
        Ok(())
    }

    /// Close connection
    pub fn close(&mut self, tx: &mut TcpTxBatch) {
        match self.state {
            TcpState::TCP_ESTABLISHED => {
                self.state = TcpState::TCP_FIN_WAIT1;
                let _ = self.send_fin(tx);
            }
            TcpState::TCP_CLOSE_WAIT => {
                self.state = TcpState::TCP_LAST_ACK;
                // R32-B8: leaving CLOSE_WAIT — disarm the orphan timeout.
                self.timers.close_wait_since = 0;
                let _ = self.send_fin(tx);
            }
            _ => {
                self.state = TcpState::TCP_CLOSE;
            }
        }
    }

    // ========== Reliable transmission methods ==========

    /// R32-N4: maximum bytes accepted into the send buffer per send() call.
    /// The buffer has no backpressure (the syscall layer cannot block), and
    /// `access_ok` alone admits user lengths up to 256GB — one huge
    /// sendto/write exhausted the 32MB kernel heap (VecDeque byte pushes
    /// until OOM panic). Accepting a prefix is POSIX-legal for stream
    /// sockets: the caller sees a partial write and retries the remainder.
    pub const TCP_SEND_MAX_CHUNK: usize = 256 * 1024;

    /// Reliable send data
    ///
    /// Puts data into send buffer and attempts to send, supports retransmission
    ///
    /// # Arguments
    /// - `data`: Data to send (kernel buffer — R35 copies user data out
    ///   at the syscall layer, before the table lock)
    /// - `tx`: R35 deferred-TX batch; segments are recorded here and
    ///   emitted after TCP_TABLE_LOCK drops
    ///
    /// # Returns
    /// Bytes sent on success, Err(()) on failure
    pub fn send_reliable(&mut self, data: &[u8], tx: &mut TcpTxBatch) -> Result<usize, ()> {
        if self.state != TcpState::TCP_ESTABLISHED {
            return Err(());
        }

        if data.is_empty() {
            return Ok(0);
        }

        // R32-N4: cap the accepted prefix (partial write semantics).
        let accept = core::cmp::min(data.len(), Self::TCP_SEND_MAX_CHUNK);

        // R35: reserve the buffer growth BEFORE the bytes go in — the old
        // per-byte push_back (and a bare extend) reallocated under
        // TCP_TABLE_LOCK, an OOM-panic point (R34 wedge class). On
        // reservation failure return Err (caller surfaces EIO); the data
        // is NOT accepted, so nothing is lost — the syscall retries.
        if self.send_buffer.try_reserve(accept).is_err() {
            return Err(());
        }
        // One extend within reserved capacity: memcpy only.
        self.send_buffer.extend(data[..accept].iter().copied());

        // Try to send data
        self.tx_packets(tx)?;

        Ok(accept)
    }

    /// Send packets (core send logic)
    ///
    /// Takes data from send buffer, builds TCP segments and sends
    /// Limited by congestion window and receive window.
    ///
    /// R35: segments are copied into the caller's pre-reserved TcpTxBatch
    /// (memcpy only) and emitted after the lock drops — the old inline
    /// tx_segment ran the virtio completion spin (10M+50M iterations per
    /// packet) under TCP_TABLE_LOCK for EVERY mss-sized chunk (up to 180
    /// per 256KB write). The retrans-queue copy is try_reserve'd FIRST so
    /// an OOM stops the loop cleanly with the send buffer intact instead
    /// of panicking under the lock.
    pub fn tx_packets(&mut self, tx: &mut TcpTxBatch) -> Result<(), ()> {
        let now = crate::drivers::timer::get_jiffies();

        // Calculate in-flight data
        let in_flight = self.snd_nxt.wrapping_sub(self.snd_una);

        // Calculate usable window: min(snd_wnd, cwnd) - in_flight
        let mut usable_window = core::cmp::min(self.snd_wnd as u32, self.congestion.cwnd)
            .saturating_sub(in_flight as u32);

        if usable_window == 0 {
            // W3: a CLOSED peer window (snd_wnd == 0, not merely cwnd-
            // limited) with queued data arms the persist timer — without
            // the zero-window probe the connection stalls forever if the
            // peer's window-reopening ACK is lost (it never retransmits
            // pure window updates).
            if self.snd_wnd == 0 && !self.send_buffer.is_empty() && self.timers.persist_deadline == 0
            {
                self.timers.persist_deadline = now + TCP_PERSIST_INTERVAL_JIFFIES;
            }
            return Ok(()); // Window full, wait
        }

        while !self.send_buffer.is_empty() && usable_window > 0 {
            // Calculate this send size
            let seg_size = core::cmp::min(
                core::cmp::min(self.mss as usize, usable_window as usize),
                self.send_buffer.len()
            );

            if seg_size == 0 {
                break;
            }

            // R35: all storage checks FIRST — a failed check breaks with
            // the send buffer and window accounting untouched (the bytes
            // drain on the next send/ACK-window open).
            if tx.desc_room() == 0 || tx.arena_room() < seg_size {
                break;
            }
            // Retrans-queue data copy (this one outlives the call — it
            // must be an owned Vec; try_reserve keeps failure graceful —
            // as does the retrans_queue's own deque-chunk growth).
            let mut rdata = alloc::vec::Vec::new();
            if rdata.try_reserve_exact(seg_size).is_err()
                || self.retrans_queue.try_reserve(1).is_err()
            {
                break;
            }

            // Pop the chunk straight into the staging arena (single
            // memcpy; reserved capacity — cannot allocate).
            let off = tx.arena_extend_drain(self.send_buffer.drain(..seg_size));
            rdata.extend_from_slice(tx.arena_slice(off, seg_size));

            // Record the segment for post-lock emission.
            if self.is_v6 {
                tx.commit6(
                    &self.local_ip6,
                    &self.remote_ip6,
                    self.local_port,
                    self.remote_port,
                    self.snd_nxt,
                    self.rcv_nxt,
                    0x0018, // PSH + ACK
                    self.rcv_wnd,
                    off,
                    seg_size,
                );
            } else {
                tx.commit(
                    self.local_ip,
                    self.remote_ip,
                    self.local_port,
                    self.remote_port,
                    self.snd_nxt,
                    self.rcv_nxt,
                    0x0018, // PSH + ACK
                    self.rcv_wnd,
                    off,
                    seg_size,
                );
            }

            // Add segment to retransmit queue
            self.retrans_queue.push_back(TcpSendSeg {
                seq: self.snd_nxt,
                len: seg_size,
                data: rdata,
                tx_time: now,
                retries: 0,
            });

            // Update sequence number
            self.snd_nxt = self.snd_nxt.wrapping_add(seg_size as u32);

            // R32-N1: consume the window as we fill it. usable_window was
            // computed once from in_flight and never decremented before, so
            // the loop kept granting the full min(snd_wnd, cwnd) to EVERY
            // iteration — the entire send_buffer went out in mss-sized
            // segments, exceeding cwnd/snd_wnd by up to (buffer/mss)x and
            // defeating congestion control.
            usable_window -= seg_size as u32;
        }

        // Start retransmit timer
        if !self.retrans_queue.is_empty() && self.timers.retransmit_deadline == 0 {
            self.start_retransmit_timer();
        }

        Ok(())
    }

    /// Record a single TCP segment for post-lock emission (R35)
    ///
    /// `seq` is the sequence number of the first byte of `data`. R32-B5:
    /// this is now a parameter instead of implicitly using snd_nxt — the
    /// retransmit paths (timer expiry, fast retransmit) re-send segments
    /// whose starting sequence is BELOW snd_nxt once later data has been
    /// transmitted; using snd_nxt there emitted a wrong byte range that
    /// the peer treated as duplicate/invalid data.
    fn tx_segment(&self, tx: &mut TcpTxBatch, seq: TcpSeq, data: &[u8]) -> Result<(), ()> {
        // R23-4: a zero-data segment in the retrans queue IS the FIN
        // (send_fin pushed it with an empty payload) — the retransmitted
        // copy must carry the FIN bit, not PSH.
        let is_fin_retrans = data.is_empty();

        if self.tx_record(
            tx,
            seq,
            self.rcv_nxt,
            if is_fin_retrans { 0x0011 } else { 0x0018 }, // FIN+ACK vs PSH+ACK
            self.rcv_wnd,
            data,
        ) {
            Ok(())
        } else {
            // Degraded (can't-happen staging overflow): skip this copy —
            // the RTO timer re-fires while the segment stays queued.
            Err(())
        }
    }

    /// Process ACK acknowledgment
    ///
    /// When ACK is received, update send window, RTT estimate, congestion control.
    ///
    /// W3: `has_data` tells whether the carrying segment had a payload
    /// (dup-ACK detection per RFC 5681 needs a PURE ack); `window` is the
    /// peer's advertised window in this segment.
    pub fn process_ack(&mut self, ack: TcpSeq, has_data: bool, window: u16, tx: &mut TcpTxBatch) -> bool {
        // Check ACK sequence number
        if self.seq_before(ack, self.snd_una) {
            // Old ACK below snd_una — STALE, not a duplicate ACK. (The old
            // code counted these as dup-ACKs and, worse, ignored the
            // ack == snd_una case entirely — so real dup-ACKs never fired
            // fast retransmit and recovery depended on RTO alone.)
            return false;
        }

        if self.seq_after(ack, self.snd_nxt) {
            // ACK exceeds sent data, ignore
            return false;
        }

        // W3: adopt the peer's window advertisement on every acceptable
        // ACK (snd_wnd used to be the TCP_MAX_WINDOW initial value forever,
        // ignoring both shrinking and reopening advertisements — a peer
        // closing its window black-holed nothing but reopening it never
        // released a stalled sender either, and large-advertising peers
        // were under-used).
        // P2 window scale: the 16-bit field carries window >> shift when
        // the peer offered WS in its SYN — reconstruct the true size
        // (saturating at the u16 storage).
        self.snd_wnd = ((window as u32) << self.peer_wscale).min(u16::MAX as u32) as u16;

        // Calculate acknowledged bytes
        let acked_bytes = ack.wrapping_sub(self.snd_una);

        if acked_bytes > 0 {
            // New ACK
            // 1. Remove acknowledged segments, capture tx_time of last acked seg
            //    (Karn-filtered in remove_acked_segments — retransmitted
            //    segments are never RTT samples, W3)
            let ack_tx_time = self.remove_acked_segments(ack);

            // 2. Update snd_una
            self.snd_una = ack;

            // 3. Update RTT estimate from the acknowledged segment's tx_time (fixes H37)
            if let Some(tx_time) = ack_tx_time {
                self.update_rtt(tx_time);
            }

            // 4. Congestion control: received new ACK
            self.congestion.on_ack(acked_bytes, self.mss);
            self.congestion.dup_ack_count = 0;

            // 5. Reset or stop retransmit timer
            if !self.retrans_queue.is_empty() {
                self.start_retransmit_timer();
            } else {
                self.timers.stop_retransmit();
            }

            // W3: the window may have reopened — flush what was stalled in
            // the send buffer and disarm the zero-window probe.
            if self.snd_wnd > 0 {
                self.timers.persist_deadline = 0;
            }
            if !self.send_buffer.is_empty() {
                let _ = self.tx_packets(tx);
            }

            // P2 SO_KEEPALIVE: a fresh ACK proves the peer is alive —
            // reset the probe cycle; the timer tick re-arms a full idle
            // window (keepidle) on the next quiet pass.
            if self.keepalive {
                self.timers.keepalive_probes = 0;
                self.timers.keepalive_deadline = 0;
            }
        } else if !has_data {
            // W3: ack == snd_una on a pure ACK = duplicate ACK (RFC 5681)
            // — typically triggered by the receiver buffering an
            // out-of-order segment. Count it; three in a row fire fast
            // retransmit (the dead path before this fix).
            self.congestion.on_dup_ack(ack, self.snd_nxt, self.mss);
            if self.congestion.dup_ack_count >= 3 {
                self.fast_retransmit(tx);
            }
        }
        true
    }

    /// Remove acknowledged segments from retransmit queue.
    /// Returns the `tx_time` of the last fully-acknowledged segment (for RTT sampling).
    ///
    /// W3 (Karn's algorithm): segments that were retransmitted at least
    /// once carry ambiguous RTT signal — their acks are never sampled.
    fn remove_acked_segments(&mut self, ack: TcpSeq) -> Option<u64> {
        let mut last_tx_time: Option<u64> = None;
        // Drain segments that are fully covered by the ACK.
        while let Some(seg) = self.retrans_queue.front() {
            let seg_end = seg.seq.wrapping_add(seg.len as u32);
            // Sequence comparison: seg_end before (or equal to) ack
            if ((seg_end as i32) - (ack as i32)) <= 0 {
                if seg.retries == 0 {
                    last_tx_time = Some(seg.tx_time); // Karn: only never-retransmitted
                }
                self.retrans_queue.pop_front();
            } else {
                break;
            }
        }
        last_tx_time
    }

    /// Update RTT estimate from the transmission time of the acknowledged segment.
    fn update_rtt(&mut self, tx_time: u64) {
        let now = crate::drivers::timer::get_jiffies();
        // Jiffies to microseconds (1 jiffy = 10ms = 10_000us)
        let rtt_us = now.saturating_sub(tx_time) * 10_000;
        if rtt_us > 0 {
            self.rtt_estimator.update(rtt_us);
        }
    }

    /// Fast retransmit
    ///
    /// R35: records the retransmission into `tx` (memcpy into the
    /// pre-reserved arena — the old `tx_segment` inline send, and the
    /// retransmit-timer path's `seg.data.clone()`, both allocated and
    /// spun on virtio completion under TCP_TABLE_LOCK).
    fn fast_retransmit(&mut self, tx: &mut TcpTxBatch) {
        if let Some(seg) = self.retrans_queue.front() {
            // Retransmit earliest segment — with ITS starting sequence
            // number, not snd_nxt (R32-B5).
            let _ = self.tx_segment(tx, seg.seq, &seg.data);
        }
    }

    /// W3: zero-window persist probe (RFC 793 persist state, minimal).
    ///
    /// Sends a single byte past the zero window so the peer is forced to
    /// answer (its ACK carries the current window). Prefers re-probing
    /// the oldest unacknowledged byte; if nothing is in flight yet,
    /// transmits one fresh byte from the send buffer.
    pub fn send_zero_window_probe(&mut self, tx: &mut TcpTxBatch) {
        if let Some(seg) = self.retrans_queue.front() {
            if !seg.data.is_empty() {
                let _ = self.tx_record(
                    tx,
                    seg.seq,
                    self.rcv_nxt,
                    0x0010, // ACK
                    self.rcv_wnd,
                    &seg.data[..1],
                );
                return;
            }
        }
        if self.send_buffer.is_empty() {
            self.timers.persist_deadline = 0; // nothing to probe for
            return;
        }
        // Fresh 1-byte probe: stage into the retrans queue first (R35
        // try_reserve discipline — no allocation can fail under the lock
        // into a panic).
        let mut data = alloc::vec::Vec::new();
        if data.try_reserve_exact(1).is_err() || self.retrans_queue.try_reserve(1).is_err() {
            return; // retry at the next persist tick
        }
        let byte = self.send_buffer.pop_front();
        if let Some(b) = byte {
            data.push(b);
            self.tx_record(
                tx,
                self.snd_nxt,
                self.rcv_nxt,
                0x0018, // PSH + ACK
                self.rcv_wnd,
                &data,
            );
            self.retrans_queue.push_back(TcpSendSeg {
                seq: self.snd_nxt,
                len: 1,
                data,
                tx_time: crate::drivers::timer::get_jiffies(),
                retries: 0,
            });
            self.snd_nxt = self.snd_nxt.wrapping_add(1);
        }
    }

    /// P2 SO_KEEPALIVE: one keepalive probe (RFC 1122 §4.2.3.6, minimal) —
    /// an empty ACK carrying sequence `snd_una - 1`, i.e. one byte BELOW
    /// the lowest unacknowledged byte. A live peer cannot accept it and
    /// answers with an ACK, which resets the probe cycle in process_ack.
    pub fn send_keepalive_probe(&self, tx: &mut TcpTxBatch) {
        let probe_seq = self.snd_una.wrapping_sub(1);
        let _ = self.tx_record_ctl(
            tx,
            probe_seq,
            self.rcv_nxt,
            0x0010, // ACK, no payload
            self.rcv_wnd,
        );
    }

    /// Start retransmit timer
    fn start_retransmit_timer(&mut self) {
        self.timers.start_retransmit(self.rtt_estimator.rto);
    }

    /// Retransmit timer expired handling
    ///
    /// Called by TCP timer tick. R35: the retransmitted segment is copied
    /// into `tx` (arena memcpy — zero allocation) instead of `seg.data
    /// .clone()` (heap) + inline virtio spin, both of which used to run
    /// under TCP_TABLE_LOCK. Mutation order (retries++ / backoff / timer
    /// restart) is unchanged so the six-state timer semantics and
    /// bounded-retry lifetime are preserved bit-for-bit; only the wire
    /// emission is deferred past the lock.
    pub fn retransmit_timer_expired(&mut self, tx: &mut TcpTxBatch) {
        // Check retransmit queue
        if self.retrans_queue.is_empty() {
            self.timers.stop_retransmit();
            return;
        }

        // First get needed info to avoid borrow conflicts
        let should_close;

        // Copy the endpoint scalars out of self before the &mut borrow of
        // the retrans queue (P1: family-aware recording needs them).
        let (ep_v6, ep_lip, ep_rip, ep_lip6, ep_rip6, ep_lport, ep_rport, ep_rcv_nxt, ep_rcv_wnd) =
            (
                self.is_v6,
                self.local_ip,
                self.remote_ip,
                self.local_ip6,
                self.remote_ip6,
                self.local_port,
                self.remote_port,
                self.rcv_nxt,
                self.rcv_wnd,
            );

        {
            if let Some(seg) = self.retrans_queue.front_mut() {
                if seg.retries >= TCP_MAX_RETRIES {
                    // Exceeded maximum retransmit count, close connection
                    should_close = true;
                } else {
                    should_close = false;
                    // Record the retransmission into the deferred batch
                    // (R32-B5: the segment's OWN seq, not snd_nxt — the
                    // window may have advanced past it since the original
                    // transmission). Failure (can't-happen staging
                    // overflow) skips only the emission; retries/backoff
                    // below still run, keeping the bounded lifetime.
                    let _ = tx_record_endpoints(
                        ep_v6,
                        ep_lip,
                        ep_rip,
                        ep_lip6,
                        ep_rip6,
                        ep_lport,
                        ep_rport,
                        tx,
                        seg.seq,
                        ep_rcv_nxt,
                        if seg.data.is_empty() { 0x0011 } else { 0x0018 }, // R23-4 FIN vs PSH
                        ep_rcv_wnd,
                        &seg.data,
                    );
                    // Increment retransmit count
                    seg.retries += 1;
                }
            } else {
                return;
            }
        }

        if should_close {
            // W3: record ETIMEDOUT — a blocked writer / SO_ERROR reader
            // must see why the connection died.
            self.pending_error = 110; // ETIMEDOUT
            self.state = TcpState::TCP_CLOSE;
            self.timers.stop_retransmit();
            return;
        }

        // Congestion control: timeout handling
        self.congestion.on_timeout(self.mss);

        // RTO exponential backoff
        self.rtt_estimator.backoff();

        // Reset timer
        self.start_retransmit_timer();
    }

    /// Sequence number comparison: a before b (considering wraparound)
    #[inline]
    fn seq_before(&self, a: TcpSeq, b: TcpSeq) -> bool {
        ((a as i32) - (b as i32)) < 0
    }

    /// Sequence number comparison: a after b (considering wraparound)
    #[inline]
    fn seq_after(&self, a: TcpSeq, b: TcpSeq) -> bool {
        self.seq_before(b, a)
    }

    /// Sequence number comparison: a before or equal to b
    #[inline]
    fn seq_before_or_eq(&self, a: TcpSeq, b: TcpSeq) -> bool {
        !self.seq_after(a, b)
    }

    /// Sequence number comparison: a after or equal to b
    #[inline]
    fn seq_after_or_eq(&self, a: TcpSeq, b: TcpSeq) -> bool {
        !self.seq_before(a, b)
    }

    /// Update receive window
    pub fn update_rcv_wnd(&mut self) {
        // Receive window = buffer size - used space.
        // R32-B11: compute in u32 and clamp BEFORE the u16 narrowing —
        // `recv_buffer.len() as u16` truncated modulo 65536, so a backlog
        // of exactly 64KB+ advertised a window of TCP_MAX_WINDOW (full)
        // while the buffer was actually overflowing, telling the peer to
        // send even more.
        let used = (self.recv_buffer.len() as u32).min(TCP_MAX_WINDOW as u32);
        self.rcv_wnd = TCP_MAX_WINDOW.saturating_sub(used as u16);
    }
}

/// TCP connection manager
///
/// Manages all TCP connections, handles received TCP packets
pub struct TcpConnectionManager {
    /// Listening sockets
    listen_sockets: alloc::vec::Vec<TcpSocket>,
    /// Established connections
    established_connections: alloc::vec::Vec<TcpSocket>,
    /// Pending connection queue (for accept)
    pending_connections: alloc::vec::Vec<TcpSocket>,
}

/// W3: which protocol slots a dispatched segment affected — tcp_rcv wakes
/// the corresponding VFS wait queues after dropping the table lock
/// (blocking recv/accept sleepers).
#[derive(Debug, Clone, Copy)]
pub struct TcpRvWake {
    /// Data/established socket's protocol-table index
    pub socket: i32,
    /// Listener whose accept queue gained a child (SYN spawn), if any
    pub parent: Option<i32>,
}

impl TcpConnectionManager {
    pub fn new() -> Self {
        Self {
            listen_sockets: alloc::vec::Vec::new(),
            established_connections: alloc::vec::Vec::new(),
            pending_connections: alloc::vec::Vec::new(),
        }
    }

    /// Add listening socket
    pub fn add_listen_socket(&mut self, socket: TcpSocket) {
        self.listen_sockets.push(socket);
    }

    /// Handle received TCP packet.
    ///
    /// Single source of truth (review NET-C4): the SOCKET TABLE owns every
    /// connection — client (connect) and server (SYN-spawned) sockets alike.
    /// Lookup order:
    ///   (a) exact 4-tuple match anywhere in the table (skip LISTEN sockets);
    ///   (b) a SYN for a port with a LISTEN socket spawns a new table slot
    ///       whose state starts at LISTEN, then runs the state machine once
    ///       (LISTEN + SYN → handle_syn_recv → SYN-ACK, no pre-set state);
    ///   (c) no match → Err (caller sends RST).
    /// The manager-side pending/established lists are no longer part of the
    /// data path; the sockets stay in their table slots after the handshake,
    /// so RX lookup, timers and accept() all share one view.
    ///
    /// R35: `tx` collects every segment this packet triggers; the caller
    /// (tcp_rcv) emits them after dropping TCP_TABLE_LOCK — no virtio TX
    /// spin under the table lock (chain 2).
    ///
    /// W3: returns the affected slots so tcp_rcv can wake blocking
    /// recv()/accept() waiters (None = parsed but nothing to wake).
    pub fn handle_tcp_packet(
        &mut self,
        skb: &SkBuff,
        src_ip: u32,
        dest_ip: u32,
        tx: &mut TcpTxBatch,
    ) -> Result<Option<TcpRvWake>, ()> {
        // Parse TCP header
        let tcp_hdr = match tcp_parse_packet(skb) {
            Some(hdr) => hdr,
            None => return Ok(None),
        };

        let src_port = TcpPort::from_be(tcp_hdr.source);
        let dest_port = TcpPort::from_be(tcp_hdr.dest);

        let is_syn = tcp_hdr.syn() && !tcp_hdr.ack();
        let mut listen_parent: Option<i32> = None;

        // Pass (a) + (b) listener detection in a single table scan.
        // SAFETY: single-core softirq/syscall serialization for the global
        // TCP socket table (full locking is review NET-M15).
        unsafe {
            let table = &mut TCP_SOCKET_TABLE;
            for fd in 0..table.count {
                let socket = match table.sockets[fd].as_mut() {
                    Some(s) => s,
                    None => continue,
                };
                if socket.state == TcpState::TCP_LISTEN {
                    if socket.local_port == dest_port {
                        listen_parent = Some(fd as i32);
                    }
                    continue;
                }
                // R32-B12: CLOSE corpses (pending sweep reaping) must not
                // swallow packets — a 4-tuple match on them ran the state
                // machine's dead arm and returned Ok, so neither a fresh
                // child (pass b) nor an RST could ever be produced for a
                // reused slot. Skip them: the packet falls through to the
                // listener spawn below, or to the RST path.
                if socket.state == TcpState::TCP_CLOSE {
                    continue;
                }
                if socket.local_port == dest_port
                    && socket.remote_port == src_port
                    && socket.remote_ip == src_ip
                    && (socket.local_ip == dest_ip || socket.local_ip == 0)
                {
                    // local_ip == 0 (INADDR_ANY) matches any destination.
                    // Normalize it so later comparisons are exact.
                    if socket.local_ip == 0 {
                        socket.local_ip = dest_ip;
                    }
                    // R32-B12 (accept slot-reuse race): a pure SYN for a
                    // tuple held by a DYING connection is a new incarnation
                    // (peer rebooted / port reuse): the old connection can
                    // never use it. Reset the old socket and keep scanning
                    // so this same SYN can spawn a fresh child under the
                    // listener (pass b). The old slot is reaped by the
                    // timer sweep once it reaches CLOSE with user_refs==0.
                    if is_syn
                        && matches!(
                            socket.state,
                            TcpState::TCP_FIN_WAIT1
                                | TcpState::TCP_FIN_WAIT2
                                | TcpState::TCP_CLOSING
                                | TcpState::TCP_LAST_ACK
                                | TcpState::TCP_TIME_WAIT
                                | TcpState::TCP_CLOSE_WAIT
                        )
                    {
                        socket.state = TcpState::TCP_CLOSE;
                        socket.send_buffer.clear();
                        socket.recv_buffer.clear();
                        socket.retrans_queue.clear();
                        socket.ooo_queue.clear();
                        socket.timers.stop_retransmit();
                        continue;
                    }
                    let payload = match tcp_payload_slice(skb, tcp_hdr.header_len()) {
                        Some(p) => p,
                        None => return Ok(None),
                    };
                    let _ = socket.handle_packet(tcp_hdr, payload, tx);
                    return Ok(Some(TcpRvWake { socket: fd as i32, parent: None }));
                }
            }

            // Pass (b): inbound SYN for a listening port — spawn a child
            // connection in its own table slot.
            if is_syn {
                if let Some(parent) = listen_parent {
                    // Backlog cap: bound the number of not-yet-accepted
                    // children per listener (review NET-M8).
                    let mut children = 0usize;
                    for slot in table.sockets.iter().take(table.count) {
                        if let Some(s) = slot.as_ref() {
                            if s.parent_fd == Some(parent) && !s.accepted {
                                children += 1;
                            }
                        }
                    }
                    if children >= MAX_BACKLOG_PER_LISTEN {
                        return Ok(None); // drop the SYN
                    }

                    let slot = match table.alloc_slot() {
                        Ok(s) => s,
                        Err(_) => return Ok(None),
                    };
                    let mut new_socket = TcpSocket::new();
                    new_socket.local_port = dest_port;
                    new_socket.local_ip = dest_ip;
                    new_socket.remote_port = src_port;
                    new_socket.remote_ip = src_ip;
                    new_socket.state = TcpState::TCP_LISTEN;
                    new_socket.parent_fd = Some(parent);
                    // State machine drives itself from LISTEN: SYN →
                    // handle_syn_recv → sends SYN-ACK → SYN_RECV.
                    let _ = new_socket.handle_packet(tcp_hdr, &[], tx);
                    let _ = table.install(slot, new_socket);
                    return Ok(Some(TcpRvWake { socket: slot as i32, parent: Some(parent) }));
                }
            }
        }

        // No matching connection found
        Err(())
    }

    /// P1 IPv6: handle a received v6 TCP segment. Mirrors
    /// handle_tcp_packet's two-pass scan with v6 4-tuple matching; the
    /// child-spawn path copies the v6 endpoints and family bit.
    pub fn handle_tcp_packet6(
        &mut self,
        skb: &SkBuff,
        src6: &crate::net::ipv6::Ipv6Addr,
        dest6: &crate::net::ipv6::Ipv6Addr,
        tx: &mut TcpTxBatch,
    ) -> Result<Option<TcpRvWake>, ()> {
        let tcp_hdr = match tcp_parse_packet(skb) {
            Some(hdr) => hdr,
            None => return Ok(None),
        };

        let src_port = TcpPort::from_be(tcp_hdr.source);
        let dest_port = TcpPort::from_be(tcp_hdr.dest);

        let is_syn = tcp_hdr.syn() && !tcp_hdr.ack();
        let mut listen_parent: Option<i32> = None;

        // SAFETY: TCP_SOCKET_TABLE mutations run under TCP_TABLE_LOCK
        // (taken by tcp_rcv6 around this call).
        unsafe {
            let table = &mut TCP_SOCKET_TABLE;
            for fd in 0..table.count {
                let socket = match table.sockets[fd].as_mut() {
                    Some(s) => s,
                    None => continue,
                };
                if socket.state == TcpState::TCP_LISTEN {
                    if socket.local_port == dest_port && socket.is_v6 {
                        listen_parent = Some(fd as i32);
                    }
                    continue;
                }
                if socket.state == TcpState::TCP_CLOSE {
                    continue;
                }
                if socket.is_v6
                    && socket.local_port == dest_port
                    && socket.remote_port == src_port
                    && socket.remote_ip6 == *src6
                    && (socket.local_ip6 == *dest6
                        || crate::net::ipv6::is_unspecified(&socket.local_ip6))
                {
                    if crate::net::ipv6::is_unspecified(&socket.local_ip6) {
                        socket.local_ip6 = *dest6;
                    }
                    if is_syn
                        && matches!(
                            socket.state,
                            TcpState::TCP_FIN_WAIT1
                                | TcpState::TCP_FIN_WAIT2
                                | TcpState::TCP_CLOSING
                                | TcpState::TCP_LAST_ACK
                                | TcpState::TCP_TIME_WAIT
                                | TcpState::TCP_CLOSE_WAIT
                        )
                    {
                        socket.state = TcpState::TCP_CLOSE;
                        socket.send_buffer.clear();
                        socket.recv_buffer.clear();
                        socket.retrans_queue.clear();
                        socket.ooo_queue.clear();
                        socket.timers.stop_retransmit();
                        continue;
                    }
                    let payload = match tcp_payload_slice(skb, tcp_hdr.header_len()) {
                        Some(p) => p,
                        None => return Ok(None),
                    };
                    let _ = socket.handle_packet(tcp_hdr, payload, tx);
                    return Ok(Some(TcpRvWake { socket: fd as i32, parent: None }));
                }
            }

            // Pass (b): inbound SYN for a v6 listener — spawn a v6 child.
            if is_syn {
                if let Some(parent) = listen_parent {
                    let mut children = 0usize;
                    for slot in table.sockets.iter().take(table.count) {
                        if let Some(s) = slot.as_ref() {
                            if s.parent_fd == Some(parent) && !s.accepted {
                                children += 1;
                            }
                        }
                    }
                    if children >= MAX_BACKLOG_PER_LISTEN {
                        return Ok(None);
                    }

                    let slot = match table.alloc_slot() {
                        Ok(s) => s,
                        Err(_) => return Ok(None),
                    };
                    let mut new_socket = TcpSocket::new();
                    new_socket.is_v6 = true;
                    new_socket.local_port = dest_port;
                    new_socket.local_ip6 = *dest6;
                    new_socket.remote_port = src_port;
                    new_socket.remote_ip6 = *src6;
                    new_socket.state = TcpState::TCP_LISTEN;
                    new_socket.parent_fd = Some(parent);
                    let _ = new_socket.handle_packet(tcp_hdr, &[], tx);
                    let _ = table.install(slot, new_socket);
                    return Ok(Some(TcpRvWake { socket: slot as i32, parent: Some(parent) }));
                }
            }
        }

        Err(())
    }
}

/// Maximum not-yet-accepted children per listening socket.
const MAX_BACKLOG_PER_LISTEN: usize = 64;

/// Global TCP connection manager
static mut TCP_CONNECTION_MANAGER: core::mem::MaybeUninit<TcpConnectionManager> = core::mem::MaybeUninit::<TcpConnectionManager>::uninit();

/// Guards against double-init and use-before-init.
static TCP_MANAGER_INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Initialize TCP connection manager
pub fn init_tcp_manager() {
    // Deduplicate: if another CPU (or a bug) calls us again, skip.
    if TCP_MANAGER_INITIALIZED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    // SAFETY: First and only write thanks to the AtomicBool guard above.
    unsafe {
        TCP_CONNECTION_MANAGER.write(TcpConnectionManager::new());
    }
}

/// Get TCP connection manager
pub fn get_tcp_manager() -> &'static mut TcpConnectionManager {
    if !TCP_MANAGER_INITIALIZED.load(core::sync::atomic::Ordering::Acquire) {
        panic!("TCP connection manager used before init_tcp_manager()");
    }
    // SAFETY: init_tcp_manager() has completed (verified by the AtomicBool above).
    unsafe { TCP_CONNECTION_MANAGER.assume_init_mut() }
}

/// Global TCP socket table
pub struct TcpSocketTable {
    sockets: [Option<TcpSocket>; TCP_SOCKET_TABLE_SIZE],
    count: usize,
}

impl TcpSocketTable {
    const fn new() -> Self {
        const NONE: Option<TcpSocket> = None;
        Self {
            sockets: [NONE; TCP_SOCKET_TABLE_SIZE],
            count: 0,
        }
    }

    /// Allocate socket
    fn alloc(&mut self) -> Result<usize, ()> {
        // Reuse freed slots first
        for i in 0..self.count {
            if self.sockets[i].is_none() {
                self.sockets[i] = Some(TcpSocket::new());
                return Ok(i);
            }
        }
        // No freed slots; append if room
        if self.count >= TCP_SOCKET_TABLE_SIZE {
            return Err(());
        }
        let fd = self.count;
        self.sockets[fd] = Some(TcpSocket::new());
        self.count += 1;
        Ok(fd)
    }

    /// Allocate socket slot (uninitialized)
    fn alloc_slot(&mut self) -> Result<usize, ()> {
        // Reuse freed slots first
        for i in 0..self.count {
            if self.sockets[i].is_none() {
                return Ok(i);
            }
        }
        if self.count >= TCP_SOCKET_TABLE_SIZE {
            return Err(());
        }
        let fd = self.count;
        self.count += 1;
        Ok(fd)
    }

    /// Install socket to specified slot
    fn install(&mut self, fd: usize, socket: TcpSocket) -> Result<(), ()> {
        if fd >= TCP_SOCKET_TABLE_SIZE {
            return Err(());
        }

        if fd >= self.count {
            self.count = fd + 1;
        }

        self.sockets[fd] = Some(socket);
        Ok(())
    }

    /// Free socket (public for timer cleanup)
    pub fn free(&mut self, fd: usize) {
        if fd < self.count {
            self.sockets[fd] = None;
        }
    }

    /// Get socket
    fn get(&self, fd: usize) -> Option<&TcpSocket> {
        if fd < self.count {
            self.sockets[fd].as_ref()
        } else {
            None
        }
    }

    /// Get mutable socket
    fn get_mut(&mut self, fd: usize) -> Option<&mut TcpSocket> {
        if fd < self.count {
            self.sockets[fd].as_mut()
        } else {
            None
        }
    }

    /// Get mutable reference to all sockets (for timers)
    pub fn sockets_mut(&mut self) -> &mut [Option<TcpSocket>; TCP_SOCKET_TABLE_SIZE] {
        &mut self.sockets
    }

    /// Number of allocated slots (high-water mark; slots may be None).
    /// R35: read WITHOUT the table lock by the timer tick to size its
    /// deferred-TX staging before locking — a stale read only
    /// under-estimates capacity, which degrades to per-socket emission
    /// deferral (next tick), never to an under-lock allocation.
    pub fn count(&self) -> usize {
        self.count
    }
}

/// Global TCP socket table
static mut TCP_SOCKET_TABLE: TcpSocketTable = TcpSocketTable::new();

/// R21-N1: coarse table lock — the table is mutated concurrently from
/// syscalls (accept/send/recv/close) and the Timer/NetRx softirqs on a
/// 4-CPU kernel (the old 'single-core' comments were false). irqsave
/// because the softirq side can run inline at irq_exit.
pub static TCP_TABLE_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

/// Allocate TCP socket
///
/// # Returns
/// Socket file descriptor
pub fn tcp_socket_alloc() -> Result<i32, i32> {
    // SAFETY: TCP_SOCKET_TABLE is a global static; no concurrent mutation hazard
    // in current single-core kernel context.
    unsafe {
        let _g = TCP_TABLE_LOCK.lock_irqsave();
        match TCP_SOCKET_TABLE.alloc() {
            Ok(fd) => Ok(fd as i32),
            // W3: protocol table exhausted → EMFILE (Linux behavior), not EIO
            Err(_) => Err(-24), // EMFILE
        }
    }
}

/// Free TCP socket
///
/// # Arguments
/// - `fd`: Socket file descriptor
pub fn tcp_socket_free(fd: i32) {
    // SAFETY: TCP_SOCKET_TABLE is a global static; fd was returned by tcp_socket_alloc.
    unsafe {
        let _g = TCP_TABLE_LOCK.lock_irqsave();
        TCP_SOCKET_TABLE.free(fd as usize);
    }
}

/// Get mutable reference to TCP socket table (for timers)
///
/// # Safety
/// This function returns mutable reference to global socket table, caller must ensure synchronization
pub fn get_tcp_socket_table() -> &'static mut TcpSocketTable {
    // SAFETY: Caller must ensure no other mutable reference exists (timer-only use).
    unsafe { &mut TCP_SOCKET_TABLE }
}

/// Get TCP socket
///
/// # Arguments
/// - `fd`: Socket file descriptor
///
/// # Returns
/// Socket reference
pub fn tcp_socket_get(fd: i32) -> Option<&'static mut TcpSocket> {
    // SAFETY: TCP_SOCKET_TABLE is a global; caller ensures no concurrent access.
    unsafe {
        TCP_SOCKET_TABLE.get_mut(fd as usize)
    }
}

/// Bind socket to port
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `port`: Port number
///
/// # Returns
/// 0 on success, error code on failure
pub fn tcp_bind(fd: i32, port: TcpPort) -> i32 {
    let _table_g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global static; fd was returned by tcp_socket_alloc.
    unsafe {
        // R32-N9: reject a port already held by another live socket. The
        // old code accepted every bind, so two listeners (or a listener
        // and a connecting client) could share a port; RX then delivered
        // to whichever slot the scan found first. Port 0 means "assign
        // now" (ephemeral, W3) and never conflicts.
        let effective_port = if port == 0 {
            // W3: bind(0) assigns the ephemeral port IMMEDIATELY (Linux
            // semantics — getsockname must report it right after bind),
            // instead of deferring to connect().
            match alloc_ephemeral_port_checked() {
                Some(p) => p,
                None => return -99, // EADDRNOTAVAIL — ephemeral range exhausted
            }
        } else {
            if let Some(conflict_fd) = tcp_find_port_conflict(port, fd) {
                // W3 (SO_REUSEADDR, Linux semantics): both binders opting
                // in may coexist on the same port (server restart pattern;
                // this stack's binds are wildcard — the exact/wildcard
                // distinction needs per-bind address tracking, listed as a
                // leftover). One-sided or missing opt-in is still EADDRINUSE.
                // SAFETY: caller holds TCP_TABLE_LOCK; index verified by the scan.
                let both_reuse = TCP_SOCKET_TABLE.sockets[conflict_fd as usize]
                    .as_ref()
                    .map(|s| s.reuseaddr)
                    .unwrap_or(false)
                    && TCP_SOCKET_TABLE
                        .get(fd as usize)
                        .map(|s| s.reuseaddr)
                        .unwrap_or(false);
                if !both_reuse {
                    return -98; // EADDRINUSE
                }
            }
            port
        };
        if let Some(socket) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            match socket.bind(effective_port) {
                Ok(()) => 0,
                Err(()) => -5, // EIO
            }
        } else {
            -5 // EBADF
        }
    }
}

/// R32-N9 helper: another live (non-CLOSE) socket already bound to `port`?
/// Excludes `self_fd`. Returns the conflicting slot's index.
fn tcp_find_port_conflict(port: TcpPort, self_fd: i32) -> Option<i32> {
    // (caller holds TCP_TABLE_LOCK)
    // SAFETY: single-core serialization of the global table (review NET-M15).
    unsafe {
        for i in 0..TCP_SOCKET_TABLE.count {
            if i == self_fd as usize {
                continue;
            }
            if let Some(s) = TCP_SOCKET_TABLE.sockets[i].as_ref() {
                if s.local_port == port && s.state != TcpState::TCP_CLOSE {
                    return Some(i as i32);
                }
            }
        }
    }
    None
}

/// Listen on port
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `backlog`: Wait queue length
///
/// # Returns
/// 0 on success, error code on failure
pub fn tcp_listen(fd: i32, backlog: u32) -> i32 {
    let _table_g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global static; fd was returned by tcp_socket_alloc.
    unsafe {
        if let Some(socket) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            match socket.listen(backlog) {
                Ok(()) => 0,
                Err(()) => -5, // EIO
            }
        } else {
            -5 // EBADF
        }
    }
}

/// Next ephemeral port for auto-bind on connect (review NET-M6).
static NEXT_EPHEMERAL_PORT: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(32768);
const EPHEMERAL_PORT_MAX: u16 = 60999;

/// Allocate an unused local port in the ephemeral range.
fn alloc_ephemeral_port() -> TcpPort {
    alloc_ephemeral_port_checked().unwrap_or(0)
}

/// W3: Option-returning variant — the ephemeral range can be exhausted
/// (every port held by a live socket); callers translate None into an
/// errno instead of silently binding port 0.
fn alloc_ephemeral_port_checked() -> Option<TcpPort> {
    // (caller tcp_connect/tcp_bind already holds TCP_TABLE_LOCK — no nested take)
    // SAFETY: single-core serialization of the global table (review NET-M15).
    unsafe {
        for _ in 0..(EPHEMERAL_PORT_MAX - 32768 + 1) {
            let port = NEXT_EPHEMERAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let port = if port > EPHEMERAL_PORT_MAX { port % EPHEMERAL_PORT_MAX + 1024 } else { port };
            let in_use = (0..TCP_SOCKET_TABLE.count).any(|i| {
                TCP_SOCKET_TABLE
                    .sockets
                    .get(i)
                    .and_then(|s| s.as_ref())
                    .map(|s| s.local_port == port && s.state != TcpState::TCP_CLOSE)
                    .unwrap_or(false)
            });
            if !in_use {
                return Some(port);
            }
        }
        None
    }
}

/// Connect to remote address
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `ip`: IP address
/// - `port`: Port number
///
/// # Returns
/// 0 on success, error code on failure
pub fn tcp_connect(fd: i32, ip: u32, port: TcpPort) -> i32 {
    // R35: stage the SYN OUTSIDE the table lock — connect() used to run
    // send_syn → ipv4_send_src → virtio xmit (10M-iteration completion
    // spin, seconds under tcg) while holding TCP_TABLE_LOCK, serializing
    // every CPU's networking behind one connect().
    let mut tx = TcpTxBatch::new();
    if !tx.reserve(1, 0) {
        return -12; // ENOMEM — clean failure, lock never taken
    }
    let ret;
    {
        let _table_g = TCP_TABLE_LOCK.lock_irqsave();
        // SAFETY: TCP_SOCKET_TABLE is a global static; fd was returned by tcp_socket_alloc.
        unsafe {
            if let Some(socket) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
                // Auto-bind an ephemeral local port when the caller never bound
                // (old code sent SYN with source port 0, review NET-M6).
                if socket.local_port == 0 {
                    socket.local_port = alloc_ephemeral_port();
                    socket.bound = true;
                }
                // Source address for the connection: loopback for loopback
                // destinations, the device address otherwise. Without this the
                // socket keeps local_ip = 0 (ANY) and inbound SYN-ACKs fail the
                // 4-tuple lookup below (found via the nettest E2E run).
                if socket.local_ip == 0 {
                    socket.local_ip = if (ip >> 24) == 127 {
                        0x7F000001
                    } else {
                        crate::net::arp::get_local_ip()
                    };
                }
                ret = match socket.connect(ip, port, &mut tx) {
                    Ok(()) => 0,
                    Err(()) => -5, // EIO
                };
            } else {
                ret = -5; // EBADF
            }
        }
    }
    tx.emit_all();
    ret
}

/// P1 IPv6: connect to a pure v6 remote (family-aware tcp_connect).
pub fn tcp_connect6(fd: i32, ip6: &crate::net::ipv6::Ipv6Addr, port: TcpPort) -> i32 {
    let mut tx = TcpTxBatch::new();
    if !tx.reserve(1, 0) {
        return -12; // ENOMEM
    }
    let ret;
    {
        let _table_g = TCP_TABLE_LOCK.lock_irqsave();
        // SAFETY: TCP_SOCKET_TABLE is a global static; fd was returned by tcp_socket_alloc.
        unsafe {
            if let Some(socket) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
                if socket.local_port == 0 {
                    socket.local_port = alloc_ephemeral_port();
                    socket.bound = true;
                }
                // v6 source: loopback for ::1, our SLAAC link-local
                // otherwise (the v4 analogue of the ANY-local_ip trap).
                if crate::net::ipv6::is_unspecified(&socket.local_ip6) {
                    socket.local_ip6 = if *ip6 == crate::net::ipv6::IPV6_ADDR_LOOPBACK {
                        crate::net::ipv6::IPV6_ADDR_LOOPBACK
                    } else {
                        match crate::net::ipv6::get_link_local() {
                            Some(ll) => ll,
                            None => return -99, // EADDRNOTAVAIL — no v6 source
                        }
                    };
                }
                ret = match socket.connect6(ip6, port, &mut tx) {
                    Ok(()) => 0,
                    Err(()) => -5, // EIO
                };
            } else {
                ret = -5; // EBADF
            }
        }
    }
    tx.emit_all();
    ret
}

/// Accept connection
///
/// # Arguments
/// - `fd`: Listening socket index (TCP protocol table)
///
/// # Returns
/// The protocol-table index of an established child connection, or a
/// negative error code. The connection STAYS in its table slot — only the
/// `accepted` flag is set; the syscall layer wraps the index into a process
/// fd (review NET-C4: the old code moved sockets between three manager
/// lists that RX never looked at, so accept() always returned EAGAIN).
pub fn tcp_accept(fd: i32) -> i32 {
    // R24 (R23-3 follow-up): the scan+flag must be atomic against the RX
    // softirq and the timer tick (both mutate the table under
    // TCP_TABLE_LOCK) AND against a second concurrent accept() on the same
    // listener — two unlocked scanners both saw accepted==false and both
    // claimed the same child (two fds over one connection). Safe to lock
    // here: sys_accept drains ethernet_poll() BEFORE calling this, and
    // tcp_accept itself never re-enters tcp_rcv.
    let _table_g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: single-core softirq/syscall serialization (review NET-M15).
    unsafe {
        // Validate the listening socket
        let listen_socket = match TCP_SOCKET_TABLE.get(fd as usize) {
            Some(s) => s,
            None => return -9, // EBADF
        };
        if listen_socket.state != TcpState::TCP_LISTEN {
            return -22; // EINVAL
        }

        // Find an established, not-yet-accepted child of this listener.
        // R32-B8: CLOSE_WAIT children are acceptable too — the peer FINed
        // before accept() ran (port-scan pattern). Excluding them made
        // them permanently unacceptable, so they could never gain an fd
        // and sat in CLOSE_WAIT until the orphan timeout; handing them
        // out now delivers the buffered data + EOF to the application
        // (recv is legal in CLOSE_WAIT).
        for i in 0..TCP_SOCKET_TABLE.count {
            if let Some(socket) = TCP_SOCKET_TABLE.sockets[i].as_mut() {
                if socket.parent_fd == Some(fd)
                    && !socket.accepted
                    && (socket.state == TcpState::TCP_ESTABLISHED
                        || socket.state == TcpState::TCP_CLOSE_WAIT)
                {
                    socket.accepted = true;
                    return i as i32;
                }
            }
        }

        -11 // EAGAIN (no completed connections)
    }
}

// ============================================================================
// W3: blocking-semantics query helpers (socket layer wait conditions)
// ============================================================================

/// Would a recv() on this slot return data, EOF or an error (i.e. anything
/// but EAGAIN)? Used by the blocking-read re-check and poll.
pub fn tcp_readable(fd: i32) -> bool {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        match TCP_SOCKET_TABLE.get(fd as usize) {
            Some(ts) => {
                !ts.recv_buffer.is_empty()
                    || ts.pending_error != 0
                    // Closed / peer-FINed: recv returns EOF, not EAGAIN.
                    || ts.state == TcpState::TCP_CLOSE
                    || ts.state == TcpState::TCP_CLOSE_WAIT
            }
            None => true, // slot gone — let recv() surface the error
        }
    }
}

/// Can send() accept more bytes now (established with window room, or a
/// terminal state whose error returns immediately)?
pub fn tcp_send_ready(fd: i32) -> bool {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        match TCP_SOCKET_TABLE.get(fd as usize) {
            Some(ts) => match ts.state {
                TcpState::TCP_SYN_SENT | TcpState::TCP_SYN_RECV => false, // handshake pending
                TcpState::TCP_ESTABLISHED => {
                    let in_flight = ts.snd_nxt.wrapping_sub(ts.snd_una);
                    core::cmp::min(ts.snd_wnd as u32, ts.congestion.cwnd) > in_flight
                }
                _ => true, // terminal: send() errors out immediately
            },
            None => true,
        }
    }
}

/// Does this listener have an established, not-yet-accepted child?
/// (Blocking-accept wait condition; mirrors tcp_accept's scan.)
pub fn tcp_accept_pending(fd: i32) -> bool {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        for i in 0..TCP_SOCKET_TABLE.count {
            if let Some(s) = TCP_SOCKET_TABLE.sockets[i].as_ref() {
                if s.parent_fd == Some(fd)
                    && !s.accepted
                    && (s.state == TcpState::TCP_ESTABLISHED
                        || s.state == TcpState::TCP_CLOSE_WAIT)
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Locked read of a slot's bound local port (ephemeral bind readback).
pub fn tcp_local_port(fd: i32) -> u16 {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe { TCP_SOCKET_TABLE.get(fd as usize).map(|s| s.local_port).unwrap_or(0) }
}

/// W3: locked read of a slot's remote endpoint (accept's addr writeout).
pub fn tcp_remote_endpoint(fd: i32) -> (u32, u16) {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        TCP_SOCKET_TABLE
            .get(fd as usize)
            .map(|s| (s.remote_ip, s.remote_port))
            .unwrap_or((0, 0))
    }
}

/// P1 IPv6: locked read of a slot's v6 remote endpoint (accept's addr
/// writeout on AF_INET6 sockets; unspecified for v4 slots).
pub fn tcp_remote_endpoint6(fd: i32) -> (crate::net::ipv6::Ipv6Addr, u16) {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        TCP_SOCKET_TABLE
            .get(fd as usize)
            .map(|s| (s.remote_ip6, s.remote_port))
            .unwrap_or((crate::net::ipv6::IPV6_ADDR_UNSPECIFIED, 0))
    }
}

/// P1 IPv6: locked read of a slot's v6 local endpoint (getsockname mirror
/// after tcp_connect6 filled the source).
pub fn tcp_local_endpoint6(fd: i32) -> (crate::net::ipv6::Ipv6Addr, u16) {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        TCP_SOCKET_TABLE
            .get(fd as usize)
            .map(|s| (s.local_ip6, s.local_port))
            .unwrap_or((crate::net::ipv6::IPV6_ADDR_UNSPECIFIED, 0))
    }
}

/// P1 IPv6: locked read of a slot's family flag (accept's addr shape).
pub fn tcp_is_v6(fd: i32) -> bool {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        TCP_SOCKET_TABLE
            .get(fd as usize)
            .map(|s| s.is_v6)
            .unwrap_or(false)
    }
}

/// P1 IPv6: mark a slot as v6 with its local address (bind path — a v6
/// listener must match in handle_tcp_packet6's spawn pass).
pub fn tcp_set_v6_local(fd: i32, addr6: crate::net::ipv6::Ipv6Addr) {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        if let Some(s) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            s.is_v6 = true;
            s.local_ip6 = addr6;
        }
    }
}

/// P1 /proc/net/tcp: one protocol-slot snapshot (under TCP_TABLE_LOCK).
#[derive(Debug, Clone, Copy)]
pub struct TcpSlotInfo {
    pub local_ip: u32,
    pub local_port: u16,
    pub remote_ip: u32,
    pub remote_port: u16,
    /// Linux TCP state number (1 ESTABLISHED ... 10 LISTEN, 11 CLOSING)
    pub linux_state: u8,
    pub is_v6: bool,
    pub local_ip6: crate::net::ipv6::Ipv6Addr,
    pub remote_ip6: crate::net::ipv6::Ipv6Addr,
}

/// Linux state-number mapping (our enum order differs).
fn tcp_linux_state(s: TcpState) -> u8 {
    match s {
        TcpState::TCP_ESTABLISHED => 1,
        TcpState::TCP_SYN_SENT => 2,
        TcpState::TCP_SYN_RECV => 3,
        TcpState::TCP_FIN_WAIT1 => 4,
        TcpState::TCP_FIN_WAIT2 => 5,
        TcpState::TCP_TIME_WAIT => 6,
        TcpState::TCP_CLOSE => 7,
        TcpState::TCP_CLOSE_WAIT => 8,
        TcpState::TCP_LAST_ACK => 9,
        TcpState::TCP_LISTEN => 10,
        TcpState::TCP_CLOSING => 11,
    }
}

/// P1 /proc/net/tcp(+tcp6): snapshot every live TCP slot.
pub fn tcp_dump() -> alloc::vec::Vec<TcpSlotInfo> {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    let mut out = alloc::vec::Vec::new();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        let table = &TCP_SOCKET_TABLE;
        for slot in table.sockets.iter().take(table.count) {
            if let Some(s) = slot.as_ref() {
                if s.state == TcpState::TCP_CLOSE && !s.bound {
                    continue; // never-used slot
                }
                out.push(TcpSlotInfo {
                    local_ip: s.local_ip,
                    local_port: s.local_port,
                    remote_ip: s.remote_ip,
                    remote_port: s.remote_port,
                    linux_state: tcp_linux_state(s.state),
                    is_v6: s.is_v6,
                    local_ip6: s.local_ip6,
                    remote_ip6: s.remote_ip6,
                });
            }
        }
    }
    out
}

/// Read-and-clear the slot's pending protocol error (SO_ERROR semantics).
pub fn tcp_take_pending_error(fd: i32) -> i32 {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        match TCP_SOCKET_TABLE.get_mut(fd as usize) {
            Some(s) => core::mem::replace(&mut s.pending_error, 0),
            None => 0,
        }
    }
}

/// Mirror SO_REUSEADDR into the protocol slot (participates in bind checks).
pub fn tcp_set_reuseaddr(fd: i32, on: bool) {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        if let Some(s) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            s.reuseaddr = on;
        }
    }
}

/// P2 SO_KEEPALIVE: mirror the socket-layer option triplet into the
/// protocol slot (Linux tcp_keepalive_timer parameters). Also resets any
/// in-flight probe cycle so a freshly-enabled socket starts a clean idle
/// window.
pub fn tcp_set_keepalive(fd: i32, on: bool, idle_s: u32, intvl_s: u32, cnt: u32) {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        if let Some(s) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            s.keepalive = on;
            s.ka_idle_s = idle_s;
            s.ka_intvl_s = intvl_s;
            s.ka_cnt = cnt;
            if !on {
                s.timers.keepalive_deadline = 0;
                s.timers.keepalive_probes = 0;
            }
        }
    }
}

/// P2 IP_TTL: mirror the per-socket TTL into the protocol slot (0 =
/// system default 64).
pub fn tcp_set_ttl(fd: i32, ttl: u8) {
    let _g = TCP_TABLE_LOCK.lock_irqsave();
    // SAFETY: TCP_SOCKET_TABLE is a global; protected by TCP_TABLE_LOCK.
    unsafe {
        if let Some(s) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            s.ttl = ttl;
        }
    }
}

/// Calculate TCP checksum
///
/// # Arguments
/// - `shdr`: Source IP address (network byte order)
/// - `dhdr`: Destination IP address (network byte order)
/// - `thdr`: TCP header
/// - `data`: Data
///
/// # Returns
/// Checksum (network byte order)
pub fn tcp_checksum(shdr: u32, dhdr: u32, thdr: &TcpHdr, data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Pseudo header (12 bytes). Callers pass network-order (`.to_be()`)
    // values; the halves must be converted to host word values so they sum
    // the same wire-bytes pairing as the header/data words below (review
    // NET-H2 — the raw shifts summed byte-swapped halves, so every outbound
    // TCP checksum was wrong).
    // Source IP (4 bytes)
    sum += u16::from_be((shdr >> 16) as u16) as u32;
    sum += u16::from_be(shdr as u16) as u32;
    // Destination IP (4 bytes)
    sum += u16::from_be((dhdr >> 16) as u16) as u32;
    sum += u16::from_be(dhdr as u16) as u32;
    // Reserved (1 byte) + Protocol (1 byte) + TCP length (2 bytes)
    sum += 6u32; // TCP protocol number (reserved=0, protocol=6)
    let tcp_len = (thdr.header_len() + data.len()) as u16;
    sum += tcp_len as u32;

    // TCP header (include full header with options)
    // SAFETY: thdr is a valid TcpHdr reference; reading header_len() bytes
    // from its repr(C) layout is well-defined. The underlying skb buffer is
    // large enough for the full TCP header.
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(
            (thdr as *const TcpHdr) as *const u8,
            thdr.header_len()
        )
    };

    let mut i = 0;
    while i + 1 < hdr_bytes.len() {
        let word = u16::from_be_bytes([hdr_bytes[i], hdr_bytes[i + 1]]) as u32;
        sum += word;
        i += 2;
    }

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

/// Build TCP packet
///
/// # Arguments
/// - `skb`: SkBuff
/// - `source`: Source port
/// - `dest`: Destination port
/// - `seq`: Sequence number
/// - `ack_seq`: Acknowledgment number
/// - `flags`: Flag bits
/// - `window`: Window size
/// - `src_ip`: Source IP address (network byte order)
/// - `dest_ip`: Destination IP address (network byte order)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn tcp_build_packet(
    skb: &mut SkBuff,
    source: TcpPort,
    dest: TcpPort,
    seq: TcpSeq,
    ack_seq: TcpAck,
    flags: u16,
    window: u16,
    src_ip: u32,
    dest_ip: u32,
    opts: &TcpOptOut,
) -> Result<(), ()> {
    // P2 TCP options: SYN segments carry MSS + WS + TS (20 bytes), data
    // segments TS only (12 bytes) — wire_len pads to a 4-byte multiple.
    let is_syn = flags & 0x0002 != 0;
    let opt_len = opts.wire_len(is_syn);
    let hdr_len = TCP_MIN_HLEN + opt_len;

    // Allocate space for TCP header
    let ptr = skb.skb_push(hdr_len as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // hdr_len bytes; writing fields of repr(C) TcpHdr is well-defined.
    unsafe {
        let tcp_hdr = &mut *(ptr as *mut TcpHdr);

        // Source port
        tcp_hdr.source = source.to_be();

        // Destination port
        tcp_hdr.dest = dest.to_be();

        // Sequence number
        tcp_hdr.seq = seq.to_be();

        // Acknowledgment number
        tcp_hdr.ack_seq = ack_seq.to_be();

        // Data offset (20 bytes = 5 32-bit words, plus options)
        tcp_hdr.set_dof(5 + (opt_len / 4) as u8);

        // Window size
        tcp_hdr.set_window(window);

        // Flags
        tcp_hdr.flags = (flags & 0xFF) as u8;

        // Checksum (set to 0 first, calculate later)
        tcp_hdr.check = 0;

        // Urgent pointer
        tcp_hdr.urg_ptr = 0;

        // P2: serialize the option bytes after the fixed header.
        if opt_len > 0 {
            let opt_bytes = core::slice::from_raw_parts_mut(ptr.add(TCP_MIN_HLEN), opt_len);
            let _ = opts.encode(opt_bytes, is_syn);
        }

        // Compute TCP checksum (RFC 793). The field is big-endian on the
        // wire — store the network-order value (review NEW: missing .to_be()
        // byte-swapped every outbound segment's checksum).
        let data_ptr = ptr.add(hdr_len);
        let data_len = (skb.len as usize).saturating_sub(hdr_len);
        let data_slice = core::slice::from_raw_parts(data_ptr as *const u8, data_len);
        tcp_hdr.check = tcp_checksum(src_ip, dest_ip, tcp_hdr, data_slice).to_be();
    }

    Ok(())
}

/// P1 IPv6: build a TCP segment with the RFC 8200 §8.1 pseudo-header
/// checksum (128-bit endpoints). Mirrors tcp_build_packet.
#[allow(clippy::too_many_arguments)]
pub fn tcp_build_packet6(
    skb: &mut SkBuff,
    source: TcpPort,
    dest: TcpPort,
    seq: TcpSeq,
    ack_seq: TcpAck,
    flags: u16,
    window: u16,
    src_ip6: &crate::net::ipv6::Ipv6Addr,
    dest_ip6: &crate::net::ipv6::Ipv6Addr,
    opts: &TcpOptOut,
) -> Result<(), ()> {
    // P2 TCP options (see tcp_build_packet).
    let is_syn = flags & 0x0002 != 0;
    let opt_len = opts.wire_len(is_syn);
    let hdr_len = TCP_MIN_HLEN + opt_len;

    let ptr = skb.skb_push(hdr_len as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // hdr_len bytes; writing fields of repr(C) TcpHdr is well-defined.
    unsafe {
        let tcp_hdr = &mut *(ptr as *mut TcpHdr);

        tcp_hdr.source = source.to_be();
        tcp_hdr.dest = dest.to_be();
        tcp_hdr.seq = seq.to_be();
        tcp_hdr.ack_seq = ack_seq.to_be();
        tcp_hdr.set_dof(5 + (opt_len / 4) as u8);
        tcp_hdr.set_window(window);
        tcp_hdr.flags = (flags & 0xFF) as u8;
        tcp_hdr.check = 0;
        tcp_hdr.urg_ptr = 0;

        if opt_len > 0 {
            let opt_bytes = core::slice::from_raw_parts_mut(ptr.add(TCP_MIN_HLEN), opt_len);
            let _ = opts.encode(opt_bytes, is_syn);
        }

        let data_len = (skb.len as usize).saturating_sub(hdr_len);
        let mut csum = crate::net::ipv6::transport_checksum6(
            src_ip6,
            dest_ip6,
            crate::net::ipv6::next_header::TCP,
            // The checksum covers header + data with the field zeroed; the
            // header is fully built above except check (already 0).
            core::slice::from_raw_parts(ptr as *const u8, hdr_len + data_len),
        );
        if csum == 0 {
            csum = 0xFFFF;
        }
        tcp_hdr.check = csum.to_be();
    }

    Ok(())
}

/// Parse TCP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing TCP packet)
///
/// # Returns
/// TCP header reference, or None if parsing fails
///
/// Helper: get TCP payload as slice with checked arithmetic to avoid underflow.
fn tcp_payload_slice(skb: &SkBuff, header_len: usize) -> Option<&'static [u8]> {
    let payload_len = (skb.len as usize).checked_sub(header_len)?;
    // SAFETY: header_len is validated by tcp_parse_packet; payload_len is now >= 0.
    unsafe { Some(core::slice::from_raw_parts(skb.data.add(header_len), payload_len)) }
}

pub fn tcp_parse_packet(skb: &SkBuff) -> Option<&'static TcpHdr> {
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < TCP_MIN_HLEN {
        return None;
    }

    let tcp_hdr = TcpHdr::from_bytes(data)?;

    // Validate header length
    let hdr_len = tcp_hdr.header_len();
    if hdr_len < TCP_MIN_HLEN || hdr_len > TCP_MAX_HLEN {
        return None;
    }
    // W3: the header (options included) must fit inside the packet — an
    // over-long data offset made tcp_payload_slice underflow and fed the
    // checksum/state machine garbage bytes.
    if hdr_len > data.len() {
        return None;
    }

    Some(tcp_hdr)
}

/// W3: parse the MSS option (kind 2, len 4) from a SYN's option bytes.
/// Returns None when the option is absent or malformed.
pub fn tcp_parse_mss(tcp_hdr: &TcpHdr) -> Option<u16> {
    let hlen = tcp_hdr.header_len();
    if hlen <= TCP_MIN_HLEN || hlen > TCP_MAX_HLEN {
        return None;
    }
    // SAFETY: tcp_hdr aliases the skb data buffer; the option bytes occupy
    // [TCP_MIN_HLEN, header_len) of the same buffer, which
    // tcp_parse_packet validated against the packet length.
    let opts = unsafe {
        core::slice::from_raw_parts(
            (tcp_hdr as *const TcpHdr as *const u8).add(TCP_MIN_HLEN),
            hlen - TCP_MIN_HLEN,
        )
    };
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            0 => break,           // End of option list
            1 => { i += 1; }      // NOP
            kind => {
                if i + 1 >= opts.len() {
                    break;
                }
                let len = opts[i + 1] as usize;
                if len < 2 || i + len > opts.len() {
                    break;
                }
                if kind == 2 && len == 4 {
                    return Some(u16::from_be_bytes([opts[i + 2], opts[i + 3]]));
                }
                i += len;
            }
        }
    }
    None
}

/// P2 TCP options: raw option-bytes accessor shared by the parsers.
/// SAFETY contract mirrors tcp_parse_mss (caller-validated header).
fn tcp_option_bytes(tcp_hdr: &TcpHdr) -> &'static [u8] {
    let hlen = tcp_hdr.header_len();
    if hlen <= TCP_MIN_HLEN || hlen > TCP_MAX_HLEN {
        return &[];
    }
    // SAFETY: tcp_hdr aliases the skb data buffer; the option bytes occupy
    // [TCP_MIN_HLEN, header_len) of the same buffer, which
    // tcp_parse_packet validated against the packet length.
    unsafe {
        core::slice::from_raw_parts(
            (tcp_hdr as *const TcpHdr as *const u8).add(TCP_MIN_HLEN),
            hlen - TCP_MIN_HLEN,
        )
    }
}

/// P2: parse the window-scale option (kind 3, len 3) from a SYN.
/// Returns the peer's advertised shift (0..=14), or None when absent.
pub fn tcp_parse_wscale(tcp_hdr: &TcpHdr) -> Option<u8> {
    let opts = tcp_option_bytes(tcp_hdr);
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            0 => break,
            1 => { i += 1; }
            kind => {
                if i + 1 >= opts.len() {
                    break;
                }
                let len = opts[i + 1] as usize;
                if len < 2 || i + len > opts.len() {
                    break;
                }
                if kind == 3 && len == 3 {
                    return Some(opts[i + 2].min(14));
                }
                i += len;
            }
        }
    }
    None
}

/// P2: parse the timestamps option (kind 8, len 10) from a segment.
/// Returns the peer's TSval.
pub fn tcp_parse_tsval(tcp_hdr: &TcpHdr) -> Option<u32> {
    let opts = tcp_option_bytes(tcp_hdr);
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            0 => break,
            1 => { i += 1; }
            kind => {
                if i + 1 >= opts.len() {
                    break;
                }
                let len = opts[i + 1] as usize;
                if len < 2 || i + len > opts.len() {
                    break;
                }
                if kind == 8 && len == 10 && i + 6 <= opts.len() {
                    return Some(u32::from_be_bytes([
                        opts[i + 2],
                        opts[i + 3],
                        opts[i + 4],
                        opts[i + 5],
                    ]));
                }
                i += len;
            }
        }
    }
    None
}

/// P2 TCP timestamps: the TSval clock — milliseconds since boot (jiffies
/// × 10 ms), truncated to 32 bits like Linux's jiffies-based TS clock.
fn tcp_ts_now() -> u32 {
    (crate::drivers::timer::get_jiffies().wrapping_mul(10)) as u32
}

/// W3: RX segments dropped for TCP checksum mismatch (observability).
pub static TCP_RX_CSUM_ERRORS: AtomicU32 = AtomicU32::new(0);

/// Receive and process TCP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing TCP packet)
/// - `src_ip`: Source IP address (host order)
/// - `dest_ip`: Destination IP address (host order)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
///
/// R35 (chain-2 fix): every outbound segment this packet triggers
/// (SYN-ACK, ACKs, dup-ACKs, fast retransmit, RST) is recorded into a
/// TcpTxBatch whose capacity is reserved BEFORE the table lock is taken,
/// and emitted AFTER the lock drops — the ack path used to run
/// ethernet_send → virtio xmit (a 10M+50M-iteration completion spin,
/// seconds per packet under tcg) inline under TCP_TABLE_LOCK.
pub fn tcp_rcv(skb: &SkBuff, src_ip: u32, dest_ip: u32) -> Result<(), ()> {
    let manager = get_tcp_manager();

    // W3: verify the TCP checksum BEFORE any state-machine processing —
    // UDP already validated, TCP accepted bit-flipped segments as valid
    // and enqueued corrupted data. A correct segment's checksum (computed
    // over header+data INCLUDING the stored checksum field) sums to zero.
    let tcp_hdr = match tcp_parse_packet(skb) {
        Some(h) => h,
        None => return Ok(()),
    };
    {
        let payload = tcp_payload_slice(skb, tcp_hdr.header_len()).unwrap_or(&[]);
        if tcp_checksum(src_ip.to_be(), dest_ip.to_be(), tcp_hdr, payload) != 0 {
            TCP_RX_CSUM_ERRORS.fetch_add(1, Ordering::Relaxed);
            return Ok(()); // silently drop (counted)
        }
    }

    // Worst case per inbound packet: SYN-ACK spawn (1) or fast-retrans
    // data (1 x MSS) + dup/data ACK + FIN ACK (see handle_packet arms) —
    // 8 descriptor slots and one MSS of arena is a comfortable bound.
    // Reserve failure (OOM) degrades to segment drops (peer retransmits;
    // R34: allocation OUTSIDE the lock cannot wedge the kernel).
    let mut tx = TcpTxBatch::new();
    let _ = tx.reserve(8, TCP_DEFAULT_MSS as usize);

    // W3: slots whose RX-visible state changed (data/establish/error) —
    // their VFS wait queues are woken after the lock drops.
    let mut wake: Option<TcpRvWake> = None;

    {
        // R21-N1: RX path mutates the shared table (states, buffers, slot
        // frees) — serialized against syscalls and the timer tick.
        let _table_g = TCP_TABLE_LOCK.lock_irqsave();

        match manager.handle_tcp_packet(skb, src_ip, dest_ip, &mut tx) {
            Ok(w) => wake = w,
            Err(()) => {
                // No matching connection found — send RST (RFC 793 §3.9).
                // R35: recorded into `tx` and emitted after the lock drops.
                if !tcp_hdr.rst() && dest_ip != 0xFFFFFFFF {
                    let _ = tcp_send_reset(src_ip, dest_ip, tcp_hdr, &mut tx);
                }
            }
        }
    }

    // No table lock held here: re-entry is impossible (loopback_send only
    // queues to the backlog drained by a later ethernet_poll; virtio TX
    // goes straight to the device) — same argument as Socket::close.
    tx.emit_all();

    // W3: wake blocking recv/accept waiters on the affected sockets (data
    // landed, connection established, RST/error recorded).
    if let Some(w) = wake {
        crate::net::socket::wake_tcp_socket(w.socket);
        if let Some(parent) = w.parent {
            crate::net::socket::wake_tcp_socket(parent);
        }
    }

    Ok(())
}

/// Record an RST (response to a segment for a non-existing connection)
/// into the deferred-TX batch (R35 — was an inline alloc_skb + emit under
/// TCP_TABLE_LOCK).
fn tcp_send_reset(src_ip: u32, dest_ip: u32, tcp_hdr: &TcpHdr, tx: &mut TcpTxBatch) -> Result<(), ()> {
    // RST sequence number: if ACK is set, seq = ack_seq; otherwise seq = 0
    let rst_seq = if tcp_hdr.ack() {
        TcpSeq::from_be(tcp_hdr.ack_seq)
    } else {
        0
    };
    // RST ACK: if ACK is set, ack = 0; otherwise ack = seq + len
    let rst_ack = if tcp_hdr.ack() {
        0
    } else {
        let seg_len = if tcp_hdr.syn() { 1 } else { 0 }
            + if tcp_hdr.fin() { 1 } else { 0 };
        TcpSeq::from_be(tcp_hdr.seq).wrapping_add(seg_len)
    };

    // Source of the RST is the addressed local IP (matches the TCP
    // pseudo-header the old tcp_build_packet call used; the old
    // ipv4_send(skb, src_ip, 6) filled the IP source with the DEVICE
    // address instead — an inconsistency this path inherits fixed, every
    // other sender already uses the socket's local_ip via ipv4_send_src).
    let ok = tx.push_ctl(
        dest_ip,
        src_ip,
        TcpPort::from_be(tcp_hdr.dest),
        TcpPort::from_be(tcp_hdr.source),
        rst_seq,
        rst_ack,
        0x0014, // RST + ACK
        TCP_MAX_WINDOW,
    );
    if ok {
        Ok(())
    } else {
        Err(()) // dropped — peer times out / retransmits (recoverable)
    }
}

/// P1 IPv6: RST for a segment that matched no v6 connection (see
/// tcp_send_reset).
fn tcp_send_reset6(
    src6: &crate::net::ipv6::Ipv6Addr,
    dest6: &crate::net::ipv6::Ipv6Addr,
    tcp_hdr: &TcpHdr,
    tx: &mut TcpTxBatch,
) -> Result<(), ()> {
    let rst_seq = if tcp_hdr.ack() {
        TcpSeq::from_be(tcp_hdr.ack_seq)
    } else {
        0
    };
    let rst_ack = if tcp_hdr.ack() {
        0
    } else {
        let seg_len = if tcp_hdr.syn() { 1 } else { 0 }
            + if tcp_hdr.fin() { 1 } else { 0 };
        TcpSeq::from_be(tcp_hdr.seq).wrapping_add(seg_len)
    };

    let ok = tx.push_ctl6(
        dest6,
        src6,
        TcpPort::from_be(tcp_hdr.dest),
        TcpPort::from_be(tcp_hdr.source),
        rst_seq,
        rst_ack,
        0x0014, // RST + ACK
        TCP_MAX_WINDOW,
    );
    if ok {
        Ok(())
    } else {
        Err(())
    }
}

/// P1 IPv6: receive a TCP-over-IPv6 segment (checksum verified against the
/// 128-bit pseudo-header; same deferred-TX/wake discipline as tcp_rcv).
pub fn tcp_rcv6(
    skb: &SkBuff,
    src6: &crate::net::ipv6::Ipv6Addr,
    dest6: &crate::net::ipv6::Ipv6Addr,
) -> Result<(), ()> {
    let manager = get_tcp_manager();

    let tcp_hdr = match tcp_parse_packet(skb) {
        Some(h) => h,
        None => return Ok(()),
    };
    {
        // Verify with the v6 pseudo-header: a correct segment (computed
        // over header+data INCLUDING the stored checksum) sums to zero.
        let payload = tcp_payload_slice(skb, tcp_hdr.header_len()).unwrap_or(&[]);
        let total = tcp_hdr.header_len() + payload.len();
        // SAFETY: tcp_parse_packet validated header_len <= skb.len; the
        // payload slice bounds the rest.
        let seg = unsafe {
            core::slice::from_raw_parts(skb.data as *const u8, total)
        };
        if crate::net::ipv6::transport_checksum6(
            src6,
            dest6,
            crate::net::ipv6::next_header::TCP,
            seg,
        ) != 0
        {
            TCP_RX_CSUM_ERRORS.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }

    let mut tx = TcpTxBatch::new();
    let _ = tx.reserve(8, TCP_DEFAULT_MSS as usize);
    let mut wake: Option<TcpRvWake> = None;

    {
        let _table_g = TCP_TABLE_LOCK.lock_irqsave();
        match manager.handle_tcp_packet6(skb, src6, dest6, &mut tx) {
            Ok(w) => wake = w,
            Err(()) => {
                if !tcp_hdr.rst() && !crate::net::ipv6::is_multicast(dest6) {
                    let _ = tcp_send_reset6(src6, dest6, tcp_hdr, &mut tx);
                }
            }
        }
    }

    tx.emit_all();

    if let Some(w) = wake {
        crate::net::socket::wake_tcp_socket(w.socket);
        if let Some(parent) = w.parent {
            crate::net::socket::wake_tcp_socket(parent);
        }
    }

    Ok(())
}

/// Handle ICMP error for a TCP connection (soft error)
///
/// Called when ICMP destination unreachable or time exceeded is received
/// for a packet that matches one of our TCP connections.
///
/// The embedded original header belongs to OUR OWN outbound packet:
/// `orig_src_*` is this connection's LOCAL end and `orig_dst_*` its REMOTE
/// end. W3: the old matching compared local_port against the original
/// DESTINATION and remote_ip against the original SOURCE — every lookup
/// missed, so ICMP fast-fail (RST-equivalent abort) never fired.
pub fn tcp_v4_err(
    icmp_type: u8,
    icmp_code: u8,
    orig_src_ip: u32,
    orig_src_port: u16,
    orig_dst_ip: u32,
    orig_dst_port: u16,
) {
    let mut wake_fd: Option<i32> = None;
    {
        let _g = TCP_TABLE_LOCK.lock_irqsave();
        // SAFETY: TCP_SOCKET_TABLE is a global; caller (icmp_rcv softirq /
        // syscall poll) is serialized by the table lock.
        unsafe {
            'scan: for i in 0..TCP_SOCKET_TABLE.count {
                let socket = match TCP_SOCKET_TABLE.sockets[i].as_mut() {
                    Some(s) => s,
                    None => continue,
                };
                if socket.local_port == orig_src_port
                    && socket.remote_port == orig_dst_port
                    && socket.remote_ip == orig_dst_ip
                    && (socket.local_ip == 0 || socket.local_ip == orig_src_ip)
                {
                    match icmp_type {
                        crate::net::icmp::icmp_type::DEST_UNREACH => {
                            // Abort the connection on host/port/net unreachable
                            match icmp_code {
                                crate::net::icmp::icmp_code::HOST_UNREACH => {
                                    socket.pending_error = 113; // EHOSTUNREACH
                                    socket.state = TcpState::TCP_CLOSE;
                                }
                                crate::net::icmp::icmp_code::PORT_UNREACH => {
                                    // Peer refused: the classic RST-equivalent
                                    socket.pending_error = 111; // ECONNREFUSED
                                    socket.state = TcpState::TCP_CLOSE;
                                }
                                crate::net::icmp::icmp_code::NET_UNREACH => {
                                    socket.pending_error = 101; // ENETUNREACH
                                    socket.state = TcpState::TCP_CLOSE;
                                }
                                _ => {
                                    // FRAG_NEEDED etc. — just record, don't abort
                                }
                            }
                        }
                        crate::net::icmp::icmp_type::TIME_EXCEEDED => {
                            // TTL expired — abort
                            socket.pending_error = 110; // ETIMEDOUT
                            socket.state = TcpState::TCP_CLOSE;
                        }
                        _ => {}
                    }
                    socket.send_buffer.clear();
                    socket.retrans_queue.clear();
                    socket.timers.stop_retransmit();
                    wake_fd = Some(i as i32);
                    break 'scan;
                }
            }
        }
    }
    // W3: wake blocked writers/readers sleeping on the aborted connection.
    if let Some(fd) = wake_fd {
        crate::net::socket::wake_tcp_socket(fd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcphdr_size() {
        assert_eq!(core::mem::size_of::<TcpHdr>(), 20);
    }

    #[test]
    fn test_tcp_socket() {
        let mut socket = TcpSocket::new();
        assert_eq!(socket.state, TcpState::TCP_CLOSE);
        assert!(!socket.bound);

        assert!(socket.bind(8080).is_ok());
        assert!(socket.bound);

        assert!(socket.listen(10).is_ok());
        assert_eq!(socket.state, TcpState::TCP_LISTEN);
    }

    #[test]
    fn test_tcp_socket_alloc() {
        let fd1 = tcp_socket_alloc();
        assert!(fd1.is_ok());
        assert_eq!(fd1.unwrap(), 0);

        let fd2 = tcp_socket_alloc();
        assert!(fd2.is_ok());
        assert_eq!(fd2.unwrap(), 1);

        tcp_socket_free(fd1.unwrap());
        tcp_socket_free(fd2.unwrap());
    }

    #[test]
    fn test_tcp_flags() {
        let mut hdr = TcpHdr::default();

        assert!(!hdr.syn());
        hdr.set_syn();
        assert!(hdr.syn());

        assert!(!hdr.ack());
        hdr.set_ack();
        assert!(hdr.ack());

        assert!(!hdr.fin());
        hdr.set_fin();
        assert!(hdr.fin());
    }
}
