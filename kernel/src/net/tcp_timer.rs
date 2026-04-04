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
        let mut to_free: alloc::vec::Vec<usize> = alloc::vec::Vec::new();

        for (idx, slot) in sockets.iter_mut().enumerate() {
            if let Some(ref mut socket) = slot {
                let prev_state = socket.state;
                self.check_socket_timers(socket, now);

                // If timer transitioned socket to TCP_CLOSE, schedule for freeing
                if prev_state != TcpState::TCP_CLOSE
                    && socket.state == TcpState::TCP_CLOSE
                {
                    to_free.push(idx);
                }
            }
        }

        // Free dead sockets outside the iteration
        for idx in to_free {
            table.free(idx);
        }
    }

    /// Check timers for single socket
    fn check_socket_timers(&mut self, socket: &mut TcpSocket, now: u64) {
        // Only check established connections or connections being closed
        match socket.state {
            TcpState::TCP_ESTABLISHED
            | TcpState::TCP_FIN_WAIT1
            | TcpState::TCP_FIN_WAIT2
            | TcpState::TCP_CLOSE_WAIT
            | TcpState::TCP_CLOSING
            | TcpState::TCP_LAST_ACK => {
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

    // Process timers
    manager.tick(table);
}

/// Timer softirq handler — deferred from clock interrupt via `raise_softirq_irqoff(Timer)`.
pub fn timer_softirq_handler(_vec: usize) {
    tcp_timer_tick();
}
