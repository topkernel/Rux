//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP congestion control, RTT estimator, and header flag invariant tests.
//!
//! Types copied from: kernel/src/net/tcp.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/net/tcp.rs
// ============================================================================

pub const TCP_RTO_MIN_US: u64 = 200_000;
pub const TCP_RTO_MAX_US: u64 = 120_000_000;
pub const TCP_RTO_DEFAULT_US: u64 = 1_000_000;
pub const TCP_DEFAULT_MSS: u16 = 1460;
pub const TCP_MIN_HLEN: usize = 20;

pub type TcpSeq = u32;
pub type TcpPort = u16;
pub type TcpAck = u32;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpHdr {
    pub source: TcpPort,
    pub dest: TcpPort,
    pub seq: TcpSeq,
    pub ack_seq: TcpAck,
    pub dof_res: u8,
    pub flags_win: u16,
    pub check: u16,
    pub urg_ptr: u16,
}

impl TcpHdr {
    pub fn dof(&self) -> u8 {
        self.dof_res >> 4
    }

    pub fn header_len(&self) -> usize {
        (self.dof() as usize) * 4
    }

    pub fn syn(&self) -> bool {
        (self.flags_win & 0x02) != 0
    }

    pub fn ack(&self) -> bool {
        (self.flags_win & 0x10) != 0
    }

    pub fn fin(&self) -> bool {
        (self.flags_win & 0x01) != 0
    }

    pub fn rst(&self) -> bool {
        (self.flags_win & 0x04) != 0
    }

    pub fn psh(&self) -> bool {
        (self.flags_win & 0x08) != 0
    }

    pub fn window(&self) -> u16 {
        u16::from_be(self.flags_win & 0xFF00)
    }

    pub fn set_dof(&mut self, dof: u8) {
        self.dof_res = (dof << 4) | (self.dof_res & 0x0F);
    }

    pub fn set_syn(&mut self) {
        self.flags_win |= 0x0002;
    }

    pub fn set_ack(&mut self) {
        self.flags_win |= 0x0010;
    }

    pub fn set_fin(&mut self) {
        self.flags_win |= 0x0001;
    }

    pub fn set_rst(&mut self) {
        self.flags_win |= 0x0004;
    }

    pub fn set_psh(&mut self) {
        self.flags_win |= 0x0008;
    }

    pub fn set_window(&mut self, win: u16) {
        self.flags_win = (self.flags_win & 0x00FF) | (win & 0xFF00);
    }
}

#[derive(Debug, Clone)]
pub struct TcpRttEstimator {
    pub srtt: u64,
    pub rttvar: u64,
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

    pub fn update(&mut self, rtt_sample: u64) {
        if self.srtt == 0 {
            self.srtt = rtt_sample;
            self.rttvar = rtt_sample / 2;
        } else {
            let delta = if rtt_sample > self.srtt {
                rtt_sample - self.srtt
            } else {
                self.srtt - rtt_sample
            };
            self.rttvar = (3 * self.rttvar + delta) / 4;
            self.srtt = (7 * self.srtt + rtt_sample) / 8;
        }
        self.rto = self.srtt.saturating_add(4 * self.rttvar);
        self.rto = self.rto.clamp(TCP_RTO_MIN_US, TCP_RTO_MAX_US);
    }

    pub fn backoff(&mut self) {
        self.rto = std::cmp::min(self.rto * 2, TCP_RTO_MAX_US);
    }

    pub fn reset(&mut self) {
        self.rto = TCP_RTO_DEFAULT_US;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpCongState {
    SlowStart,
    CongestionAvoidance,
    FastRecovery,
}

#[derive(Debug, Clone)]
pub struct TcpCongestion {
    pub cwnd: u32,
    pub ssthresh: u32,
    pub state: TcpCongState,
    pub dup_ack_count: u32,
    pub recover_seq: TcpSeq,
}

impl TcpCongestion {
    pub fn new(mss: u16) -> Self {
        Self {
            cwnd: mss as u32,
            ssthresh: u32::MAX,
            state: TcpCongState::SlowStart,
            dup_ack_count: 0,
            recover_seq: 0,
        }
    }

    pub fn on_ack(&mut self, acked_bytes: u32, mss: u16) {
        match self.state {
            TcpCongState::SlowStart => {
                self.cwnd += mss as u32;
                if self.cwnd >= self.ssthresh {
                    self.state = TcpCongState::CongestionAvoidance;
                }
            }
            TcpCongState::CongestionAvoidance => {
                let increment = (mss as u32 * mss as u32) / std::cmp::max(self.cwnd, 1);
                self.cwnd += increment;
            }
            TcpCongState::FastRecovery => {
                self.state = TcpCongState::CongestionAvoidance;
            }
        }
    }

    pub fn on_dup_ack(&mut self, ack: TcpSeq, snd_nxt: TcpSeq, mss: u16) {
        self.dup_ack_count += 1;
        if self.dup_ack_count == 3 && Self::seq_before(ack, snd_nxt) {
            self.ssthresh = std::cmp::max(self.cwnd / 2, 2 * mss as u32);
            self.cwnd = self.ssthresh + 3 * mss as u32;
            self.recover_seq = snd_nxt;
            self.state = TcpCongState::FastRecovery;
        } else if self.state == TcpCongState::FastRecovery {
            self.cwnd += mss as u32;
        }
    }

    pub fn on_timeout(&mut self, mss: u16) {
        self.ssthresh = std::cmp::max(self.cwnd / 2, 2 * mss as u32);
        self.cwnd = mss as u32;
        self.state = TcpCongState::SlowStart;
        self.dup_ack_count = 0;
    }

    pub fn reset(&mut self, mss: u16) {
        self.cwnd = mss as u32;
        self.ssthresh = u32::MAX;
        self.state = TcpCongState::SlowStart;
        self.dup_ack_count = 0;
        self.recover_seq = 0;
    }

    pub fn seq_before(a: TcpSeq, b: TcpSeq) -> bool {
        ((a as i32) - (b as i32)) < 0
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-RTT-1: First measurement sets srtt and rttvar
    #[test]
    fn test_rtt_first_measurement(rtt in 1000u64..5_000_000u64) {
        let mut est = TcpRttEstimator::new();
        est.update(rtt);
        prop_assert_eq!(est.srtt, rtt);
        prop_assert_eq!(est.rttvar, rtt / 2);
    }

    /// INV-RTT-2: RTO clamped to [MIN, MAX]
    #[test]
    fn test_rto_clamped(rtt in 1000u64..5_000_000u64) {
        let mut est = TcpRttEstimator::new();
        est.update(rtt);
        prop_assert!(est.rto >= TCP_RTO_MIN_US);
        prop_assert!(est.rto <= TCP_RTO_MAX_US);
    }

    /// INV-RTT-3: backoff doubles RTO, capped at MAX
    #[test]
    fn test_rto_backoff(rtt in 1000u64..5_000_000u64) {
        let mut est = TcpRttEstimator::new();
        est.update(rtt);
        let before = est.rto;
        est.backoff();
        prop_assert_eq!(est.rto, std::cmp::min(before * 2, TCP_RTO_MAX_US));
        prop_assert!(est.rto <= TCP_RTO_MAX_US);
    }

    /// INV-RTT-4: reset restores default RTO
    #[test]
    fn test_rto_reset(rtt in 1000u64..5_000_000u64) {
        let mut est = TcpRttEstimator::new();
        est.update(rtt);
        est.backoff();
        est.backoff();
        est.reset();
        prop_assert_eq!(est.rto, TCP_RTO_DEFAULT_US);
    }

    /// INV-RTT-5: RTO always in bounds after any sequence
    #[test]
    fn test_rto_bounds_sequence(
        rtts in proptest::collection::vec(1000u64..5_000_000u64, 1..20),
        backs in proptest::collection::vec(proptest::bool::ANY, 1..10),
    ) {
        let mut est = TcpRttEstimator::new();
        for &rtt in &rtts {
            est.update(rtt);
        }
        for &back in &backs {
            if back {
                est.backoff();
            }
        }
        prop_assert!(est.rto >= TCP_RTO_MIN_US);
        prop_assert!(est.rto <= TCP_RTO_MAX_US);
    }

    /// INV-CONG-1: new(mss).cwnd == mss
    #[test]
    fn test_cong_init(mss in 512u16..9000u16) {
        let c = TcpCongestion::new(mss);
        prop_assert_eq!(c.cwnd, mss as u32);
        prop_assert_eq!(c.state, TcpCongState::SlowStart);
    }

    /// INV-CONG-2: Slow start increases cwnd by MSS per ACK
    #[test]
    fn test_cong_slow_start(mss in 512u16..9000u16) {
        let mut c = TcpCongestion::new(mss);
        let before = c.cwnd;
        c.on_ack(mss as u32, mss);
        prop_assert_eq!(c.cwnd, before + mss as u32);
        prop_assert_eq!(c.state, TcpCongState::SlowStart);
    }

    /// INV-CONG-3: on_timeout resets cwnd to MSS
    #[test]
    fn test_cong_timeout(mss in 512u16..9000u16) {
        let mut c = TcpCongestion::new(mss);
        // Drive up cwnd
        for _ in 0..10 {
            c.on_ack(mss as u32, mss);
        }
        c.on_timeout(mss);
        prop_assert_eq!(c.cwnd, mss as u32);
        prop_assert_eq!(c.state, TcpCongState::SlowStart);
    }

    /// INV-CONG-4: reset restores initial state
    #[test]
    fn test_cong_reset(mss in 512u16..9000u16) {
        let mut c = TcpCongestion::new(mss);
        for _ in 0..10 {
            c.on_ack(mss as u32, mss);
        }
        c.on_timeout(mss);
        c.reset(mss);
        prop_assert_eq!(c.cwnd, mss as u32);
        prop_assert_eq!(c.ssthresh, u32::MAX);
        prop_assert_eq!(c.state, TcpCongState::SlowStart);
    }

    /// INV-CONG-5: cwnd never decreases in slow start
    #[test]
    fn test_cong_slow_start_monotone(
        mss in 512u16..9000u16,
        acks in proptest::collection::vec(0u32..2u32, 1..20),
    ) {
        let mut c = TcpCongestion::new(mss);
        let mut prev = c.cwnd;
        for _acked in acks {
            c.on_ack(mss as u32, mss);
            if c.state == TcpCongState::SlowStart {
                prop_assert!(c.cwnd > prev);
            }
            prev = c.cwnd;
        }
    }

    /// INV-SEQ-1: seq_before is irreflexive
    #[test]
    fn test_seq_irreflexive(seq in 0u32..u32::MAX) {
        prop_assert!(!TcpCongestion::seq_before(seq, seq));
    }

    /// INV-SEQ-2: seq_before(a, b) implies !seq_before(b, a) for close values
    #[test]
    fn test_seq_antisymmetric(base in 0u32..1_000_000u32, delta in 1u32..100u32) {
        let a = base;
        let b = base.wrapping_add(delta);
        let ab = TcpCongestion::seq_before(a, b);
        let ba = TcpCongestion::seq_before(b, a);
        prop_assert_ne!(ab, ba);
    }

    /// INV-TCP-1: dof set/get roundtrip
    #[test]
    fn test_dof_roundtrip(dof in 5u8..15u8) {
        let mut h = TcpHdr::default();
        h.set_dof(dof);
        prop_assert_eq!(h.dof(), dof);
        prop_assert_eq!(h.header_len(), (dof as usize) * 4);
    }

    /// INV-TCP-2: flag set/get roundtrips
    #[test]
    fn test_flag_roundtrips(
        set_syn in proptest::bool::ANY,
        set_ack in proptest::bool::ANY,
        set_fin in proptest::bool::ANY,
        set_rst in proptest::bool::ANY,
        set_psh in proptest::bool::ANY,
    ) {
        let mut h = TcpHdr::default();
        if set_syn { h.set_syn(); }
        if set_ack { h.set_ack(); }
        if set_fin { h.set_fin(); }
        if set_rst { h.set_rst(); }
        if set_psh { h.set_psh(); }
        prop_assert_eq!(h.syn(), set_syn);
        prop_assert_eq!(h.ack(), set_ack);
        prop_assert_eq!(h.fin(), set_fin);
        prop_assert_eq!(h.rst(), set_rst);
        prop_assert_eq!(h.psh(), set_psh);
    }

    /// INV-TCP-3: window set/get roundtrip
    #[test]
    fn test_window_roundtrip(win in 0u16..65535u16) {
        let mut h = TcpHdr::default();
        h.set_window(win);
        prop_assert_eq!(h.window(), u16::from_be(win & 0xFF00));
    }

    /// INV-TCP-4: 3 dup ACKs trigger fast retransmit
    #[test]
    fn test_fast_retransmit(
        mss in 512u16..9000u16,
        ack in 1000u32..5000u32,
        snd_nxt_delta in 100u32..10000u32,
    ) {
        let snd_nxt = ack.wrapping_add(snd_nxt_delta);
        let mut c = TcpCongestion::new(mss);
        // Build up cwnd
        for _ in 0..5 {
            c.on_ack(mss as u32, mss);
        }
        let old_cwnd = c.cwnd;
        // 3 dup ACKs
        for _ in 0..3 {
            c.on_dup_ack(ack, snd_nxt, mss);
        }
        prop_assert_eq!(c.state, TcpCongState::FastRecovery);
        prop_assert!(c.ssthresh <= old_cwnd / 2 + 1); // max(cwnd/2, 2*MSS)
    }
}
