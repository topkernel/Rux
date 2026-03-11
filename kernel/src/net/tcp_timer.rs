//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP 定时器管理
//!
//! 参考 Linux: net/ipv4/tcp_timer.c
//!
//! # 功能
//! - 重传定时器 (RTO)
//! - 延迟 ACK 定时器
//! - 零窗口探测定时器
//! - TIME_WAIT 定时器
//!
//! # 集成
//! 在时钟中断处理函数中调用 tcp_timer_tick() 来检查和处理到期的定时器

use crate::drivers::timer::get_jiffies;
use crate::net::tcp::{TcpSocket, TcpState, TcpSocketTable};

/// TCP 定时器常量
pub const TCP_RTO_MIN_US: u64 = 200_000;      // 最小 RTO 200ms
pub const TCP_RTO_MAX_US: u64 = 120_000_000;  // 最大 RTO 120s
pub const TCP_MAX_RETRIES: u32 = 15;          // 最大重传次数
pub const TCP_DELACK_TIMEOUT_US: u64 = 40_000; // 延迟 ACK 40ms
pub const TCP_TIMEWAIT_TIMEOUT_US: u64 = 60_000_000; // TIME_WAIT 60s

/// TCP 定时器管理器
///
/// 管理所有 TCP socket 的定时器
pub struct TcpTimerManager {
    /// 统计：定时器触发次数
    pub timer_ticks: u64,
    /// 统计：重传次数
    pub retransmits: u64,
    /// 统计：超时关闭连接数
    pub timeout_closes: u64,
}

impl TcpTimerManager {
    /// 创建新的 TCP 定时器管理器
    pub const fn new() -> Self {
        Self {
            timer_ticks: 0,
            retransmits: 0,
            timeout_closes: 0,
        }
    }

    /// TCP 定时器 tick
    ///
    /// 在时钟中断处理中调用，检查所有 socket 的定时器
    ///
    /// # 注意
    /// - 此函数在中断上下文中调用，不能阻塞
    /// - 需要尽快完成
    pub fn tick(&mut self, table: &mut TcpSocketTable) {
        self.timer_ticks += 1;
        let now = get_jiffies();

        // 使用 sockets_mut 获取 socket 数组
        let sockets = table.sockets_mut();

        for slot in sockets.iter_mut() {
            if let Some(ref mut socket) = slot {
                self.check_socket_timers(socket, now);
            }
        }
    }

    /// 检查单个 socket 的定时器
    fn check_socket_timers(&mut self, socket: &mut TcpSocket, now: u64) {
        // 只检查已建立连接或正在关闭的连接
        match socket.state {
            TcpState::TCP_ESTABLISHED
            | TcpState::TCP_FIN_WAIT1
            | TcpState::TCP_FIN_WAIT2
            | TcpState::TCP_CLOSE_WAIT
            | TcpState::TCP_CLOSING
            | TcpState::TCP_LAST_ACK => {
                // 检查重传定时器
                if socket.timers.retransmit_deadline > 0
                    && now >= socket.timers.retransmit_deadline
                {
                    self.retransmits += 1;
                    socket.retransmit_timer_expired();

                    if socket.state == TcpState::TCP_CLOSE {
                        self.timeout_closes += 1;
                    }
                }

                // 检查延迟 ACK 定时器
                if socket.timers.delack_deadline > 0
                    && now >= socket.timers.delack_deadline
                {
                    // 发送延迟 ACK
                    let _ = socket.send_ack_public();
                    socket.timers.delack_deadline = 0;
                }
            }
            TcpState::TCP_TIME_WAIT => {
                // 检查 TIME_WAIT 定时器
                if socket.timers.retransmit_deadline > 0
                    && now >= socket.timers.retransmit_deadline
                {
                    // TIME_WAIT 超时，关闭连接
                    socket.state = TcpState::TCP_CLOSE;
                    socket.timers.stop_retransmit();
                }
            }
            _ => {
                // 其他状态不处理定时器
            }
        }
    }
}

/// 全局 TCP 定时器管理器
static mut TCP_TIMER_MANAGER: core::mem::MaybeUninit<TcpTimerManager> =
    core::mem::MaybeUninit::uninit();

/// 初始化 TCP 定时器管理器
pub fn init_tcp_timer_manager() {
    unsafe {
        TCP_TIMER_MANAGER.write(TcpTimerManager::new());
    }
}

/// 获取 TCP 定时器管理器
pub fn get_tcp_timer_manager() -> &'static mut TcpTimerManager {
    unsafe { TCP_TIMER_MANAGER.assume_init_mut() }
}

/// TCP 定时器 tick - 从时钟中断调用
///
/// # Safety
/// 此函数修改全局 TCP socket 表，调用者需确保同步
pub fn tcp_timer_tick() {
    // 获取定时器管理器
    let manager = get_tcp_timer_manager();

    // 获取 TCP socket 表
    let table = crate::net::tcp::get_tcp_socket_table();

    // 处理定时器
    manager.tick(table);
}
