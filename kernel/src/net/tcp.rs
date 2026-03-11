//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP 协议
//!
//! 完全...

use crate::net::buffer::SkBuff;
use crate::net::ipv4::{route, checksum};
pub use crate::config::TCP_SOCKET_TABLE_SIZE;

/// TCP 头部长度
pub const TCP_MIN_HLEN: usize = 20;
pub const TCP_MAX_HLEN: usize = 60;

/// TCP 最大窗口大小
pub const TCP_MAX_WINDOW: u16 = 65535;

/// TCP 默认 MSS
pub const TCP_DEFAULT_MSS: u16 = 1460;

/// TCP 定时器常量
pub const TCP_RTO_MIN_US: u64 = 200_000;      // 最小 RTO 200ms
pub const TCP_RTO_MAX_US: u64 = 120_000_000;  // 最大 RTO 120s
pub const TCP_RTO_DEFAULT_US: u64 = 1_000_000; // 默认 RTO 1s
pub const TCP_MAX_RETRIES: u32 = 15;          // 最大重传次数
pub const TCP_DELACK_TIMEOUT_US: u64 = 40_000; // 延迟 ACK 40ms

/// TCP 端口号
pub type TcpPort = u16;

/// TCP 序列号
pub type TcpSeq = u32;

/// TCP 确认号
pub type TcpAck = u32;

/// TCP 头部
///
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpHdr {
    /// 源端口
    pub source: TcpPort,
    /// 目标端口
    pub dest: TcpPort,
    /// 序列号
    pub seq: TcpSeq,
    /// 确认号
    pub ack_seq: TcpAck,
    /// 数据偏移 + 保留 + 标志
    pub dof_res: u8,
    /// 标志 + 窗口大小
    pub flags_win: u16,
    /// 校验和
    pub check: u16,
    /// 紧急指针
    pub urg_ptr: u16,
}

impl TcpHdr {
    /// 从字节切片创建 TCP 头部
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < TCP_MIN_HLEN {
            return None;
        }

        unsafe {
            Some(&*(data.as_ptr() as *const TcpHdr))
        }
    }

    /// 获取数据偏移（以 32 位字为单位）
    pub fn dof(&self) -> u8 {
        self.dof_res >> 4
    }

    /// 获取 TCP 头部长度（字节）
    pub fn header_len(&self) -> usize {
        (self.dof() as usize) * 4
    }

    /// 检查 SYN 标志
    pub fn syn(&self) -> bool {
        (self.flags_win & 0x02) != 0
    }

    /// 检查 ACK 标志
    pub fn ack(&self) -> bool {
        (self.flags_win & 0x10) != 0
    }

    /// 检查 FIN 标志
    pub fn fin(&self) -> bool {
        (self.flags_win & 0x01) != 0
    }

    /// 检查 RST 标志
    pub fn rst(&self) -> bool {
        (self.flags_win & 0x04) != 0
    }

    /// 检查 PSH 标志
    pub fn psh(&self) -> bool {
        (self.flags_win & 0x08) != 0
    }

    /// 获取窗口大小
    pub fn window(&self) -> u16 {
        u16::from_be(self.flags_win & 0xFF00)
    }

    /// 设置数据偏移
    pub fn set_dof(&mut self, dof: u8) {
        self.dof_res = (dof << 4) | (self.dof_res & 0x0F);
    }

    /// 设置 SYN 标志
    pub fn set_syn(&mut self) {
        self.flags_win |= 0x0002;
    }

    /// 设置 ACK 标志
    pub fn set_ack(&mut self) {
        self.flags_win |= 0x0010;
    }

    /// 设置 FIN 标志
    pub fn set_fin(&mut self) {
        self.flags_win |= 0x0001;
    }

    /// 设置 RST 标志
    pub fn set_rst(&mut self) {
        self.flags_win |= 0x0004;
    }

    /// 设置 PSH 标志
    pub fn set_psh(&mut self) {
        self.flags_win |= 0x0008;
    }

    /// 设置窗口大小
    pub fn set_window(&mut self, win: u16) {
        self.flags_win = (self.flags_win & 0x00FF) | (win & 0xFF00);
    }
}

/// TCP 状态
///
/// ...
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum TcpState {
    /// 关闭
    TCP_CLOSE = 0,
    /// 监听
    TCP_LISTEN = 1,
    /// SYN 发送
    TCP_SYN_SENT = 2,
    /// SYN 接收
    TCP_SYN_RECV = 3,
    /// 已建立
    TCP_ESTABLISHED = 4,
    /// FIN 等待 1
    TCP_FIN_WAIT1 = 5,
    /// FIN 等待 2
    TCP_FIN_WAIT2 = 6,
    /// 关闭等待
    TCP_CLOSE_WAIT = 7,
    /// 最后 ACK
    TCP_LAST_ACK = 8,
    /// 时间等待
    TCP_TIME_WAIT = 9,
    /// 关闭中
    TCP_CLOSING = 10,
}

/// TCP 发送段（用于重传队列）
///
/// 保存已发送但未确认的数据副本
#[derive(Debug, Clone)]
pub struct TcpSendSeg {
    /// 起始序列号
    pub seq: TcpSeq,
    /// 数据长度
    pub len: usize,
    /// 数据副本
    pub data: alloc::vec::Vec<u8>,
    /// 发送时间戳 (jiffies)
    pub tx_time: u64,
    /// 重传次数
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

/// TCP RTT 估算器 (RFC 6298)
#[derive(Debug, Clone)]
pub struct TcpRttEstimator {
    /// 平滑 RTT (微秒)
    pub srtt: u64,
    /// RTT 方差 (微秒)
    pub rttvar: u64,
    /// 当前 RTO (微秒)
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

    /// 更新 RTT 估算 (RFC 6298)
    ///
    /// # 参数
    /// - `rtt_sample`: RTT 样本（微秒）
    pub fn update(&mut self, rtt_sample: u64) {
        if self.srtt == 0 {
            // 第一次测量
            self.srtt = rtt_sample;
            self.rttvar = rtt_sample / 2;
        } else {
            // RFC 6298 算法
            let delta = if rtt_sample > self.srtt {
                rtt_sample - self.srtt
            } else {
                self.srtt - rtt_sample
            };
            self.rttvar = (3 * self.rttvar + delta) / 4;
            self.srtt = (7 * self.srtt + rtt_sample) / 8;
        }

        // 计算 RTO = SRTT + 4 * RTTVAR
        self.rto = self.srtt.saturating_add(4 * self.rttvar);
        self.rto = self.rto.clamp(TCP_RTO_MIN_US, TCP_RTO_MAX_US);
    }

    /// RTO 指数退避
    pub fn backoff(&mut self) {
        self.rto = core::cmp::min(self.rto * 2, TCP_RTO_MAX_US);
    }

    /// 重置 RTO（连接建立后）
    pub fn reset(&mut self) {
        self.rto = TCP_RTO_DEFAULT_US;
    }
}

impl Default for TcpRttEstimator {
    fn default() -> Self {
        Self::new()
    }
}

/// TCP 拥塞控制状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpCongState {
    /// 慢启动
    SlowStart,
    /// 拥塞避免
    CongestionAvoidance,
    /// 快速恢复
    FastRecovery,
}

impl Default for TcpCongState {
    fn default() -> Self {
        TcpCongState::SlowStart
    }
}

/// TCP 拥塞控制 (RFC 5681)
#[derive(Debug, Clone)]
pub struct TcpCongestion {
    /// 拥塞窗口（字节）
    pub cwnd: u32,
    /// 慢启动阈值（字节）
    pub ssthresh: u32,
    /// 当前拥塞状态
    pub state: TcpCongState,
    /// 重复 ACK 计数
    pub dup_ack_count: u32,
    /// 恢复点序列号
    pub recover_seq: TcpSeq,
}

impl TcpCongestion {
    pub fn new(mss: u16) -> Self {
        Self {
            cwnd: mss as u32,      // 初始 1 MSS
            ssthresh: u32::MAX,    // 初始无限大
            state: TcpCongState::SlowStart,
            dup_ack_count: 0,
            recover_seq: 0,
        }
    }

    /// 收到 ACK 更新拥塞窗口
    pub fn on_ack(&mut self, acked_bytes: u32, mss: u16) {
        match self.state {
            TcpCongState::SlowStart => {
                // 慢启动：cwnd 每收到一个 ACK 增加 1 MSS
                self.cwnd += mss as u32;
                if self.cwnd >= self.ssthresh {
                    self.state = TcpCongState::CongestionAvoidance;
                }
            }
            TcpCongState::CongestionAvoidance => {
                // 拥塞避免：cwnd 每个往返增加 1 MSS
                // 即每个 ACK 增加 MSS * MSS / cwnd
                let increment = (mss as u32 * mss as u32) / core::cmp::max(self.cwnd, 1);
                self.cwnd += increment;
            }
            TcpCongState::FastRecovery => {
                // 快速恢复：收到新数据的 ACK，结束快速恢复
                self.state = TcpCongState::CongestionAvoidance;
            }
        }
    }

    /// 收到重复 ACK
    pub fn on_dup_ack(&mut self, ack: TcpSeq, snd_nxt: TcpSeq, mss: u16) {
        self.dup_ack_count += 1;

        if self.dup_ack_count == 3 && Self::seq_before(ack, snd_nxt) {
            // 快速重传：3 个重复 ACK
            // 设置 ssthresh = max(cwnd/2, 2*MSS)
            self.ssthresh = core::cmp::max(self.cwnd / 2, 2 * mss as u32);

            // 设置 cwnd = ssthresh + 3*MSS
            self.cwnd = self.ssthresh + 3 * mss as u32;

            // 记录恢复点
            self.recover_seq = snd_nxt;

            // 进入快速恢复
            self.state = TcpCongState::FastRecovery;
        } else if self.state == TcpCongState::FastRecovery {
            // 在快速恢复中，收到重复 ACK，增加 cwnd
            self.cwnd += mss as u32;
        }
    }

    /// 超时处理
    pub fn on_timeout(&mut self, mss: u16) {
        // 超时是严重拥塞
        self.ssthresh = core::cmp::max(self.cwnd / 2, 2 * mss as u32);
        self.cwnd = mss as u32; // 重置为 1 MSS
        self.state = TcpCongState::SlowStart;
        self.dup_ack_count = 0;
    }

    /// 重置（新连接）
    pub fn reset(&mut self, mss: u16) {
        self.cwnd = mss as u32;
        self.ssthresh = u32::MAX;
        self.state = TcpCongState::SlowStart;
        self.dup_ack_count = 0;
        self.recover_seq = 0;
    }

    /// 序列号比较：a 在 b 之前
    pub fn seq_before(a: TcpSeq, b: TcpSeq) -> bool {
        ((a as i32) - (b as i32)) < 0
    }
}

impl Default for TcpCongestion {
    fn default() -> Self {
        Self::new(TCP_DEFAULT_MSS)
    }
}

/// TCP 定时器状态
#[derive(Debug, Clone)]
pub struct TcpTimers {
    /// 重传定时器到期时间 (jiffies)，0 表示未激活
    pub retransmit_deadline: u64,
    /// 延迟 ACK 定时器到期时间 (jiffies)
    pub delack_deadline: u64,
}

impl TcpTimers {
    pub fn new() -> Self {
        Self {
            retransmit_deadline: 0,
            delack_deadline: 0,
        }
    }

    /// 启动重传定时器
    pub fn start_retransmit(&mut self, rto_us: u64) {
        let now = crate::drivers::timer::get_jiffies();
        // 微秒转 jiffies (1 jiffy = 10ms = 10_000us)
        let rto_jiffies = (rto_us / 10_000).max(1);
        self.retransmit_deadline = now + rto_jiffies;
    }

    /// 停止重传定时器
    pub fn stop_retransmit(&mut self) {
        self.retransmit_deadline = 0;
    }

    /// 检查重传定时器是否到期
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

/// TCP Socket 结构
///
/// 包含连接状态、序列号、可靠性机制等
#[repr(C)]
pub struct TcpSocket {
    // === 基本连接信息 ===
    /// 本地端口
    pub local_port: TcpPort,
    /// 远程端口
    pub remote_port: TcpPort,
    /// 远程 IP 地址
    pub remote_ip: u32,
    /// 本地 IP 地址
    pub local_ip: u32,
    /// TCP 状态
    pub state: TcpState,
    /// 是否已绑定
    pub bound: bool,

    // === 序列号管理 ===
    /// 发送序列号（下一个要发送的）
    pub snd_nxt: TcpSeq,
    /// 发送未确认序列号（最早的未确认）
    pub snd_una: TcpSeq,
    /// 接收序列号（期望接收的下一个）
    pub rcv_nxt: TcpSeq,

    // === 滑动窗口 ===
    /// 发送窗口（对端通告）
    pub snd_wnd: u16,
    /// 接收窗口（本端通告）
    pub rcv_wnd: u16,

    // === 缓冲区 ===
    /// 发送缓冲区（等待发送的数据）
    pub send_buffer: alloc::collections::VecDeque<u8>,
    /// 接收缓冲区（已接收未读取的数据）
    pub recv_buffer: alloc::collections::VecDeque<u8>,
    /// 重传队列（已发送未确认）
    pub retrans_queue: alloc::collections::VecDeque<TcpSendSeg>,

    // === 可靠性机制 ===
    /// RTT 估算器
    pub rtt_estimator: TcpRttEstimator,
    /// 拥塞控制
    pub congestion: TcpCongestion,
    /// 定时器
    pub timers: TcpTimers,

    // === 连接参数 ===
    /// 最大段大小
    pub mss: u16,
    /// 初始序列号
    pub isn: TcpSeq,

    // === 向后兼容 ===
    /// 窗口大小（已废弃，使用 snd_wnd）
    #[deprecated]
    pub window: u16,
}

impl TcpSocket {
    /// 创建新的 TCP Socket
    pub fn new() -> Self {
        Self {
            local_port: 0,
            remote_port: 0,
            remote_ip: 0,
            local_ip: 0xC0A80164, // 默认 192.168.1.100
            state: TcpState::TCP_CLOSE,
            bound: false,

            snd_nxt: 0,
            snd_una: 0,
            rcv_nxt: 0,

            snd_wnd: TCP_MAX_WINDOW,
            rcv_wnd: TCP_MAX_WINDOW,

            send_buffer: alloc::collections::VecDeque::new(),
            recv_buffer: alloc::collections::VecDeque::new(),
            retrans_queue: alloc::collections::VecDeque::new(),

            rtt_estimator: TcpRttEstimator::new(),
            congestion: TcpCongestion::new(TCP_DEFAULT_MSS),
            timers: TcpTimers::new(),

            mss: TCP_DEFAULT_MSS,
            isn: 0,

            #[allow(deprecated)]
            window: TCP_MAX_WINDOW,
        }
    }

    /// 绑定端口
    ///
    /// # 参数
    /// - `port`: 端口号
    pub fn bind(&mut self, port: TcpPort) -> Result<(), ()> {
        // TODO: 检查端口是否已被占用
        self.local_port = port;
        self.bound = true;
        Ok(())
    }

    /// 监听端口
    ///
    /// # 参数
    /// - `backlog`: 等待队列长度
    pub fn listen(&mut self, _backlog: u32) -> Result<(), ()> {
        if !self.bound {
            return Err(());
        }
        self.state = TcpState::TCP_LISTEN;
        Ok(())
    }

        /// 连接到远程地址（主动打开，三次握手）
    ///
    /// # 参数
    /// - `ip`: IP 地址
    /// - `port`: 端口号
    pub fn connect(&mut self, ip: u32, port: TcpPort) -> Result<(), ()> {
        self.remote_ip = ip;
        self.remote_port = port;

        // 初始化序列号（简化实现：使用固定值，实际应使用 ISN）
        self.snd_nxt = 12345;
        self.snd_una = self.snd_nxt;
        self.rcv_nxt = 0; // 将从 SYN-ACK 中获取

        // 发送 SYN 包（三次握手的第一步）
        self.send_syn()?;
        self.state = TcpState::TCP_SYN_SENT;

        Ok(())
    }

    /// 发送 SYN 包（三次握手第一步）
    fn send_syn(&self) -> Result<(), ()> {
        // 构造 SYN 包：seq=ISN, ack=0, flags=SYN
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            0, // ACK 号为 0
            &[], // 无数据
            0x0002, // SYN 标志
        )?;

        // 发送到 IP 层
        crate::net::ipv4::ipv4_send(skb, self.remote_ip, 6); // IPPROTO_TCP = 6

        Ok(())
    }

    /// 发送 SYN-ACK 包（三次握手第二步）
    fn send_synack(&mut self, ack_seq: TcpSeq) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            &[],
            0x0012, // SYN + ACK 标志
        )?;

        crate::net::ipv4::ipv4_send(skb, self.remote_ip, 6);

        Ok(())
    }

    /// 发送 ACK 包（三次握手第三步）
    fn send_ack(&self) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            &[],
            0x0010, // ACK 标志
        )?;

        crate::net::ipv4::ipv4_send(skb, self.remote_ip, 6);

        Ok(())
    }

    /// 发送 ACK 包（公共接口，用于定时器）
    pub fn send_ack_public(&self) -> Result<(), ()> {
        self.send_ack()
    }

    /// 处理接收到的 TCP 包
    pub fn handle_packet(&mut self, tcp_hdr: &TcpHdr, data: &[u8]) -> Result<(), ()> {
        match self.state {
            TcpState::TCP_LISTEN => {
                // 服务器端：接收 SYN 包
                if tcp_hdr.syn() && !tcp_hdr.ack() {
                    self.handle_syn_recv(tcp_hdr)?;
                }
            }
            TcpState::TCP_SYN_SENT => {
                // 客户端：接收 SYN-ACK 包
                if tcp_hdr.syn() && tcp_hdr.ack() {
                    self.handle_synack_recv(tcp_hdr)?;
                }
            }
            TcpState::TCP_SYN_RECV => {
                // 服务器端：接收 ACK 包
                if tcp_hdr.ack() && !tcp_hdr.syn() {
                    self.handle_ack_recv()?;
                }
            }
            TcpState::TCP_ESTABLISHED => {
                // 连接已建立，处理数据
                if tcp_hdr.fin() {
                    self.handle_fin_recv()?;
                } else if !data.is_empty() {
                    self.handle_data_recv(tcp_hdr, data)?;
                }
            }
            _ => {
                // 其他状态暂不处理
            }
        }

        Ok(())
    }

    /// 处理接收到的 SYN 包（服务器端）
    fn handle_syn_recv(&mut self, tcp_hdr: &TcpHdr) -> Result<(), ()> {
        // 记录客户端的初始序列号
        let client_isn = tcp_hdr.seq;
        self.remote_ip = 0; // TODO: 从 IP 包头获取
        self.remote_port = TcpPort::from_be(tcp_hdr.source);

        // 初始化自己的序列号
        self.snd_nxt = 54321; // 服务器 ISN
        self.snd_una = self.snd_nxt;
        self.rcv_nxt = client_isn.wrapping_add(1);

        // 发送 SYN-ACK（三次握手第二步）
        self.send_synack(self.rcv_nxt)?;
        self.state = TcpState::TCP_SYN_RECV;

        Ok(())
    }

    /// 处理接收到的 SYN-ACK 包（客户端）
    fn handle_synack_recv(&mut self, tcp_hdr: &TcpHdr) -> Result<(), ()> {
        // 检查 ACK 是否确认了我们的 SYN
        let ack_num = TcpSeq::from_be(tcp_hdr.ack_seq);
        if ack_num != self.snd_nxt.wrapping_add(1) {
            return Err(()); // ACK 不正确
        }

        // 记录服务器的初始序列号
        let server_isn = tcp_hdr.seq;
        self.rcv_nxt = server_isn.wrapping_add(1);

        // 更新发送序列号
        self.snd_una = self.snd_nxt.wrapping_add(1);
        self.snd_nxt = self.snd_una;

        // 发送 ACK（三次握手第三步）
        self.send_ack()?;
        self.state = TcpState::TCP_ESTABLISHED;

        Ok(())
    }

    /// 处理接收到的 ACK 包（服务器端）
    fn handle_ack_recv(&mut self) -> Result<(), ()> {
        // 检查 ACK 是否确认了我们的 SYN-ACK
        // 三次握手完成，连接建立
        self.state = TcpState::TCP_ESTABLISHED;
        Ok(())
    }

    /// 处理接收到的数据
    fn handle_data_recv(&mut self, tcp_hdr: &TcpHdr, data: &[u8]) -> Result<(), ()> {
        // 检查序列号
        let seq = TcpSeq::from_be(tcp_hdr.seq);
        if seq != self.rcv_nxt {
            return Err(()); // 序列号不匹配
        }

        // 更新接收序列号
        self.rcv_nxt = self.rcv_nxt.wrapping_add(data.len() as u32);

        // 将数据放入接收队列
        self.enqueue_data(data);

        // 发送 ACK（确认数据）
        self.send_ack()?;

        Ok(())
    }

    /// 处理接收到的 FIN 包
    fn handle_fin_recv(&mut self) -> Result<(), ()> {
        // 更新接收序列号（FIN 占据一个序列号）
        self.rcv_nxt = self.rcv_nxt.wrapping_add(1);

        // 发送 ACK
        self.send_ack()?;

        // 根据当前状态转换
        match self.state {
            TcpState::TCP_ESTABLISHED => {
                self.state = TcpState::TCP_CLOSE_WAIT;
            }
            TcpState::TCP_FIN_WAIT1 => {
                self.state = TcpState::TCP_TIME_WAIT;
            }
            _ => {}
        }

        Ok(())
    }

    /// 发送数据
    ///
    /// # 参数
    /// - `data`: 数据
    pub fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
        if self.state != TcpState::TCP_ESTABLISHED {
            return Err(());
        }

        if data.is_empty() {
            return Ok(0);
        }

        // 分段发送（MSS 简化为 1460）
        const MSS: usize = 1460;
        let mut sent = 0;

        while sent < data.len() {
            let chunk_end = (sent + MSS).min(data.len());
            let chunk = &data[sent..chunk_end];

            // 构造 TCP 数据包
            let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

            // 添加数据
            skb.skb_put_data(chunk)?;

            // 构造 TCP 头部
            tcp_build_packet(
                &mut skb,
                self.local_port,
                self.remote_port,
                self.snd_nxt,
                self.rcv_nxt,
                &[],
                0x0018, // PSH + ACK 标志
            )?;

            // 发送到 IP 层
            crate::net::ipv4::ipv4_send(skb, self.remote_ip, 6)?; // IPPROTO_TCP = 6

            // 更新序列号
            self.snd_nxt = self.snd_nxt.wrapping_add(chunk.len() as u32);
            sent = chunk_end;
        }

        Ok(sent)
    }

    /// 接收数据
    ///
    /// # 参数
    /// - `buf`: 缓冲区
    /// - `len`: 缓冲区长度
    pub fn recv(&mut self, buf: &mut [u8], _len: usize) -> Result<usize, ()> {
        if self.state != TcpState::TCP_ESTABLISHED {
            return Err(());
        }

        // 从接收缓冲区读取数据
        let mut read = 0;
        while read < buf.len() && !self.recv_buffer.is_empty() {
            if let Some(byte) = self.recv_buffer.pop_front() {
                buf[read] = byte;
                read += 1;
            }
        }

        Ok(read)
    }

    /// 将数据放入接收缓冲区
    ///
    /// # 参数
    /// - `data`: 接收到的数据
    pub fn enqueue_data(&mut self, data: &[u8]) {
        for &byte in data {
            self.recv_buffer.push_back(byte);
        }
    }

    /// 关闭连接
    pub fn close(&mut self) {
        match self.state {
            TcpState::TCP_ESTABLISHED => {
                self.state = TcpState::TCP_FIN_WAIT1;
                // TODO: 发送 FIN 包
            }
            TcpState::TCP_CLOSE_WAIT => {
                self.state = TcpState::TCP_LAST_ACK;
                // TODO: 发送 FIN 包
            }
            _ => {
                self.state = TcpState::TCP_CLOSE;
            }
        }
    }

    // ========== 可靠性传输方法 ==========

    /// 可靠发送数据
    ///
    /// 将数据放入发送缓冲区并尝试发送，支持重传
    ///
    /// # 参数
    /// - `data`: 要发送的数据
    ///
    /// # 返回
    /// 成功返回发送的字节数，失败返回 Err(())
    pub fn send_reliable(&mut self, data: &[u8]) -> Result<usize, ()> {
        if self.state != TcpState::TCP_ESTABLISHED {
            return Err(());
        }

        if data.is_empty() {
            return Ok(0);
        }

        // 将数据放入发送缓冲区
        for &byte in data {
            self.send_buffer.push_back(byte);
        }

        // 尝试发送数据
        self.tx_packets()?;

        Ok(data.len())
    }

    /// 发送数据包（核心发送逻辑）
    ///
    /// 从发送缓冲区取数据，构造 TCP 段并发送
    /// 受拥塞窗口和接收窗口限制
    pub fn tx_packets(&mut self) -> Result<(), ()> {
        // 计算在途数据量 (in_flight)
        let in_flight = self.snd_nxt.wrapping_sub(self.snd_una);

        // 计算可用窗口：min(snd_wnd, cwnd) - in_flight
        let usable_window = core::cmp::min(self.snd_wnd as u32, self.congestion.cwnd)
            .saturating_sub(in_flight as u32);

        if usable_window == 0 {
            return Ok(()); // 窗口已满，等待
        }

        let now = crate::drivers::timer::get_jiffies();

        while !self.send_buffer.is_empty() && usable_window > 0 {
            // 计算本次发送大小
            let seg_size = core::cmp::min(
                core::cmp::min(self.mss as usize, usable_window as usize),
                self.send_buffer.len()
            );

            if seg_size == 0 {
                break;
            }

            // 取出数据
            let mut seg_data = alloc::vec::Vec::with_capacity(seg_size);
            for _ in 0..seg_size {
                if let Some(byte) = self.send_buffer.pop_front() {
                    seg_data.push(byte);
                }
            }

            // 发送 TCP 段
            self.tx_segment(&seg_data)?;

            // 将段加入重传队列
            let seg = TcpSendSeg::new(self.snd_nxt, &seg_data, now);
            self.retrans_queue.push_back(seg);

            // 更新序列号
            self.snd_nxt = self.snd_nxt.wrapping_add(seg_size as u32);
        }

        // 启动重传定时器
        if !self.retrans_queue.is_empty() && self.timers.retransmit_deadline == 0 {
            self.start_retransmit_timer();
        }

        Ok(())
    }

    /// 发送单个 TCP 段
    fn tx_segment(&self, data: &[u8]) -> Result<(), ()> {
        let mut skb = crate::net::buffer::alloc_skb(1500).ok_or(())?;

        // 添加数据
        skb.skb_put_data(data)?;

        // 构造 TCP 头部
        tcp_build_packet(
            &mut skb,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            data,
            0x0018, // PSH + ACK
        )?;

        // 发送到 IP 层
        crate::net::ipv4::ipv4_send(skb, self.remote_ip, 6)?;

        Ok(())
    }

    /// 处理 ACK 确认
    ///
    /// 当收到 ACK 时，更新发送窗口、RTT 估算、拥塞控制
    pub fn process_ack(&mut self, ack: TcpSeq) {
        // 检查 ACK 序列号
        if self.seq_before(ack, self.snd_una) {
            // 旧的 ACK，可能是重复 ACK
            self.congestion.dup_ack_count += 1;
            if self.congestion.dup_ack_count >= 3 {
                // 快速重传
                self.congestion.on_dup_ack(ack, self.snd_nxt, self.mss);
                self.fast_retransmit();
            }
            return;
        }

        if self.seq_after(ack, self.snd_nxt) {
            // ACK 超过了发送的数据，忽略
            return;
        }

        // 计算确认的字节数
        let acked_bytes = ack.wrapping_sub(self.snd_una);

        if acked_bytes > 0 {
            // 新的 ACK
            // 1. 从重传队列移除已确认的段
            self.remove_acked_segments(ack);

            // 2. 更新 snd_una
            self.snd_una = ack;

            // 3. 更新 RTT 估算
            self.update_rtt();

            // 4. 拥塞控制：收到新 ACK
            self.congestion.on_ack(acked_bytes, self.mss);
            self.congestion.dup_ack_count = 0;

            // 5. 重置或停止重传定时器
            if !self.retrans_queue.is_empty() {
                self.start_retransmit_timer();
            } else {
                self.timers.stop_retransmit();
            }
        }
    }

    /// 从重传队列移除已确认的段
    fn remove_acked_segments(&mut self, ack: TcpSeq) {
        // 使用关联函数避免借用冲突
        self.retrans_queue.retain(|seg| {
            let seg_end = seg.seq.wrapping_add(seg.len as u32);
            // 序列号比较：seg_end 在 ack 之前
            ((seg_end as i32) - (ack as i32)) < 0
        });
    }

    /// 更新 RTT 估算
    fn update_rtt(&mut self) {
        if let Some(seg) = self.retrans_queue.front() {
            let now = crate::drivers::timer::get_jiffies();
            // jiffies 转微秒 (1 jiffy = 10ms = 10_000us)
            let rtt_us = now.saturating_sub(seg.tx_time) * 10_000;
            if rtt_us > 0 {
                self.rtt_estimator.update(rtt_us);
            }
        }
    }

    /// 快速重传
    fn fast_retransmit(&mut self) {
        if let Some(seg) = self.retrans_queue.front() {
            // 重传最早的段
            let _ = self.tx_segment(&seg.data);
        }
    }

    /// 启动重传定时器
    fn start_retransmit_timer(&mut self) {
        self.timers.start_retransmit(self.rtt_estimator.rto);
    }

    /// 重传定时器到期处理
    ///
    /// 由 TCP 定时器 tick 调用
    pub fn retransmit_timer_expired(&mut self) {
        // 检查重传队列
        if self.retrans_queue.is_empty() {
            self.timers.stop_retransmit();
            return;
        }

        // 先获取需要的信息，避免借用冲突
        let should_close;
        let data_to_retransmit;

        {
            if let Some(seg) = self.retrans_queue.front_mut() {
                if seg.retries >= TCP_MAX_RETRIES {
                    // 超过最大重传次数，关闭连接
                    should_close = true;
                    data_to_retransmit = None;
                } else {
                    should_close = false;
                    // 复制数据用于重传
                    data_to_retransmit = Some(seg.data.clone());
                    // 增加重传次数
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

        // 拥塞控制：超时处理
        self.congestion.on_timeout(self.mss);

        // 重传
        if let Some(data) = data_to_retransmit {
            let _ = self.tx_segment(&data);
        }

        // RTO 指数退避
        self.rtt_estimator.backoff();

        // 重新设置定时器
        self.start_retransmit_timer();
    }

    /// 序列号比较：a 在 b 之前（考虑回绕）
    #[inline]
    fn seq_before(&self, a: TcpSeq, b: TcpSeq) -> bool {
        ((a as i32) - (b as i32)) < 0
    }

    /// 序列号比较：a 在 b 之后（考虑回绕）
    #[inline]
    fn seq_after(&self, a: TcpSeq, b: TcpSeq) -> bool {
        self.seq_before(b, a)
    }

    /// 更新接收窗口
    pub fn update_rcv_wnd(&mut self) {
        // 接收窗口 = 缓冲区大小 - 已使用空间
        let used = self.recv_buffer.len() as u16;
        self.rcv_wnd = TCP_MAX_WINDOW.saturating_sub(used);
    }
}

/// TCP 连接管理器
///
/// 管理所有 TCP 连接，处理接收到的 TCP 包
pub struct TcpConnectionManager {
    /// 监听 Socket 列表
    listen_sockets: alloc::vec::Vec<TcpSocket>,
    /// 已建立的连接
    established_connections: alloc::vec::Vec<TcpSocket>,
    /// 待处理连接队列（用于 accept）
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

    /// 添加监听 Socket
    pub fn add_listen_socket(&mut self, socket: TcpSocket) {
        self.listen_sockets.push(socket);
    }

    /// 处理接收到的 TCP 包
    ///
    /// 根据目标端口和状态分发到对应的 Socket
    pub fn handle_tcp_packet(&mut self, skb: &SkBuff, src_ip: u32, dest_port: TcpPort) -> Result<(), ()> {
        // 解析 TCP 头部
        let tcp_hdr = match tcp_parse_packet(skb) {
            Some(hdr) => hdr,
            None => return Ok(()),
        };

        let src_port = TcpPort::from_be(tcp_hdr.source);

        // 查找匹配的 Socket
        // 1. 首先检查已建立的连接
        for socket in &mut self.established_connections.iter_mut() {
            if socket.local_port == dest_port
                && socket.remote_port == src_port
                && socket.remote_ip == src_ip
            {
                // 找到匹配的连接，处理包
                let _ = socket.handle_packet(tcp_hdr, unsafe {
                    core::slice::from_raw_parts(
                        skb.data.add(tcp_hdr.header_len()),
                        (skb.len as usize - tcp_hdr.header_len())
                    )
                });
                return Ok(());
            }
        }

        // 2. 检查监听 Socket
        for socket in &mut self.listen_sockets.iter_mut() {
            if socket.local_port == dest_port && socket.state == TcpState::TCP_LISTEN {
                // 创建新的连接
                let mut new_socket = TcpSocket::new();
                new_socket.local_port = dest_port;
                new_socket.remote_port = src_port;
                new_socket.remote_ip = src_ip;
                new_socket.state = TcpState::TCP_SYN_RECV;

                // 处理 SYN 包
                if tcp_hdr.syn() && !tcp_hdr.ack() {
                    let _ = new_socket.handle_packet(tcp_hdr, &[]);

                    // 将连接加入待处理队列
                    self.pending_connections.push(new_socket);
                }
                return Ok(());
            }
        }

        // 3. 检查待处理连接（SYN_SENT 状态）
        let mut idx_to_move: Option<usize> = None;
        for (idx, socket) in self.pending_connections.iter_mut().enumerate() {
            if socket.local_port == dest_port
                && socket.remote_port == src_port
                && socket.remote_ip == src_ip
            {
                let _ = socket.handle_packet(tcp_hdr, unsafe {
                    core::slice::from_raw_parts(
                        skb.data.add(tcp_hdr.header_len()),
                        (skb.len as usize - tcp_hdr.header_len())
                    )
                });

                // 如果连接建立，标记要移动到已建立连接列表
                if socket.state == TcpState::TCP_ESTABLISHED {
                    idx_to_move = Some(idx);
                }
                break;
            }
        }

        // 移动已建立的连接（如果在循环外）
        if let Some(idx) = idx_to_move {
            let socket = self.pending_connections.remove(idx);
            self.established_connections.push(socket);
        }

        Ok(())
    }
}

/// 全局 TCP 连接管理器
static mut TCP_CONNECTION_MANAGER: core::mem::MaybeUninit<TcpConnectionManager> = core::mem::MaybeUninit::<TcpConnectionManager>::uninit();

/// 初始化 TCP 连接管理器
pub fn init_tcp_manager() {
    unsafe {
        TCP_CONNECTION_MANAGER.write(TcpConnectionManager::new());
    }
}

/// 获取 TCP 连接管理器
pub fn get_tcp_manager() -> &'static mut TcpConnectionManager {
    unsafe { TCP_CONNECTION_MANAGER.assume_init_mut() }
}

/// 全局 TCP Socket 表
///
/// 简化实现：固定大小的 Socket 表
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

    /// 分配 Socket
    fn alloc(&mut self) -> Result<usize, ()> {
        if self.count >= TCP_SOCKET_TABLE_SIZE {
            return Err(());
        }

        let fd = self.count;
        self.sockets[fd] = Some(TcpSocket::new());
        self.count += 1;
        Ok(fd)
    }

    /// 分配 Socket 槽位（不初始化）
    fn alloc_slot(&mut self) -> Result<usize, ()> {
        if self.count >= TCP_SOCKET_TABLE_SIZE {
            return Err(());
        }

        let fd = self.count;
        self.count += 1;
        Ok(fd)
    }

    /// 安装 Socket 到指定槽位
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

    /// 释放 Socket
    fn free(&mut self, fd: usize) {
        if fd < self.count {
            self.sockets[fd] = None;
            // 不减少 count，简化实现
        }
    }

    /// 获取 Socket
    fn get(&self, fd: usize) -> Option<&TcpSocket> {
        if fd < self.count {
            self.sockets[fd].as_ref()
        } else {
            None
        }
    }

    /// 获取可变 Socket
    fn get_mut(&mut self, fd: usize) -> Option<&mut TcpSocket> {
        if fd < self.count {
            self.sockets[fd].as_mut()
        } else {
            None
        }
    }

    /// 获取所有 socket 的可变引用（用于定时器）
    pub fn sockets_mut(&mut self) -> &mut [Option<TcpSocket>; TCP_SOCKET_TABLE_SIZE] {
        &mut self.sockets
    }
}/// 全局 TCP Socket 表
static mut TCP_SOCKET_TABLE: TcpSocketTable = TcpSocketTable::new();

/// 分配 TCP Socket
///
/// # 返回
/// 返回 Socket 文件描述符
pub fn tcp_socket_alloc() -> Result<i32, i32> {
    unsafe {
        match TCP_SOCKET_TABLE.alloc() {
            Ok(fd) => Ok(fd as i32),
            Err(_) => Err(-5), // EIO
        }
    }
}

/// 释放 TCP Socket
///
/// # 参数
/// - `fd`: Socket 文件描述符
pub fn tcp_socket_free(fd: i32) {
    unsafe {
        TCP_SOCKET_TABLE.free(fd as usize);
    }
}

/// 获取 TCP Socket 表的可变引用（用于定时器）
///
/// # Safety
/// 此函数返回全局 socket 表的可变引用，调用者需要确保同步
pub fn get_tcp_socket_table() -> &'static mut TcpSocketTable {
    unsafe { &mut TCP_SOCKET_TABLE }
}

/// 获取 TCP Socket
///
/// # 参数
/// - `fd`: Socket 文件描述符
///
/// # 返回
/// 返回 Socket 引用
pub fn tcp_socket_get(fd: i32) -> Option<&'static mut TcpSocket> {
    unsafe {
        TCP_SOCKET_TABLE.get_mut(fd as usize)
    }
}

/// 绑定 Socket 到端口
///
/// # 参数
/// - `fd`: Socket 文件描述符
/// - `port`: 端口号
///
/// # 返回
/// 成功返回 0，失败返回错误码
pub fn tcp_bind(fd: i32, port: TcpPort) -> i32 {
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

/// 监听端口
///
/// # 参数
/// - `fd`: Socket 文件描述符
/// - `backlog`: 等待队列长度
///
/// # 返回
/// 成功返回 0，失败返回错误码
pub fn tcp_listen(fd: i32, backlog: u32) -> i32 {
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

/// 连接到远程地址
///
/// # 参数
/// - `fd`: Socket 文件描述符
/// - `ip`: IP 地址
/// - `port`: 端口号
///
/// # 返回
/// 成功返回 0，失败返回错误码
pub fn tcp_connect(fd: i32, ip: u32, port: TcpPort) -> i32 {
    unsafe {
        if let Some(socket) = TCP_SOCKET_TABLE.get_mut(fd as usize) {
            match socket.connect(ip, port) {
                Ok(()) => 0,
                Err(()) => -5, // EIO
            }
        } else {
            -5 // EBADF
        }
    }
}

/// 接受连接
///
/// # 参数
/// - `fd`: Socket 文件描述符（监听 socket）
///
/// # 返回
/// 成功返回新的 Socket 文件描述符，失败返回错误码
pub fn tcp_accept(fd: i32) -> i32 {
    unsafe {
        // 检查监听 socket 是否有效
        let listen_socket = match TCP_SOCKET_TABLE.get(fd as usize) {
            Some(s) => s,
            None => return -9, // EBADF
        };

        // 确保是监听状态
        if listen_socket.state != TcpState::TCP_LISTEN {
            return -22; // EINVAL
        }

        let local_port = listen_socket.local_port;

        // 获取 TCP 连接管理器
        let manager = get_tcp_manager();

        // 查找已建立的连接（从 pending_connections 中）
        let established_idx = manager.pending_connections.iter().position(|s| {
            s.state == TcpState::TCP_ESTABLISHED && s.local_port == local_port
        });

        match established_idx {
            Some(idx) => {
                // 取出已建立的连接
                let new_socket = manager.pending_connections.remove(idx);

                // 为新连接分配 socket fd
                let new_fd = match TCP_SOCKET_TABLE.alloc_slot() {
                    Ok(fd) => fd as i32,
                    Err(_) => {
                        // 放回队列
                        manager.pending_connections.push(new_socket);
                        return -24; // EMFILE
                    }
                };

                // 将新 socket 放入表
                if TCP_SOCKET_TABLE.install(new_fd as usize, new_socket).is_err() {
                    return -5; // EIO
                }

                new_fd
            }
            None => -11, // EAGAIN (没有待处理的连接)
        }
    }
}

/// 计算 TCP 校验和
///
/// # 参数
/// - `shdr`: 源 IP 地址 (网络字节序)
/// - `dhdr`: 目标 IP 地址 (网络字节序)
/// - `thdr`: TCP 头部
/// - `data`: 数据
///
/// # 返回
/// 校验和 (网络字节序)
pub fn tcp_checksum(shdr: u32, dhdr: u32, thdr: &TcpHdr, data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // 伪头部 (12 字节)
    // 源 IP (4 字节)
    sum += (shdr >> 16) & 0xFFFF;
    sum += shdr & 0xFFFF;
    // 目标 IP (4 字节)
    sum += (dhdr >> 16) & 0xFFFF;
    sum += dhdr & 0xFFFF;
    // 保留 (1 字节) + 协议 (1 字节) + TCP 长度 (2 字节)
    sum += (6u32 << 8); // TCP 协议号
    let tcp_len = (thdr.header_len() + data.len()) as u16;
    sum += tcp_len as u32;

    // TCP 头部 (假设最小 20 字节)
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(
            (thdr as *const TcpHdr) as *const u8,
            thdr.header_len().min(20)
        )
    };

    let mut i = 0;
    while i + 1 < hdr_bytes.len() {
        let word = u16::from_be_bytes([hdr_bytes[i], hdr_bytes[i + 1]]) as u32;
        sum += word;
        i += 2;
    }

    // 数据
    let mut i = 0;
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        sum += word;
        i += 2;
    }

    // 处理最后一个字节 (如果有)
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    // 处理进位
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // 取反
    !sum as u16
}

/// 构造 TCP 数据包
///
/// # 参数
/// - `skb`: SkBuff
/// - `source`: 源端口
/// - `dest`: 目标端口
/// - `seq`: 序列号
/// - `ack_seq`: 确认号
/// - `data`: 数据
/// - `flags`: 标志位
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn tcp_build_packet(
    skb: &mut SkBuff,
    source: TcpPort,
    dest: TcpPort,
    seq: TcpSeq,
    ack_seq: TcpAck,
    data: &[u8],
    flags: u16,
) -> Result<(), ()> {
    // 分配空间用于 TCP 头部
    let ptr = skb.skb_push(TCP_MIN_HLEN as u32).ok_or(())?;

    unsafe {
        let tcp_hdr = &mut *(ptr as *mut TcpHdr);

        // 源端口
        tcp_hdr.source = source.to_be();

        // 目标端口
        tcp_hdr.dest = dest.to_be();

        // 序列号
        tcp_hdr.seq = seq.to_be();

        // 确认号
        tcp_hdr.ack_seq = ack_seq.to_be();

        // 数据偏移 (20 字节 = 5 个 32 位字)
        tcp_hdr.set_dof(5);

        // 标志和窗口
        tcp_hdr.flags_win = flags.to_be();

        // 窗口大小
        tcp_hdr.set_window(TCP_MAX_WINDOW);

        // 校验和 (先设为 0，稍后计算)
        tcp_hdr.check = 0;

        // 紧急指针
        tcp_hdr.urg_ptr = 0;
    }

    // 添加数据
    skb.skb_put_data(data)?;

    // TODO: 计算 TCP 校验和 (需要源 IP 和目标 IP)
    // tcp_hdr.check = tcp_checksum(...).to_be();

    Ok(())
}

/// 解析 TCP 数据包
///
/// # 参数
/// - `skb`: SkBuff (包含 TCP 数据包)
///
/// # 返回
/// 返回 TCP 头部引用，如果解析失败则返回 None
pub fn tcp_parse_packet(skb: &SkBuff) -> Option<&'static TcpHdr> {
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < TCP_MIN_HLEN {
        return None;
    }

    let tcp_hdr = TcpHdr::from_bytes(data)?;

    // 验证头部长度
    let hdr_len = tcp_hdr.header_len();
    if hdr_len < TCP_MIN_HLEN || hdr_len > TCP_MAX_HLEN {
        return None;
    }

    // TODO: 验证 TCP 校验和
    // if tcp_hdr.check() != 0 && tcp_hdr.check() != 0xFFFF {
    //     return None;
    // }

    Some(tcp_hdr)
}

/// 接收并处理 TCP 数据包
///
/// # 参数
/// - `skb`: SkBuff (包含 TCP 数据包)
/// - `src_ip`: 源 IP 地址
/// - `dest_ip`: 目标 IP 地址
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn tcp_rcv(skb: &SkBuff, src_ip: u32, dest_ip: u32) -> Result<(), ()> {
    // 解析 TCP 头部
    let tcp_hdr = tcp_parse_packet(skb).ok_or(())?;

    let src_port = TcpPort::from_be(tcp_hdr.source);
    let dest_port = TcpPort::from_be(tcp_hdr.dest);

    // 获取 TCP 连接管理器
    let manager = get_tcp_manager();

    // 获取 TCP 头部长度后的数据
    let header_len = tcp_hdr.header_len();
    let data = unsafe {
        if (skb.len as usize) > header_len {
            let data_ptr = skb.data.add(header_len);
            let data_len = skb.len as usize - header_len;
            core::slice::from_raw_parts(data_ptr, data_len)
        } else {
            &[]
        }
    };

    // 查找匹配的连接
    // 1. 首先检查已建立的连接
    for socket in manager.established_connections.iter_mut() {
        if socket.local_port == dest_port
            && socket.remote_port == src_port
            && socket.remote_ip == src_ip
        {
            let _ = socket.handle_packet(tcp_hdr, data);
            return Ok(());
        }
    }

    // 2. 检查监听 Socket（新的连接请求）
    for socket in manager.listen_sockets.iter_mut() {
        if socket.local_port == dest_port && socket.state == TcpState::TCP_LISTEN {
            // 创建新的连接 socket
            let mut new_socket = TcpSocket::new();
            new_socket.local_port = dest_port;
            new_socket.local_ip = dest_ip;
            new_socket.remote_port = src_port;
            new_socket.remote_ip = src_ip;

            // 处理 SYN 包
            if tcp_hdr.syn() && !tcp_hdr.ack() {
                let _ = new_socket.handle_packet(tcp_hdr, &[]);

                // 将连接加入待处理队列
                manager.pending_connections.push(new_socket);
            }
            return Ok(());
        }
    }

    // 3. 检查待处理连接（SYN_SENT 状态）
    let mut idx_to_move: Option<usize> = None;
    for (idx, socket) in manager.pending_connections.iter_mut().enumerate() {
        if socket.local_port == dest_port
            && socket.remote_port == src_port
            && socket.remote_ip == src_ip
        {
            let _ = socket.handle_packet(tcp_hdr, data);

            // 如果连接建立，标记要移动到已建立连接列表
            if socket.state == TcpState::TCP_ESTABLISHED {
                idx_to_move = Some(idx);
            }
            break;
        }
    }

    // 移动已建立的连接
    if let Some(idx) = idx_to_move {
        let socket = manager.pending_connections.remove(idx);
        manager.established_connections.push(socket);
    }

    Ok(())
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
