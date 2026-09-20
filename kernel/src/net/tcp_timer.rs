//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP Timer Management
//!
//! # Features
//! - Retransmission timer (RTO)
//! - Delayed ACK timer
//! - Zero window probe timer
//! - TIME_WAIT timer
//!
//! # Integration
//! Call tcp_timer_tick() in clock interrupt handler to check and process expired timers

use crate::drivers::timer::get_jiffies;
use crate::net::tcp::{TcpSocket, TcpState, TcpSocketTable, TcpTxBatch, TCP_SOCKET_TABLE_SIZE, TCP_DEFAULT_MSS};

/// TCP timer constants - from config
pub const TCP_RTO_MIN_US: u64 = crate::config::TCP_RTO_MIN_US;
pub const TCP_RTO_MAX_US: u64 = crate::config::TCP_RTO_MAX_US;
pub const TCP_MAX_RETRIES: u32 = crate::config::TCP_MAX_RETRIES;
pub const TCP_DELACK_TIMEOUT_US: u64 = crate::config::TCP_DELACK_TIMEOUT_US;
pub const TCP_TIMEWAIT_TIMEOUT_US: u64 = crate::config::TCP_TIMEWAIT_TIMEOUT_US;

/// TCP timer manager
///
/// Manages timers for all TCP sockets
pub struct TcpTimerManager {
    /// Statistics: timer trigger count
    pub timer_ticks: u64,
    /// Statistics: retransmit count
    pub retransmits: u64,
    /// Statistics: timeout close connection count
    pub timeout_closes: u64,
}

impl TcpTimerManager {
    /// Create new TCP timer manager
    pub const fn new() -> Self {
        Self {
            timer_ticks: 0,
            retransmits: 0,
            timeout_closes: 0,
        }
    }

    /// TCP timer tick
    ///
    /// Called in clock interrupt handler, checks all socket timers
    ///
    /// # Note
    /// - This function is called in interrupt context, cannot block
    /// - Must complete quickly
    ///
    /// R35 (chain-2 fix): retransmissions / delayed ACKs / SYN re-sends
    /// are RECORDED into `tx` (memcpy into capacity reserved outside the
    /// table lock) and emitted by the caller after TCP_TABLE_LOCK drops —
    /// the tick used to run the virtio TX completion spin (10M+50M
    /// iterations per packet) inline while holding the table lock,
    /// serializing every CPU's networking behind one expired timer.
    pub fn tick(&mut self, table: &mut TcpSocketTable, tx: &mut TcpTxBatch) {
        self.timer_ticks += 1;
        let now = get_jiffies();

        // Use sockets_mut to get socket array
        let sockets = table.sockets_mut();

        for (_idx, slot) in sockets.iter_mut().enumerate() {
            if let Some(ref mut socket) = slot {
                self.check_socket_timers(socket, now, tx);
            }
        }

        // Free dead sockets outside the iteration — R21-N4: only when no
        // userspace fd still wraps the slot (user_refs == 0); otherwise
        // the slot stays CLOSE-but-allocated for Socket::close to reap
        // (prevents index reuse under a live fd = cross-connection data
        // confusion).
        // R24: sweep unreferenced CLOSE corpses among RX-SPAWNED CHILDREN,
        // not just the ones this tick transitioned. A SYN-spawned child
        // that entered TCP_CLOSE outside the timer (peer RST in tcp_rcv, or
        // LAST_ACK's final ACK) with no fd ever created (never accepted, or
        // accept's pin failed) was reaped by no one — the 64-slot table
        // fills one leaked slot per RST'd connection. parent_fd.is_some()
        // is the discriminator for children; R32-B7 adds `orphaned` for
        // CLIENT/listener slots whose fd left them mid-close (FIN_WAIT
        // etc.) — without the flag those CLOSE corpses were invisible to
        // the sweep (indistinguishable from fresh pre-connect slots with
        // user_refs==0) and leaked forever.
        //
        // R34 (TCP-side wedge): this whole tick runs under TCP_TABLE_LOCK —
        // the old `Vec::collect()` heap-allocated under the lock, an
        // OOM-panic point (`alloc_error_handler` panics; panic=abort parks
        // the CPU in `wfi` holding TCP_TABLE_LOCK while the other 3 CPUs
        // spin on it forever — the observed "holder never returns"
        // signature). The table is bounded by TCP_SOCKET_TABLE_SIZE, so a
        // fixed stack array removes the allocation entirely.
        let mut freeable = [0usize; crate::net::tcp::TCP_SOCKET_TABLE_SIZE];
        let mut freeable_len = 0usize;
        for (idx, slot) in sockets.iter().enumerate() {
            if slot
                .as_ref()
                .map(|sk| {
                    sk.state == TcpState::TCP_CLOSE
                        && sk.user_refs.load(core::sync::atomic::Ordering::Acquire) == 0
                        && (sk.parent_fd.is_some() || sk.orphaned)
                })
                .unwrap_or(false)
            {
                freeable[freeable_len] = idx;
                freeable_len += 1;
            }
        }
        drop(sockets);
        for idx in freeable.iter().take(freeable_len) {
            table.free(*idx);
        }
    }

    /// Check timers for single socket
    fn check_socket_timers(&mut self, socket: &mut TcpSocket, now: u64, tx: &mut TcpTxBatch) {
        // Only check established connections or connections being closed
        match socket.state {
            TcpState::TCP_ESTABLISHED
            | TcpState::TCP_CLOSE_WAIT
            | TcpState::TCP_CLOSING
            | TcpState::TCP_LAST_ACK => {
                // R32-B8: orphaned CLOSE_WAIT reclamation — the peer sent
                // FIN and no fd ever claimed the slot (child never
                // accepted, or accept unwound). Nothing else can observe
                // or close this connection; without a bound it sat in
                // CLOSE_WAIT forever and each port-scan style
                // connect+FIN leaked one of the 64 table slots. fd-held
                // CLOSE_WAITs (user_refs > 0) are left alone — the
                // application may still be draining/sending, exactly like
                // Linux; they are reclaimed when the fd closes.
                if socket.state == TcpState::TCP_CLOSE_WAIT
                    && socket.user_refs.load(core::sync::atomic::Ordering::Acquire) == 0
                {
                    if socket.timers.close_wait_since == 0 {
                        socket.timers.close_wait_since = now;
                    } else if now - socket.timers.close_wait_since
                        > (crate::config::TCP_TIMEWAIT_TIMEOUT_US / 10_000)
                    {
                        socket.state = TcpState::TCP_CLOSE;
                        socket.send_buffer.clear();
                        socket.recv_buffer.clear();
                        socket.retrans_queue.clear();
                        socket.ooo_queue.clear();
                        socket.timers.stop_retransmit();
                    }
                }

                // R23-2: restored the combined retransmit/delack arm —
                // the R21-N3b split (ESTABLISHED grouped with FIN_WAIT)
                // both killed 60s+ live connections via the fin_wait
                // timeout and (Rust arms do not fall through) silently
                // deleted RTO retransmission for those states.
                // Check retransmit timer
                if socket.timers.retransmit_deadline > 0
                    && now >= socket.timers.retransmit_deadline
                {
                    self.retransmits += 1;
                    socket.retransmit_timer_expired(tx);

                    if socket.state == TcpState::TCP_CLOSE {
                        self.timeout_closes += 1;
                    }
                }

                // Check delayed ACK timer
                if socket.timers.delack_deadline > 0
                    && now >= socket.timers.delack_deadline
                {
                    // Send delayed ACK (R35: recorded into `tx`, emitted
                    // after the lock drops; deadline cleared regardless,
                    // preserving the old semantics where a failed
                    // alloc_skb also cleared it).
                    let _ = socket.send_ack_public(tx);
                    socket.timers.delack_deadline = 0;
                }
            }
            TcpState::TCP_SYN_SENT => {
                // R32-N16: SYN retransmission. connect() arms
                // retransmit_deadline; the old catch-all arm ignored
                // SYN_SENT entirely, so one lost SYN parked the socket
                // (and the connect()ing task) in SYN_SENT forever.
                if socket.timers.retransmit_deadline > 0
                    && now >= socket.timers.retransmit_deadline
                {
                    if socket.timers.syn_retries >= TCP_MAX_RETRIES {
                        socket.state = TcpState::TCP_CLOSE;
                        socket.timers.stop_retransmit();
                        socket.timers.syn_retries = 0;
                        self.timeout_closes += 1;
                    } else {
                        socket.timers.syn_retries += 1;
                        let _ = socket.resend_syn(tx);
                        // Exponential backoff, capped at TCP_RTO_MAX_US.
                        let shift = core::cmp::min(socket.timers.syn_retries, 6) as u32;
                        let backoff_us = (crate::config::TCP_RTO_DEFAULT_US << shift)
                            .min(TCP_RTO_MAX_US);
                        socket.timers.start_retransmit(backoff_us);
                    }
                }
            }
            TcpState::TCP_FIN_WAIT1 | TcpState::TCP_FIN_WAIT2 => {
                // R23-1: orphaned half-close timeout — armed ONLY in
                // FIN_WAIT (never while ESTABLISHED), bounded so dead
                // peers cannot hold slots forever.
                if socket.timers.fin_wait_since == 0 {
                    socket.timers.fin_wait_since = now;
                } else if now - socket.timers.fin_wait_since
                    > (crate::config::TCP_TIMEWAIT_TIMEOUT_US / 10_000)
                {
                    socket.state = TcpState::TCP_CLOSE;
                }
                // Retransmit (the FIN itself — R21-N3) + delack still run.
                if socket.timers.retransmit_deadline > 0
                    && now >= socket.timers.retransmit_deadline
                {
                    self.retransmits += 1;
                    socket.retransmit_timer_expired(tx);

                    if socket.state == TcpState::TCP_CLOSE {
                        self.timeout_closes += 1;
                    }
                }
                if socket.timers.delack_deadline > 0
                    && now >= socket.timers.delack_deadline
                {
                    let _ = socket.send_ack_public(tx);
                    socket.timers.delack_deadline = 0;
                }
            }
            TcpState::TCP_TIME_WAIT => {
                // Check TIME_WAIT timer
                if socket.timers.retransmit_deadline > 0
                    && now >= socket.timers.retransmit_deadline
                {
                    // TIME_WAIT timeout, close connection
                    socket.state = TcpState::TCP_CLOSE;
                    socket.timers.stop_retransmit();
                }
            }
            _ => {
                // Other states don't process timers
            }
        }
    }
}

/// Global TCP timer manager
static mut TCP_TIMER_MANAGER: core::mem::MaybeUninit<TcpTimerManager> =
    core::mem::MaybeUninit::uninit();

/// Initialize TCP timer manager
pub fn init_tcp_timer_manager() {
    unsafe {
        TCP_TIMER_MANAGER.write(TcpTimerManager::new());
    }
}

/// Get TCP timer manager
pub fn get_tcp_timer_manager() -> &'static mut TcpTimerManager {
    unsafe { TCP_TIMER_MANAGER.assume_init_mut() }
}

/// TCP timer tick - called from Timer softirq (bottom half)
///
/// # Safety
/// This function modifies global TCP socket table, caller must ensure synchronization
///
/// R35 (chain-2 fix): the retransmit/delack/SYN-resend emissions moved OUT
/// of the TCP_TABLE_LOCK critical section. The tick records wire-ready
/// segments into a TcpTxBatch whose capacity is reserved BEFORE the lock
/// (try_reserve — an OOM here is a clean deferral to the next tick, not
/// the alloc_error_handler panic that parked the CPU in `wfi` holding the
/// lock, the R34 wedge signature), then emits them after the lock drops.
/// The whole decision pass stays under the table lock exactly as before
/// (R21-N1 serialization; the R34 fixed-size sweep array is untouched) —
/// only the virtio TX spin (10M+50M iterations per packet) left the
/// critical section.
pub fn tcp_timer_tick() {
    // Get timer manager
    let manager = get_tcp_timer_manager();

    // Get TCP socket table
    let table = crate::net::tcp::get_tcp_socket_table();

    // Size the staging BEFORE the lock from a racy table.count() read:
    // count only changes under TCP_TABLE_LOCK, so a stale read can only
    // under-estimate — which degrades to per-segment deferral on the next
    // tick (retransmit_timer_expired keeps the retry accounting intact),
    // never to an allocation under the lock. Worst case is bounded by the
    // table size: 2 descriptors (retrans + delack) and one MSS of payload
    // per socket.
    let n = table
        .count()
        .min(TCP_SOCKET_TABLE_SIZE);
    let mut tx = TcpTxBatch::new();
    let _ = tx.reserve(2 * n + 4, n * TCP_DEFAULT_MSS as usize);

    // Process timers — R21-N1: under the table lock (was racing syscalls
    // and RX on the 4-CPU kernel).
    {
        let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
        manager.tick(table, &mut tx);
    }

    // Emit outside the lock — no re-entry hazard: emitted packets go to
    // the loopback backlog (drained later by ethernet_poll) or straight
    // to the virtio device, never back into tcp_rcv on this CPU.
    tx.emit_all();
}

/// Timer softirq handler — deferred from clock interrupt via `raise_softirq_irqoff(Timer)`.
pub fn timer_softirq_handler(_vec: usize) {
    tcp_timer_tick();
}
