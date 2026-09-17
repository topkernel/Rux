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

/// TCP port number
pub type TcpPort = u16;

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
/// Stores copy of sent but unacknowledged data
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

impl TcpSendSeg {
    pub fn new(seq: TcpSeq, data: &[u8], tx_time: u64) -> Self {
        Self {
            seq,
            len: data.len(),
            data: alloc::vec::Vec::from(data),
            tx_time,
            retries: 0,
        }
    }
}

/// TCP out-of-order segment (for reassembly queue)
#[derive(Debug, Clone)]
pub struct TcpOooSeg {
    /// Starting sequence number
    pub seq: TcpSeq,
    /// Segment data
    pub data: alloc::vec::Vec<u8>,
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
}

impl TcpTimers {
    pub fn new() -> Self {
        Self {
            retransmit_deadline: 0,
            delack_deadline: 0,
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
    pub fn connect(&mut self, ip: u32, port: TcpPort) -> Result<(), ()> {
        self.remote_ip = ip;
        self.remote_port = port;

        // Initialize sequence number from connection 4-tuple
        self.snd_nxt = generate_isn(self.local_ip, self.local_port, ip, port);
        self.snd_una = self.snd_nxt;
        self.rcv_nxt = 0; // Will be obtained from SYN-ACK

        // Send SYN packet (first step of three-way handshake)
        self.send_syn()?;
        self.state = TcpState::TCP_SYN_SENT;

        Ok(())
    }

    /// Send SYN packet (first step of three-way handshake)
    fn send_syn(&self) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            0, // ACK number is 0
            0x0002, // SYN flag
            self.rcv_wnd,
            self.local_ip.to_be(),
            self.remote_ip.to_be(),
        )?;

        // Send to IP layer
        crate::net::ipv4::ipv4_send_src(skb, self.local_ip, self.remote_ip, 6); // IPPROTO_TCP = 6

        Ok(())
    }

    /// Send SYN-ACK packet (second step of three-way handshake)
    fn send_synack(&mut self, _ack_seq: TcpSeq) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            0x0012, // SYN + ACK flags
            self.rcv_wnd,
            self.local_ip.to_be(),
            self.remote_ip.to_be(),
        )?;

        crate::net::ipv4::ipv4_send_src(skb, self.local_ip, self.remote_ip, 6);

        // R14-7 (HIGH-3): the SYN consumes one sequence number. Without
        // this, the peer's rcv_nxt (= ISN+1) mismatched every subsequent
        // server segment, and the first client ACK made in_flight wrap
        // (usable_window 0) — server-side TX permanently blocked.
        self.snd_nxt = self.snd_nxt.wrapping_add(1);

        Ok(())
    }

    /// Send ACK packet (third step of three-way handshake)
    fn send_ack(&self) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            0x0010, // ACK flag
            self.rcv_wnd,
            self.local_ip.to_be(),
            self.remote_ip.to_be(),
        )?;

        crate::net::ipv4::ipv4_send_src(skb, self.local_ip, self.remote_ip, 6);

        Ok(())
    }

    /// Send FIN+ACK packet
    ///
    /// FIN consumes one sequence number per RFC 793, so snd_nxt is
    /// incremented after sending.
    fn send_fin(&mut self) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            0x0011, // FIN + ACK flags
            self.rcv_wnd,
            self.local_ip.to_be(),
            self.remote_ip.to_be(),
        )?;

        crate::net::ipv4::ipv4_send_src(skb, self.local_ip, self.remote_ip, 6);

        // FIN consumes one sequence number (RFC 793)
        self.snd_nxt = self.snd_nxt.wrapping_add(1);

        Ok(())
    }

    /// Start TIME_WAIT timer (reuses retransmit_deadline field)
    fn start_timewait_timer(&mut self) {
        let now = crate::drivers::timer::get_jiffies();
        let tw_jiffies = crate::config::TCP_TIMEWAIT_TIMEOUT_US / 10_000;
        self.timers.retransmit_deadline = now + tw_jiffies;
    }

    /// Send ACK packet (public interface, for timers)
    pub fn send_ack_public(&self) -> Result<(), ()> {
        self.send_ack()
    }

    /// Handle received TCP packet
    pub fn handle_packet(&mut self, tcp_hdr: &TcpHdr, data: &[u8]) -> Result<(), ()> {
        // Global RST handling (RFC 793 §3.9)
        if tcp_hdr.rst() {
            self.handle_rst_recv();
            return Ok(());
        }

        match self.state {
            TcpState::TCP_LISTEN => {
                // Server: receive SYN packet
                if tcp_hdr.syn() && !tcp_hdr.ack() {
                    self.handle_syn_recv(tcp_hdr)?;
                }
            }
            TcpState::TCP_SYN_SENT => {
                // Client: receive SYN-ACK packet
                if tcp_hdr.syn() && tcp_hdr.ack() {
                    self.handle_synack_recv(tcp_hdr)?;
                }
            }
            TcpState::TCP_SYN_RECV => {
                // Server: receive ACK packet
                if tcp_hdr.ack() && !tcp_hdr.syn() {
                    self.handle_ack_recv()?;
                }
            }
            TcpState::TCP_ESTABLISHED => {
                // Process ACK first (updates snd_una, cwnd, rtt)
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    self.process_ack(ack_num);
                }
                // Process data (may accompany FIN)
                if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data)?;
                }
                // Process FIN (may accompany data)
                if tcp_hdr.fin() {
                    self.handle_fin_recv()?;
                }
            }
            TcpState::TCP_FIN_WAIT1 => {
                // Process ACK (only accept if it falls within our send window)
                let mut valid_ack = false;
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    valid_ack = self.process_ack(ack_num);
                }
                if tcp_hdr.fin() && valid_ack {
                    // Simultaneous close: FIN+ACK -> TIME_WAIT
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    let _ = self.send_ack();
                    self.state = TcpState::TCP_TIME_WAIT;
                    self.start_timewait_timer();
                } else if valid_ack {
                    // ACK of our FIN -> FIN_WAIT2
                    self.state = TcpState::TCP_FIN_WAIT2;
                    // Data and/or FIN may follow
                    if !data.is_empty() {
                        self.handle_data_recv(tcp_hdr, data)?;
                    }
                    if tcp_hdr.fin() {
                        self.handle_fin_recv()?;
                    }
                } else if tcp_hdr.fin() {
                    // FIN without ACK -> CLOSING
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    let _ = self.send_ack();
                    self.state = TcpState::TCP_CLOSING;
                } else if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data)?;
                }
            }
            TcpState::TCP_FIN_WAIT2 => {
                // Waiting for FIN from remote
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    self.process_ack(ack_num);
                }
                if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data)?;
                }
                if tcp_hdr.fin() {
                    self.handle_fin_recv()?;
                }
            }
            TcpState::TCP_CLOSING => {
                // Simultaneous close: waiting for ACK of our FIN
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    if self.process_ack(ack_num) {
                        self.state = TcpState::TCP_TIME_WAIT;
                        self.start_timewait_timer();
                    }
                }
            }
            TcpState::TCP_LAST_ACK => {
                // Waiting for ACK of our FIN
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    if self.process_ack(ack_num) {
                        self.state = TcpState::TCP_CLOSE;
                    }
                }
            }
            TcpState::TCP_CLOSE_WAIT => {
                // Remote sent FIN, waiting for application to close
                if tcp_hdr.ack() {
                    let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
                    self.process_ack(ack_num);
                }
                if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data)?;
                }
            }
            _ => {
                // TCP_CLOSE, TCP_TIME_WAIT etc. — ignore
            }
        }

        Ok(())
    }

    /// Handle received SYN packet (server)
    fn handle_syn_recv(&mut self, tcp_hdr: &TcpHdr) -> Result<(), ()> {
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
        self.send_synack(self.rcv_nxt)?;
        self.state = TcpState::TCP_SYN_RECV;

        Ok(())
    }

    /// Handle received SYN-ACK packet (client)
    fn handle_synack_recv(&mut self, tcp_hdr: &TcpHdr) -> Result<(), ()> {
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
        self.send_ack()?;
        self.state = TcpState::TCP_ESTABLISHED;

        Ok(())
    }

    /// Handle received ACK packet (server)
    fn handle_ack_recv(&mut self) -> Result<(), ()> {
        // Check if ACK acknowledges our SYN-ACK
        // Three-way handshake complete, connection established
        // R14-7: the ACK acknowledges the SYN's sequence number — advance
        // snd_una in lockstep with send_synack's snd_nxt advance.
        self.snd_una = self.snd_una.wrapping_add(1);
        self.state = TcpState::TCP_ESTABLISHED;
        Ok(())
    }

    /// Handle received data (RFC 793 §3.9 window-based acceptance)
    fn handle_data_recv(&mut self, tcp_hdr: &TcpHdr, data: &[u8]) -> Result<(), ()> {
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
            self.send_ack()?;
            return Ok(());
        }

        // Case 2: segment is entirely after the window → out of window, drop
        if self.seq_after_or_eq(seq, rcv_wnd_end) {
            return Ok(());
        }

        if seq == rcv_nxt {
            // In-order segment → deliver to receive buffer
            self.enqueue_data(data);
            self.rcv_nxt = seg_end;

            // Drain any coalescible segments from the OOO queue
            self.drain_ooo_queue();

            self.send_ack()?;
        } else {
            // Out-of-order segment within window → buffer and send duplicate ACK
            self.ooo_queue.push_back(TcpOooSeg {
                seq,
                data: alloc::vec::Vec::from(data),
            });

            // Send duplicate ACK to trigger fast retransmit on sender
            self.send_ack()?;
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
                    self.enqueue_data(&seg.data[offset..]);
                    self.rcv_nxt = self.rcv_nxt.wrapping_add((seg.data.len() - offset) as u32);
                }
            } else {
                break;
            }
        }
    }

    /// Handle received FIN packet
    fn handle_fin_recv(&mut self) -> Result<(), ()> {
        // Update receive sequence number (FIN occupies one sequence number)
        self.rcv_nxt = self.rcv_nxt.wrapping_add(1);

        // Send ACK
        self.send_ack()?;

        // State transition based on current state
        match self.state {
            TcpState::TCP_ESTABLISHED => {
                self.state = TcpState::TCP_CLOSE_WAIT;
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
            TcpState::TCP_SYN_RECV => {
                // If ACK is acceptable, abort connection
                self.state = TcpState::TCP_CLOSE;
            }
            TcpState::TCP_ESTABLISHED
            | TcpState::TCP_FIN_WAIT1
            | TcpState::TCP_FIN_WAIT2
            | TcpState::TCP_CLOSE_WAIT => {
                // Abort connection
                self.state = TcpState::TCP_CLOSE;
                // Clear buffers
                self.send_buffer.clear();
                self.recv_buffer.clear();
                self.retrans_queue.clear();
            }
            TcpState::TCP_CLOSING
            | TcpState::TCP_LAST_ACK
            | TcpState::TCP_TIME_WAIT => {
                // In these states, just close — no error
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
    pub fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
        // Delegate to send_reliable so data goes through congestion control,
        // retransmit queue, and proper window management (fixes H38).
        self.send_reliable(data)
    }

    /// Receive data
    ///
    /// # Arguments
    /// - `buf`: Buffer
    /// - `len`: Buffer length
    pub fn recv(&mut self, buf: &mut [u8], _len: usize) -> Result<usize, ()> {
        // Allow reading in ESTABLISHED and CLOSE_WAIT (peer sent FIN but
        // data may still be buffered). Also FIN_WAIT1/FIN_WAIT2 for half-close.
        match self.state {
            TcpState::TCP_ESTABLISHED
            | TcpState::TCP_CLOSE_WAIT
            | TcpState::TCP_FIN_WAIT1
            | TcpState::TCP_FIN_WAIT2 => {}
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
            let _ = self.send_ack();
        }

        Ok(read)
    }

    /// Put data into receive buffer
    ///
    /// # Arguments
    /// - `data`: Received data
    pub fn enqueue_data(&mut self, data: &[u8]) {
        for &byte in data {
            self.recv_buffer.push_back(byte);
        }
    }

    /// Close connection
    pub fn close(&mut self) {
        match self.state {
            TcpState::TCP_ESTABLISHED => {
                self.state = TcpState::TCP_FIN_WAIT1;
                let _ = self.send_fin();
            }
            TcpState::TCP_CLOSE_WAIT => {
                self.state = TcpState::TCP_LAST_ACK;
                let _ = self.send_fin();
            }
            _ => {
                self.state = TcpState::TCP_CLOSE;
            }
        }
    }

    // ========== Reliable transmission methods ==========

    /// Reliable send data
    ///
    /// Puts data into send buffer and attempts to send, supports retransmission
    ///
    /// # Arguments
    /// - `data`: Data to send
    ///
    /// # Returns
    /// Bytes sent on success, Err(()) on failure
    pub fn send_reliable(&mut self, data: &[u8]) -> Result<usize, ()> {
        if self.state != TcpState::TCP_ESTABLISHED {
            return Err(());
        }

        if data.is_empty() {
            return Ok(0);
        }

        // Put data into send buffer
        for &byte in data {
            self.send_buffer.push_back(byte);
        }

        // Try to send data
        self.tx_packets()?;

        Ok(data.len())
    }

    /// Send packets (core send logic)
    ///
    /// Takes data from send buffer, builds TCP segments and sends
    /// Limited by congestion window and receive window
    pub fn tx_packets(&mut self) -> Result<(), ()> {
        // Calculate in-flight data
        let in_flight = self.snd_nxt.wrapping_sub(self.snd_una);

        // Calculate usable window: min(snd_wnd, cwnd) - in_flight
        let usable_window = core::cmp::min(self.snd_wnd as u32, self.congestion.cwnd)
            .saturating_sub(in_flight as u32);

        if usable_window == 0 {
            return Ok(()); // Window full, wait
        }

        let now = crate::drivers::timer::get_jiffies();

        while !self.send_buffer.is_empty() && usable_window > 0 {
            // Calculate this send size
            let seg_size = core::cmp::min(
                core::cmp::min(self.mss as usize, usable_window as usize),
                self.send_buffer.len()
            );

            if seg_size == 0 {
                break;
            }

            // Extract data
            let mut seg_data = alloc::vec::Vec::with_capacity(seg_size);
            for _ in 0..seg_size {
                if let Some(byte) = self.send_buffer.pop_front() {
                    seg_data.push(byte);
                }
            }

            // Send TCP segment
            self.tx_segment(&seg_data)?;

            // Add segment to retransmit queue
            let seg = TcpSendSeg::new(self.snd_nxt, &seg_data, now);
            self.retrans_queue.push_back(seg);

            // Update sequence number
            self.snd_nxt = self.snd_nxt.wrapping_add(seg_size as u32);
        }

        // Start retransmit timer
        if !self.retrans_queue.is_empty() && self.timers.retransmit_deadline == 0 {
            self.start_retransmit_timer();
        }

        Ok(())
    }

    /// Send single TCP segment
    fn tx_segment(&self, data: &[u8]) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        // Add data
        skb.skb_put_data(data)?;

        // Build TCP header (data already added above)
        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            0x0018, // PSH + ACK
            self.rcv_wnd,
            self.local_ip.to_be(),
            self.remote_ip.to_be(),
        )?;

        // Send to IP layer
        crate::net::ipv4::ipv4_send_src(skb, self.local_ip, self.remote_ip, 6)?;

        Ok(())
    }

    /// Process ACK acknowledgment
    ///
    /// When ACK is received, update send window, RTT estimate, congestion control
    pub fn process_ack(&mut self, ack: TcpSeq) -> bool {
        // Check ACK sequence number
        if self.seq_before(ack, self.snd_una) {
            // Old ACK, might be duplicate ACK
            self.congestion.on_dup_ack(ack, self.snd_nxt, self.mss);
            if self.congestion.dup_ack_count >= 3 {
                // Fast retransmit
                self.fast_retransmit();
            }
            return false;
        }

        if self.seq_after(ack, self.snd_nxt) {
            // ACK exceeds sent data, ignore
            return false;
        }

        // Calculate acknowledged bytes
        let acked_bytes = ack.wrapping_sub(self.snd_una);

        if acked_bytes > 0 {
            // New ACK
            // 1. Remove acknowledged segments, capture tx_time of last acked seg
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
        }
        true
    }

    /// Remove acknowledged segments from retransmit queue.
    /// Returns the `tx_time` of the last fully-acknowledged segment (for RTT sampling).
    fn remove_acked_segments(&mut self, ack: TcpSeq) -> Option<u64> {
        let mut last_tx_time: Option<u64> = None;
        // Drain segments that are fully covered by the ACK.
        while let Some(seg) = self.retrans_queue.front() {
            let seg_end = seg.seq.wrapping_add(seg.len as u32);
            // Sequence comparison: seg_end before (or equal to) ack
            if ((seg_end as i32) - (ack as i32)) <= 0 {
                last_tx_time = Some(seg.tx_time);
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
    fn fast_retransmit(&mut self) {
        if let Some(seg) = self.retrans_queue.front() {
            // Retransmit earliest segment
            let _ = self.tx_segment(&seg.data);
        }
    }

    /// Start retransmit timer
    fn start_retransmit_timer(&mut self) {
        self.timers.start_retransmit(self.rtt_estimator.rto);
    }

    /// Retransmit timer expired handling
    ///
    /// Called by TCP timer tick
    pub fn retransmit_timer_expired(&mut self) {
        // Check retransmit queue
        if self.retrans_queue.is_empty() {
            self.timers.stop_retransmit();
            return;
        }

        // First get needed info to avoid borrow conflicts
        let should_close;
        let data_to_retransmit;

        {
            if let Some(seg) = self.retrans_queue.front_mut() {
                if seg.retries >= TCP_MAX_RETRIES {
                    // Exceeded maximum retransmit count, close connection
                    should_close = true;
                    data_to_retransmit = None;
                } else {
                    should_close = false;
                    // Copy data for retransmission
                    data_to_retransmit = Some(seg.data.clone());
                    // Increment retransmit count
                    seg.retries += 1;
                }
            } else {
                return;
            }
        }

        if should_close {
            self.state = TcpState::TCP_CLOSE;
            self.timers.stop_retransmit();
            return;
        }

        // Congestion control: timeout handling
        self.congestion.on_timeout(self.mss);

        // Retransmit
        if let Some(data) = data_to_retransmit {
            let _ = self.tx_segment(&data);
        }

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
        // Receive window = buffer size - used space
        let used = self.recv_buffer.len() as u16;
        self.rcv_wnd = TCP_MAX_WINDOW.saturating_sub(used);
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
    pub fn handle_tcp_packet(&mut self, skb: &SkBuff, src_ip: u32, dest_ip: u32) -> Result<(), ()> {
        // Parse TCP header
        let tcp_hdr = match tcp_parse_packet(skb) {
            Some(hdr) => hdr,
            None => return Ok(()),
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
                    let payload = match tcp_payload_slice(skb, tcp_hdr.header_len()) {
                        Some(p) => p,
                        None => return Ok(()),
                    };
                    let _ = socket.handle_packet(tcp_hdr, payload);
                    return Ok(());
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
                        return Ok(()); // drop the SYN
                    }

                    let slot = match table.alloc_slot() {
                        Ok(s) => s,
                        Err(_) => return Ok(()),
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
                    let _ = new_socket.handle_packet(tcp_hdr, &[]);
                    let _ = table.install(slot, new_socket);
                    return Ok(());
                }
            }
        }

        // No matching connection found
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
}

/// Global TCP socket table
static mut TCP_SOCKET_TABLE: TcpSocketTable = TcpSocketTable::new();

/// Allocate TCP socket
///
/// # Returns
/// Socket file descriptor
pub fn tcp_socket_alloc() -> Result<i32, i32> {
    // SAFETY: TCP_SOCKET_TABLE is a global static; no concurrent mutation hazard
    // in current single-core kernel context.
    unsafe {
        match TCP_SOCKET_TABLE.alloc() {
            Ok(fd) => Ok(fd as i32),
            Err(_) => Err(-5), // EIO
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
    // SAFETY: TCP_SOCKET_TABLE is a global static; fd was returned by tcp_socket_alloc.
    unsafe {
        if let Some(socket) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            match socket.bind(port) {
                Ok(()) => 0,
                Err(()) => -5, // EIO
            }
        } else {
            -5 // EBADF
        }
    }
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
                return port;
            }
        }
        0
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
            match socket.connect(ip, port) {
                Ok(()) => 0,
                Err(()) => -5, // EIO
            }
        } else {
            -5 // EBADF
        }
    }
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
        for i in 0..TCP_SOCKET_TABLE.count {
            if let Some(socket) = TCP_SOCKET_TABLE.sockets[i].as_mut() {
                if socket.parent_fd == Some(fd)
                    && !socket.accepted
                    && socket.state == TcpState::TCP_ESTABLISHED
                {
                    socket.accepted = true;
                    return i as i32;
                }
            }
        }

        -11 // EAGAIN (no completed connections)
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
) -> Result<(), ()> {
    // Allocate space for TCP header
    let ptr = skb.skb_push(TCP_MIN_HLEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // TCP_MIN_HLEN bytes; writing fields of repr(C) TcpHdr is well-defined.
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

        // Data offset (20 bytes = 5 32-bit words)
        tcp_hdr.set_dof(5);

        // Window size
        tcp_hdr.set_window(window);

        // Flags
        tcp_hdr.flags = (flags & 0xFF) as u8;

        // Checksum (set to 0 first, calculate later)
        tcp_hdr.check = 0;

        // Urgent pointer
        tcp_hdr.urg_ptr = 0;

        // Compute TCP checksum (RFC 793). The field is big-endian on the
        // wire — store the network-order value (review NEW: missing .to_be()
        // byte-swapped every outbound segment's checksum).
        let data_ptr = ptr.add(TCP_MIN_HLEN);
        let data_len = (skb.len as usize).saturating_sub(TCP_MIN_HLEN);
        let data_slice = core::slice::from_raw_parts(data_ptr as *const u8, data_len);
        tcp_hdr.check = tcp_checksum(src_ip, dest_ip, tcp_hdr, data_slice).to_be();
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

    Some(tcp_hdr)
}

/// Receive and process TCP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing TCP packet)
/// - `src_ip`: Source IP address
/// - `dest_ip`: Destination IP address
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn tcp_rcv(skb: &SkBuff, src_ip: u32, dest_ip: u32) -> Result<(), ()> {
    let manager = get_tcp_manager();

    match manager.handle_tcp_packet(skb, src_ip, dest_ip) {
        Ok(()) => Ok(()),
        Err(()) => {
            // No matching connection found — send RST (RFC 793 §3.9)
            let tcp_hdr = match tcp_parse_packet(skb) {
                Some(hdr) => hdr,
                None => return Ok(()),
            };
            if !tcp_hdr.rst() && dest_ip != 0xFFFFFFFF {
                let _ = tcp_send_reset(src_ip, dest_ip, tcp_hdr);
            }
            Ok(())
        }
    }
}

/// Send RST in response to segment for non-existing connection
fn tcp_send_reset(src_ip: u32, dest_ip: u32, tcp_hdr: &TcpHdr) -> Result<(), ()> {
    let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

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

    tcp_build_packet(
        &mut skb,
        TcpPort::from_be(tcp_hdr.dest),
        TcpPort::from_be(tcp_hdr.source),
        rst_seq,
        rst_ack,
        0x0014, // RST + ACK
        TCP_MAX_WINDOW,
        dest_ip.to_be(),
        src_ip.to_be(),
    )?;

    crate::net::ipv4::ipv4_send(skb, src_ip, 6);
    Ok(())
}

/// Handle ICMP error for a TCP connection (soft error)
///
/// Called when ICMP destination unreachable or time exceeded is received
/// for a packet that matches one of our TCP connections.
///
/// Records a soft error — does not abort the connection, but the next
/// send/receive operation will detect the failure.
pub fn tcp_v4_err(
    icmp_type: u8,
    icmp_code: u8,
    src_ip: u32,
    src_port: u16,
    dest_ip: u32,
    dest_port: u16,
) {
    // Look up matching connection in the global socket table (single
    // source of truth — the manager's established list never held client
    // connections; review NET-C4/tcp_v4_err).
    // SAFETY: single-core softirq/syscall serialization (review NET-M15).
    unsafe {
        for i in 0..TCP_SOCKET_TABLE.count {
            let socket = match TCP_SOCKET_TABLE.sockets[i].as_mut() {
                Some(s) => s,
                None => continue,
            };
            if socket.local_port == dest_port
                && socket.remote_port == src_port
                && socket.remote_ip == src_ip
            {
                match icmp_type {
                crate::net::icmp::icmp_type::DEST_UNREACH => {
                    // Abort the connection on host/port unreachable
                    match icmp_code {
                        crate::net::icmp::icmp_code::HOST_UNREACH
                        | crate::net::icmp::icmp_code::PORT_UNREACH
                        | crate::net::icmp::icmp_code::NET_UNREACH => {
                            socket.state = TcpState::TCP_CLOSE;
                        }
                        _ => {
                            // FRAG_NEEDED etc. — just record, don't abort
                        }
                    }
                }
                crate::net::icmp::icmp_type::TIME_EXCEEDED => {
                    // TTL expired — abort
                    socket.state = TcpState::TCP_CLOSE;
                }
                _ => {}
            }
            return;
            }
        }
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
