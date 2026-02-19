//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP 协议
//!
//! 完全...

use crate::net::buffer::SkBuff;
use crate::net::ipv4::{route, checksum};
use crate::config::TCP_SOCKET_TABLE_SIZE;

/// TCP 头部长度
pub const TCP_MIN_HLEN: usize = 20;
pub const TCP_MAX_HLEN: usize = 60;

/// TCP 最大窗口大小
pub const TCP_MAX_WINDOW: u16 = 65535;

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

/// TCP Socket 结构
///
/// 简化实现：包含连接状态、序列号等
#[repr(C)]
pub struct TcpSocket {
    /// 本地端口
    pub local_port: TcpPort,
    /// 远程端口
    pub remote_port: TcpPort,
    /// 远程 IP 地址
    pub remote_ip: u32,
    /// TCP 状态
    pub state: TcpState,
    /// 发送序列号
    pub snd_nxt: TcpSeq,
    /// 发送未确认序列号
    pub snd_una: TcpSeq,
    /// 接收序列号
    pub rcv_nxt: TcpSeq,
    /// 窗口大小
    pub window: u16,
    /// 是否已绑定
    pub bound: bool,
}

impl TcpSocket {
    /// 创建新的 TCP Socket
    pub fn new() -> Self {
        Self {
            local_port: 0,
            remote_port: 0,
            remote_ip: 0,
            state: TcpState::TCP_CLOSE,
            snd_nxt: 0,
            snd_una: 0,
            rcv_nxt: 0,
            window: TCP_MAX_WINDOW,
            bound: false,
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

        // TODO: 将数据放入接收队列

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

        // TODO: 发送数据包
        // 更新序列号
        self.snd_nxt += data.len() as u32;

        Ok(data.len())
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

        // TODO: 从接收队列获取数据
        // 更新接收序列号

        Ok(0) // 简化实现：暂时返回 0
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
struct TcpSocketTable {
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
}

/// 全局 TCP Socket 表
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
/// - `fd`: Socket 文件描述符
///
/// # 返回
/// 成功返回新的 Socket 文件描述符，失败返回错误码
pub fn tcp_accept(fd: i32) -> i32 {
    unsafe {
        if let Some(_socket) = TCP_SOCKET_TABLE.get(fd as usize) {
            // TODO: 实现完整的 accept 逻辑
            // 1. 从队列中获取待处理的连接
            // 2. 创建新的 Socket
            // 3. 返回新的文件描述符

            // 简化实现：返回错误
            -5 // EIO
        } else {
            -5 // EBADF
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
