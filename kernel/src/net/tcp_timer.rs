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
use crate::net::tcp::{TcpSocket, TcpState, TcpSocketTable};

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
    pub fn tick(&mut self, table: &mut TcpSocketTable) {
        self.timer_ticks += 1;
        let now = get_jiffies();

        // Use sockets_mut to get socket array
        let sockets = table.sockets_mut();

        for (_idx, slot) in sockets.iter_mut().enumerate() {
            if let Some(ref mut socket) = slot {
                self.check_socket_timers(socket, now);
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
        let freeable: alloc::vec::Vec<usize> = sockets
            .iter()
            .enumerate()
            .filter(|(_idx, slot)| {
                slot.as_ref()
                    .map(|sk| {
                        sk.state == TcpState::TCP_CLOSE
                            && sk.user_refs.load(core::sync::atomic::Ordering::Acquire) == 0
                            && (sk.parent_fd.is_some() || sk.orphaned)
                    })
                    .unwrap_or(false)
            })
            .map(|(idx, _)| idx)
            .collect();
        drop(sockets);
        for idx in freeable {
            table.free(idx);
        }
    }

    /// Check timers for single socket
    fn check_socket_timers(&mut self, socket: &mut TcpSocket, now: u64) {
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
                    socket.retransmit_timer_expired();

                    if socket.state == TcpState::TCP_CLOSE {
                        self.timeout_closes += 1;
                    }
                }

                // Check delayed ACK timer
                if socket.timers.delack_deadline > 0
                    && now >= socket.timers.delack_deadline
                {
                    // Send delayed ACK
                    let _ = socket.send_ack_public();
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
                        let _ = socket.resend_syn();
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
                    socket.retransmit_timer_expired();

                    if socket.state == TcpState::TCP_CLOSE {
                        self.timeout_closes += 1;
                    }
                }
                if socket.timers.delack_deadline > 0
                    && now >= socket.timers.delack_deadline
                {
                    let _ = socket.send_ack_public();
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
pub fn tcp_timer_tick() {
    // Get timer manager
    let manager = get_tcp_timer_manager();

    // Get TCP socket table
    let table = crate::net::tcp::get_tcp_socket_table();

    // Process timers — R21-N1: under the table lock (was racing syscalls
    // and RX on the 4-CPU kernel).
    let _g = crate::net::tcp::TCP_TABLE_LOCK.lock_irqsave();
    manager.tick(table);
}

/// Timer softirq handler — deferred from clock interrupt via `raise_softirq_irqoff(Timer)`.
pub fn timer_softirq_handler(_vec: usize) {
    tcp_timer_tick();
}
