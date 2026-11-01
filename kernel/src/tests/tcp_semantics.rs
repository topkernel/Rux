//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP semantics regression suite — pins the 2026-10-31 RFC-semantics
//! sweep of net/tcp.rs (review findings 1/2/3/4/5/8/9/10 and the
//! graceful-close fix) at the state-machine level, plus the two
//! follow-on regressions the suite's first run caught:
//!
//!  - the finding-8 by-value `TcpHdr` refactor silently broke SYN
//!    option parsing (the option parsers kept reading past the header
//!    POINTER, which after the refactor is a stack copy, not the
//!    packet) — options now travel as an skb-backed slice;
//!  - the finding-10 OOO trim inverted its arithmetic
//!    (`data[len-cut]` instead of `data[cut]`), dropping stream bytes
//!    of any retransmission straddling rcv_nxt.
//!
//! Everything here drives `TcpSocket::handle_packet`/`process_ack`
//! directly with crafted segments — the same call the RX softirq makes
//! — so reorder/duplicate/retransmit scenarios that the loopback wire
//! cannot produce are exact and deterministic.
//!
//! Kernel layer policy (docs/architecture/kernel-layers.md):
//! this module must stay Safe Rust.
#![forbid(unsafe_code)]

use crate::net::tcp::{
    TcpCongState, TcpHdr, TcpSocket, TcpState, TcpTxBatch, TCP_DEFAULT_MSS, TCP_MAX_WINDOW,
    TCP_MIN_HLEN, tcp_parse_mss, tcp_parse_tsval, tcp_parse_wscale,
};
use super::test_group_start;

// TCP header flag bits (wire byte 13).
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

/// Build a 20-byte (no options) header. `seq`/`ack` are HOST-order
/// values — the header fields carry them big-endian exactly like the
/// RX path produces, and every consumer converts with `from_be`.
fn hdr(seq: u32, ack: u32, flags: u8, window: u16) -> TcpHdr {
    let mut h = TcpHdr::default();
    h.seq = seq.to_be();
    h.ack_seq = ack.to_be();
    h.set_window(window);
    h.dof_res = 5 << 4; // data offset 5 words = 20 bytes
    h.flags = flags;
    h
}

/// A reserved TX batch (tests never emit; capacity only keeps the
/// deferred-record paths from degrading).
fn tx_batch() -> TcpTxBatch {
    let mut tx = TcpTxBatch::new();
    tx.reserve(8, 2 * TCP_DEFAULT_MSS as usize);
    tx
}

/// ESTABLISHED socket with known sequence state:
/// snd_una = snd_nxt = 1000 (nothing in flight), rcv_nxt = 2000.
fn established() -> TcpSocket {
    let mut s = TcpSocket::new();
    s.bind(40000).unwrap();
    s.state = TcpState::TCP_ESTABLISHED;
    s.snd_una = 1000;
    s.snd_nxt = 1000;
    s.rcv_nxt = 2000;
    s.snd_wnd = 8192;
    s
}

/// One retransmit-queue segment filled with pattern bytes.
fn rseg(seq: u32, len: usize, retries: u32, tx_time: u64) -> crate::net::tcp::TcpSendSeg {
    let mut data = alloc::vec::Vec::new();
    data.resize(len, 0xA5);
    crate::net::tcp::TcpSendSeg { seq, len, data, tx_time, retries }
}

pub fn test_tcp_semantics() {
    test_group_start("TCP semantics");

    test_rst_sequence_validation();
    test_rst_syn_sent_validation();
    test_fin_sequence_gating();
    test_fast_retransmit_single_shot();
    test_karn_filter();
    test_partial_ack_trim();
    test_window_update_gate();
    test_ooo_queue_caps();
    test_ooo_straddle_trim();
    test_close_drains_before_fin();
    test_syn_option_gate();
    test_option_parsers();
    test_unaligned_header_parse();
}

// --------------------------------------------------------------------------
// Finding 1: RST sequence validation (RFC 793 §3.9 / RFC 1122 §4.2.3.30)
// --------------------------------------------------------------------------

fn test_rst_sequence_validation() {
    // Out-of-window RST on an ESTABLISHED connection must be dropped
    // silently: rcv_nxt=2000, rcv_wnd=4096, RST at seq=2000+8192.
    let mut s = established();
    s.rcv_wnd = 4096;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2000 + 8192, 0, RST, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_ESTABLISHED && s.pending_error == 0,
        "tcp-rst out-of-window RST ignored"
    );

    // RST at the window edge (rcv_nxt-1) IS acceptable → ECONNRESET.
    let mut s = established();
    s.rcv_wnd = 4096;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2000 - 1, 0, RST, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_CLOSE && s.pending_error == 104,
        "tcp-rst in-window RST resets (ECONNRESET)"
    );

    // RST at rcv_nxt itself (in-window) also resets.
    let mut s = established();
    s.rcv_wnd = 4096;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2000, 0, RST, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_CLOSE && s.pending_error == 104,
        "tcp-rst RST at rcv_nxt resets"
    );

    // Buffers are dropped on the accepted reset.
    let mut s = established();
    s.rcv_wnd = 4096;
    s.send_buffer.extend(core::iter::repeat(0u8).take(64));
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2000, 0, RST, 8192), &[], &[], &mut tx);
    test_assert!(
        s.send_buffer.is_empty() && s.retrans_queue.is_empty(),
        "tcp-rst reset clears send/retrans buffers"
    );
}

/// SYN_SENT: an RST is acceptable only when its ACK acknowledges our SYN
/// (SEG.ACK == SND.NXT). Everything else is a blind/stale reset.
fn test_rst_syn_sent_validation() {
    // RST without ACK — not acceptable.
    let mut s = established();
    s.state = TcpState::TCP_SYN_SENT;
    s.snd_nxt = 500;
    s.snd_una = 500;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(1, 0, RST, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_SYN_SENT,
        "tcp-rst SYN_SENT RST without ACK ignored"
    );

    // RST+ACK with the wrong ack_seq — not acceptable.
    let mut s = established();
    s.state = TcpState::TCP_SYN_SENT;
    s.snd_nxt = 500;
    s.snd_una = 500;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(1, 999, RST | ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_SYN_SENT,
        "tcp-rst SYN_SENT RST with wrong ACK ignored"
    );

    // RST+ACK acknowledging our SYN → ECONNREFUSED (the slirp refuse
    // path; R34 pinned that this aborts instead of ping-ponging).
    let mut s = established();
    s.state = TcpState::TCP_SYN_SENT;
    s.snd_nxt = 500;
    s.snd_una = 500;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(1, 501, RST | ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_CLOSE && s.pending_error == 111,
        "tcp-rst SYN_SENT valid RST aborts (ECONNREFUSED)"
    );
}

// --------------------------------------------------------------------------
// Finding 2: FIN sequence gating (RFC 793 §3.9 — FIN only at rcv_nxt)
// --------------------------------------------------------------------------

fn test_fin_sequence_gating() {
    // Out-of-order FIN (fin_seq past rcv_nxt): must NOT advance rcv_nxt
    // — the bytes it would swallow were never received.
    let mut s = established(); // rcv_nxt = 2000
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2200, 1000, FIN | ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_ESTABLISHED
            && s.rcv_nxt == 2000
            && s.recv_buffer.is_empty(),
        "tcp-fin out-of-order FIN drops without advancing rcv_nxt"
    );

    // In-order FIN riding on data: a straddling segment [1990, 2000)
    // with rcv_nxt = 1995 delivers its 5 new bytes, then its FIN (at
    // exactly rcv_nxt) consumes one seq → CLOSE_WAIT. (The FIN can
    // only sit at rcv_nxt when its data ENDS there — pure new data
    // ending past rcv_nxt makes the FIN out-of-order by definition.)
    let mut s = established();
    s.rcv_nxt = 1995;
    let mut tx = tx_batch();
    let mut data = [0u8; 10];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((1990 + i) & 0xff) as u8;
    }
    let _ = s.handle_packet(&hdr(1990, 1000, FIN | ACK | PSH, 8192), &[], &data, &mut tx);
    test_assert!(
        s.state == TcpState::TCP_CLOSE_WAIT
            && s.rcv_nxt == 2001
            && s.recv_buffer.len() == 5,
        "tcp-fin in-order FIN+data → CLOSE_WAIT, data kept"
    );
    let delivered: alloc::vec::Vec<u8> = s.recv_buffer.iter().copied().collect();
    let content_ok = delivered
        .iter()
        .enumerate()
        .all(|(i, &b)| b == ((1995 + i) & 0xff) as u8);
    test_assert!(
        content_ok && delivered.len() == 5,
        "tcp-fin FIN payload content intact"
    );

    // Retransmitted FIN in CLOSE_WAIT (fin_seq == rcv_nxt-1): re-ACK,
    // no state churn — the peer can finish its close.
    let mut s = established();
    s.state = TcpState::TCP_CLOSE_WAIT;
    s.rcv_nxt = 2001;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2000, 1000, FIN | ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_CLOSE_WAIT && s.rcv_nxt == 2001,
        "tcp-fin duplicate FIN in CLOSE_WAIT re-ACKed, state kept"
    );

    // FIN_WAIT2 + exact FIN → TIME_WAIT.
    let mut s = established();
    s.state = TcpState::TCP_FIN_WAIT2;
    s.rcv_nxt = 2000;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2000, 1000, FIN | ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_TIME_WAIT && s.rcv_nxt == 2001,
        "tcp-fin FIN_WAIT2 FIN at rcv_nxt → TIME_WAIT"
    );

    // FIN_WAIT2 + out-of-order FIN → dropped, still FIN_WAIT2.
    let mut s = established();
    s.state = TcpState::TCP_FIN_WAIT2;
    s.rcv_nxt = 2000;
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(2400, 1000, FIN | ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.state == TcpState::TCP_FIN_WAIT2 && s.rcv_nxt == 2000,
        "tcp-fin FIN_WAIT2 out-of-order FIN dropped"
    );
}

// --------------------------------------------------------------------------
// Finding 3: single-shot fast retransmit (RFC 5681 §3.2)
// --------------------------------------------------------------------------

/// Socket with snd_una=1000, snd_nxt=3000 and one 1000-byte segment in
/// flight; feed `n` pure duplicate ACKs (ack == snd_una, no payload).
fn test_fast_retransmit_single_shot() {
    let mut s = established();
    s.snd_nxt = 3000;
    s.retrans_queue.push_back(rseg(1000, 1000, 0, 0));

    let mut tx = tx_batch();
    let dup = hdr(1500, 1000, ACK, 8192);
    for _ in 0..3 {
        let _ = s.handle_packet(&dup, &[], &[], &mut tx);
    }
    test_assert!(
        s.congestion.dup_ack_count == 3
            && s.retrans_queue.front().map(|g| g.retries) == Some(1)
            && s.congestion.state == TcpCongState::FastRecovery
            && s.congestion.recover_seq == 3000,
        "tcp-fr 3rd dup-ACK fires fast retransmit once (retries 0→1)"
    );
    let cwnd_at_entry = s.congestion.cwnd;

    // Further duplicates only inflate cwnd — no second retransmission.
    for _ in 0..2 {
        let _ = s.handle_packet(&dup, &[], &[], &mut tx);
    }
    test_assert!(
        s.retrans_queue.front().map(|g| g.retries) == Some(1)
            && s.congestion.dup_ack_count == 5
            && s.congestion.cwnd > cwnd_at_entry,
        "tcp-fr dup-ACKs 4-5 inflate cwnd only (retries stays 1)"
    );

    // A duplicate ACK that CARRIES DATA is not a dup-ACK (RFC 5681:
    // the count needs a pure ACK) — the count must not move.
    let before = s.congestion.dup_ack_count;
    let payload = [9u8; 16];
    let _ = s.handle_packet(&hdr(1500, 1000, ACK | PSH, 8192), &[], &payload, &mut tx);
    test_assert!(
        s.congestion.dup_ack_count == before,
        "tcp-fr dup-ACK with payload not counted"
    );

    // A fresh ACK exits recovery and clears the counter.
    let _ = s.handle_packet(&hdr(1500, 3000, ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.congestion.dup_ack_count == 0
            && s.congestion.state == TcpCongState::CongestionAvoidance
            && s.retrans_queue.is_empty(),
        "tcp-fr fresh ACK exits recovery, queue drained"
    );
}

// --------------------------------------------------------------------------
// Finding 4a: Karn's algorithm — retransmitted segments are never an RTT
// sample (remove_acked_segments filters on retries == 0).
// --------------------------------------------------------------------------

fn test_karn_filter() {
    // Retransmitted front segment (retries > 0): full ACK must NOT
    // sample RTT even though its tx_time is ancient.
    let mut s = established();
    s.snd_nxt = 3000;
    s.retrans_queue.push_back(rseg(1000, 600, 2, 0)); // tx_time = boot
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(1500, 1600, ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.rtt_estimator.srtt == 0 && s.retrans_queue.is_empty() && s.snd_una == 1600,
        "tcp-karn retransmitted segment ACK samples no RTT"
    );

    // Control: never-retransmitted segment with the same ancient
    // tx_time DOES sample (boot jiffies are far in the past → srtt > 0).
    let mut s = established();
    s.snd_nxt = 3000;
    s.retrans_queue.push_back(rseg(1000, 600, 0, 0));
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(1500, 1600, ACK, 8192), &[], &[], &mut tx);
    test_assert!(
        s.rtt_estimator.srtt > 0 && s.snd_una == 1600,
        "tcp-karn fresh segment ACK samples RTT (control)"
    );
}

// --------------------------------------------------------------------------
// Finding 4b: partial ACK trims the covered head of the retransmit
// queue (reference tcp_trim_head) — the next RTO must not re-emit
// already-ACKed bytes.
// --------------------------------------------------------------------------

fn test_partial_ack_trim() {
    let mut s = established();
    s.snd_nxt = 3000;
    s.retrans_queue.push_back(rseg(1600, 400, 0, 0));
    s.retrans_queue.push_back(rseg(2000, 1000, 0, 0));
    s.snd_una = 1600;

    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(1500, 1800, ACK, 8192), &[], &[], &mut tx);

    let front = s.retrans_queue.front();
    test_assert!(
        s.snd_una == 1800
            && front.map(|g| g.seq) == Some(1800)
            && front.map(|g| g.len) == Some(200)
            && front.map(|g| g.data.len()) == Some(200),
        "tcp-partial-ack front segment trimmed to the un-ACKed tail"
    );
    // The second segment is untouched.
    test_assert!(
        s.retrans_queue.get(1).map(|g| g.seq) == Some(2000)
            && s.retrans_queue.get(1).map(|g| g.len) == Some(1000),
        "tcp-partial-ack later segments untouched"
    );
}

// --------------------------------------------------------------------------
// Finding 9: SND.WL1/SND.WL2 window-update gate (RFC 793 §3.9) — a
// reordered old ACK must not retract the send window.
// --------------------------------------------------------------------------

fn test_window_update_gate() {
    // Start: window last set by (seg_seq=1500, ack=1200), snd_wnd=8000.
    let mut s = established();
    s.snd_nxt = 3000;
    s.snd_wl1 = 1500;
    s.snd_wl2 = 1200;
    s.snd_wnd = 8000;

    // Reordered OLD ACK advertising w=0: SEG.SEQ < SND.WL1 → ignore.
    let mut tx = tx_batch();
    let _ = s.handle_packet(&hdr(1200, 1000, ACK, 0), &[], &[], &mut tx);
    test_assert!(
        s.snd_wnd == 8000 && s.snd_wl1 == 1500 && s.snd_wl2 == 1200,
        "tcp-wl1 reordered old ACK (w=0) does not retract snd_wnd"
    );

    // Newer segment (SEG.SEQ > SND.WL1) does update the window.
    let _ = s.handle_packet(&hdr(1600, 1000, ACK, 4000), &[], &[], &mut tx);
    test_assert!(
        s.snd_wnd == 4000 && s.snd_wl1 == 1600 && s.snd_wl2 == 1000,
        "tcp-wl1 newer ACK adopts window"
    );

    // Same SEG.SEQ with an OLDER ack (ack < SND.WL2) → ignore.
    // snd_una is still 1000 (all ACKs so far are duplicates).
    s.snd_wl2 = 1200;
    let _ = s.handle_packet(&hdr(1600, 1000, ACK, 2000), &[], &[], &mut tx);
    test_assert!(
        s.snd_wnd == 4000,
        "tcp-wl2 equal seq + older ack does not adopt window"
    );

    // Same SEG.SEQ with ack == SND.WL2 → adopt (>= is acceptable).
    let _ = s.handle_packet(&hdr(1600, 1200, ACK, 6000), &[], &[], &mut tx);
    test_assert!(
        s.snd_wnd == 6000 && s.snd_wl2 == 1200,
        "tcp-wl2 equal seq + equal ack adopts window"
    );
}

// --------------------------------------------------------------------------
// Finding 10: OOO queue caps — total OOO bytes bounded by the advertised
// receive window; duplicates below rcv_nxt are not (re-)buffered.
// --------------------------------------------------------------------------

fn test_ooo_queue_caps() {
    let mut s = established(); // snd side irrelevant here
    s.rcv_nxt = 1000; // stream position for this test
    // Shrink the advertised window the way the stack does: 60000 bytes
    // already buffered → rcv_wnd = 65535 - 60000 = 5535.
    for _ in 0..600 {
        s.recv_buffer.extend(core::iter::repeat(0u8).take(100));
    }
    s.update_rcv_wnd();
    test_assert!(
        s.rcv_wnd == TCP_MAX_WINDOW - 60000,
        "tcp-ooo rcv_wnd reflects buffered bytes"
    );

    let mut tx = tx_batch();
    // Two in-window OOO segments (2000 bytes each) fit under the cap.
    let a = [0x11u8; 2000];
    let _ = s.handle_packet(&hdr(1300, 0, ACK, 8192), &[], &a, &mut tx);
    let b = [0x22u8; 2000];
    let _ = s.handle_packet(&hdr(5000, 0, ACK, 8192), &[], &b, &mut tx);
    test_assert!(
        s.ooo_queue.len() == 2 && s.rcv_nxt == 1000,
        "tcp-ooo in-window OOO segments queued (2)"
    );

    // A third would take OOO bytes past rcv_wnd → dropped, not queued.
    let c = [0x33u8; 2000];
    let _ = s.handle_packet(&hdr(6300, 0, ACK, 8192), &[], &c, &mut tx);
    test_assert!(
        s.ooo_queue.len() == 2,
        "tcp-ooo byte cap drops the third OOO segment"
    );

    // Filling the gap delivers the coalescible head, leaves the far
    // OOO segment queued: 300 new + 2000 from OOO = 62300 buffered.
    let gap = [0x44u8; 300];
    let _ = s.handle_packet(&hdr(1000, 0, ACK | PSH, 8192), &[], &gap, &mut tx);
    test_assert!(
        s.rcv_nxt == 3300
            && s.recv_buffer.len() == 62300
            && s.ooo_queue.len() == 1
            && s.ooo_queue.front().map(|o| o.seq) == Some(5000),
        "tcp-ooo gap fill drains coalescible OOO head"
    );

    // A segment ENTIRELY below rcv_nxt (already delivered) is a pure
    // duplicate: re-ACK, nothing queued or delivered.
    let dup = [0x55u8; 100];
    let buffered = s.recv_buffer.len();
    let _ = s.handle_packet(&hdr(2000, 0, ACK, 8192), &[], &dup, &mut tx);
    test_assert!(
        s.ooo_queue.len() == 1 && s.recv_buffer.len() == buffered && s.rcv_nxt == 3300,
        "tcp-ooo fully-delivered duplicate not re-buffered"
    );
}

// --------------------------------------------------------------------------
// Finding 10 follow-up: the straddle trim. A retransmission covering
// [rcv_nxt-100, rcv_nxt+200) must deliver exactly the 200 new bytes —
// the first version of the trim inverted its arithmetic and silently
// dropped [rcv_nxt, seq+len-cut).
// --------------------------------------------------------------------------

fn test_ooo_straddle_trim() {
    let mut s = established(); // rcv_nxt = 2000
    let mut tx = tx_batch();

    // Segment bytes at stream position p carry value (p & 0xff).
    let mut seg = [0u8; 300];
    for (i, b) in seg.iter_mut().enumerate() {
        *b = ((1900 + i) & 0xff) as u8;
    }
    let _ = s.handle_packet(&hdr(1900, 0, ACK | PSH, 8192), &[], &seg, &mut tx);

    // The delivered prefix [1900, 2000) is trimmed; the remaining 200
    // bytes [2000, 2200) deliver in place (RFC 793 processes the
    // trimmed segment as in-order) — no OOO residue, no hole.
    test_assert!(
        s.rcv_nxt == 2200
            && s.recv_buffer.len() == 200
            && s.ooo_queue.is_empty(),
        "tcp-straddle trimmed remainder delivers in place"
    );
    let delivered: alloc::vec::Vec<u8> = s.recv_buffer.iter().copied().collect();
    let ok = delivered
        .iter()
        .enumerate()
        .all(|(i, &b)| b == ((2000 + i) & 0xff) as u8);
    test_assert!(
        ok && delivered.len() == 200,
        "tcp-straddle delivered bytes are the exact stream range"
    );
}

// --------------------------------------------------------------------------
// Graceful close (perf review finding 6): close() drains send_buffer
// before the FIN; under a closed window the FIN defers (fin_pending)
// and goes out behind the data once the window reopens.
// --------------------------------------------------------------------------

fn test_close_drains_before_fin() {
    // Zero peer window + queued data: FIN must defer.
    let mut s = established();
    s.snd_wnd = 0;
    s.snd_nxt = 1100;
    s.retrans_queue.push_back(rseg(1000, 100, 0, 0));
    s.send_buffer.extend(core::iter::repeat(0x77u8).take(300));

    let mut tx = tx_batch();
    s.close(&mut tx);
    test_assert!(
        s.state == TcpState::TCP_FIN_WAIT1
            && s.fin_pending
            && s.retrans_queue.len() == 1 // still the data segment, no FIN
            && s.send_buffer.len() == 300,
        "tcp-close FIN deferred behind queued data at zero window"
    );

    // Window reopens via an ACK that also frees the in-flight segment:
    // data goes out first, the FIN carries a sequence number AFTER it.
    let _ = s.handle_packet(&hdr(1500, 1100, ACK, 8192), &[], &[], &mut tx);
    let data_seg = s.retrans_queue.get(0);
    let fin_seg = s.retrans_queue.get(1);
    test_assert!(
        !s.fin_pending
            && s.send_buffer.is_empty()
            && data_seg.map(|g| (g.seq, g.len)) == Some((1100, 300))
            && fin_seg.map(|g| (g.seq, g.len, g.data.len())) == Some((1400, 1, 0))
            && s.snd_nxt == 1401,
        "tcp-close reopened window sends data then FIN (seq 1400)"
    );

    // Control: nothing queued → FIN goes out in close() itself.
    let mut s = established();
    s.snd_wnd = 8192;
    let mut tx = tx_batch();
    s.close(&mut tx);
    let fin = s.retrans_queue.back();
    test_assert!(
        s.state == TcpState::TCP_FIN_WAIT1
            && !s.fin_pending
            && s.retrans_queue.len() == 1
            && fin.map(|g| (g.len, g.data.len())) == Some((1, 0)),
        "tcp-close empty buffer sends FIN immediately"
    );
}

// --------------------------------------------------------------------------
// Finding 5: SYN options are negotiated at SYN time only. A
// retransmitted SYN-ACK reaching an ESTABLISHED socket must not reset
// peer_wscale/ts state.
//
// The packets are built as real wire buffers (header bytes + option
// bytes contiguous), parsed with TcpHdr::from_bytes and handed to
// handle_packet together with the option slice — the exact plumbing of
// the RX path, which pins the by-value-header fix (options must come
// from the buffer, never from memory past the header copy).
// --------------------------------------------------------------------------

/// Serialize a header + options into wire bytes (little-endian hosts,
/// which every supported arch is; the field values are stored BE, so
/// to_le_bytes of each field reproduces the packet layout the skb
/// path sees).
fn wire_packet(h: &TcpHdr, opts: &[u8]) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(20 + opts.len());
    v.extend_from_slice(&h.source.to_le_bytes());
    v.extend_from_slice(&h.dest.to_le_bytes());
    v.extend_from_slice(&h.seq.to_le_bytes());
    v.extend_from_slice(&h.ack_seq.to_le_bytes());
    v.push(h.dof_res);
    v.push(h.flags);
    v.extend_from_slice(&h.window.to_le_bytes());
    v.extend_from_slice(&h.check.to_le_bytes());
    v.extend_from_slice(&h.urg_ptr.to_le_bytes());
    v.extend_from_slice(opts);
    v
}

/// MSS 536 (kind 2), WS shift 7 (kind 3), TSval 0xDEADBEEF (kind 8).
fn syn_options() -> [u8; 20] {
    let mut o = [0u8; 20];
    o[0] = 2; o[1] = 4; o[2] = 0x02; o[3] = 0x18; // 536
    o[4] = 3; o[5] = 3; o[6] = 7; o[7] = 1; // WS 7 + NOP pad
    o[8] = 1; o[9] = 1; o[10] = 8; o[11] = 10;
    o[12..16].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
    o[16..20].copy_from_slice(&0x1234_5678u32.to_be_bytes());
    o
}

/// Drive handle_packet with a wire-format packet (header + options),
/// mirroring tcp_rcv's plumbing: parse by value, options from the
/// buffer.
fn with_packet<R>(
    s: &mut TcpSocket,
    pkt: &[u8],
    payload: &[u8],
    f: impl FnOnce(&mut TcpSocket, &TcpHdr, &[u8], &[u8]) -> R,
) -> R {
    let hdr = TcpHdr::from_bytes(pkt).expect("wire packet parses");
    let hlen = hdr.header_len().min(pkt.len());
    let opts: &[u8] = if hlen > TCP_MIN_HLEN {
        &pkt[TCP_MIN_HLEN..hlen]
    } else {
        &[]
    };
    f(s, &hdr, opts, payload)
}

fn test_syn_option_gate() {
    let opts = syn_options();

    // LISTEN: inbound SYN adopts MSS/WS/TS (negotiated at SYN time).
    let mut s = TcpSocket::new();
    s.bind(80).unwrap();
    s.listen(4).unwrap();
    let mut pkt = wire_packet(&hdr(9000, 0, SYN, 8192), &opts);
    pkt[12] = 10 << 4; // data offset 10 words: 20-byte header + 20-byte options
    let mut tx = tx_batch();
    let (mss, ws, ts_en, ts_recent, state) = with_packet(&mut s, &pkt, &[], |s, h, o, _| {
        let _ = s.handle_packet(h, o, &[], &mut tx);
        (s.mss, s.peer_wscale, s.ts_enabled, s.ts_recent, s.state)
    });
    test_assert!(
        state == TcpState::TCP_SYN_RECV
            && mss == 536
            && ws == 7
            && ts_en
            && ts_recent == 0xDEAD_BEEF,
        "tcp-synopts LISTEN adopts SYN options (MSS/WS/TS)"
    );

    // SYN_SENT: the SYN-ACK's options are adopted (also handshake).
    let mut s = established();
    s.state = TcpState::TCP_SYN_SENT;
    s.snd_nxt = 500;
    s.snd_una = 500;
    s.rcv_nxt = 9001;
    let mut pkt = wire_packet(&hdr(9000, 501, SYN | ACK, 8192), &opts);
    pkt[12] = 10 << 4;
    let mut tx = tx_batch();
    let (ws, ts_en, state) = with_packet(&mut s, &pkt, &[], |s, h, o, _| {
        let _ = s.handle_packet(h, o, &[], &mut tx);
        (s.peer_wscale, s.ts_enabled, s.state)
    });
    test_assert!(
        state == TcpState::TCP_ESTABLISHED && ws == 7 && ts_en,
        "tcp-synopts SYN_SENT adopts SYN-ACK options, establishes"
    );

    // ESTABLISHED (client past the handshake, our final ACK lost): the
    // peer's RETRANSMITTED SYN-ACK must not reset the negotiated state.
    let mut s = established();
    s.peer_wscale = 0;
    s.ts_enabled = false;
    s.ts_recent = 4711;
    let mss0 = s.mss;
    let mut pkt = wire_packet(&hdr(9000, 501, SYN | ACK, 8192), &opts);
    pkt[12] = 10 << 4;
    let mut tx = tx_batch();
    let (mss, ws, ts_en, ts_recent) = with_packet(&mut s, &pkt, &[], |s, h, o, _| {
        let _ = s.handle_packet(h, o, &[], &mut tx);
        (s.mss, s.peer_wscale, s.ts_enabled, s.ts_recent)
    });
    test_assert!(
        mss == mss0 && ws == 0 && !ts_en && ts_recent == 4711,
        "tcp-synopts retransmitted SYN-ACK in ESTABLISHED adopts nothing"
    );
}

// --------------------------------------------------------------------------
// Option parsers on wire-shaped option bytes (they take the skb-backed
// slice since the by-value header fix; garbage/bounded inputs must
// degrade to None, never read past the slice).
// --------------------------------------------------------------------------

fn test_option_parsers() {
    let opts = syn_options();
    test_assert!(
        tcp_parse_mss(&opts) == Some(536),
        "tcp-optparse MSS from wire option bytes"
    );
    test_assert!(
        tcp_parse_wscale(&opts) == Some(7),
        "tcp-optparse WS shift from wire option bytes"
    );
    test_assert!(
        tcp_parse_tsval(&opts) == Some(0xDEAD_BEEF),
        "tcp-optparse TSval from wire option bytes"
    );

    // EOL-terminated empty list → None everywhere.
    let empty = [0u8; 8];
    test_assert!(
        tcp_parse_mss(&empty).is_none()
            && tcp_parse_wscale(&empty).is_none()
            && tcp_parse_tsval(&empty).is_none(),
        "tcp-optparse empty option list parses to None"
    );

    // Truncated option (kind+len advertising past the end) → None; the
    // parsers must stop at the slice end, not walk past it.
    let trunc = [2u8, 4, 0x02];
    test_assert!(
        tcp_parse_mss(&trunc).is_none(),
        "tcp-optparse truncated MSS option rejected"
    );

    // WS shift clamps to 14 (RFC 7323 limit).
    let big_ws = [3u8, 3, 99, 1, 0, 0, 0, 0];
    test_assert!(
        tcp_parse_wscale(&big_ws) == Some(14),
        "tcp-optparse WS shift clamped to 14"
    );
}

// --------------------------------------------------------------------------
// Finding 8: TcpHdr::from_bytes parses an UNALIGNED header (the frame
// path hands it a buffer at ETHERNET_HEADER + IHL*4 — 2 mod 4).
// --------------------------------------------------------------------------

fn test_unaligned_header_parse() {
    // Wire-format header at buffer offset 1 (misaligned for the u32
    // fields) — from_bytes must return it by value, correctly.
    let mut pkt = [0u8; 48];
    let h = hdr(0x1122_3344, 0x5566_7788, ACK | PSH, 0xBEEF);
    let wire = wire_packet(&h, &[]);
    pkt[1..1 + wire.len()].copy_from_slice(&wire);

    let parsed = TcpHdr::from_bytes(&pkt[1..1 + wire.len()]).unwrap();
    test_assert!(
        parsed.seq == 0x1122_3344u32.to_be()
            && parsed.ack_seq == 0x5566_7788u32.to_be()
            && parsed.window() == 0xBEEF
            && parsed.ack()
            && parsed.psh(),
        "tcp-hdr unaligned header parses field-exact"
    );

    // Too-short buffers are rejected, not read.
    test_assert!(
        TcpHdr::from_bytes(&pkt[1..1 + 19]).is_none(),
        "tcp-hdr short buffer rejected"
    );
}
