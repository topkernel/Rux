//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TCP 协议
//!
//! 完全遵循 Linux 内核的 TCP 实现
//! 参考: net/ipv4/tcp.c, include/net/tcp.h, include/uapi/linux/tcp.h

use crate::net::buffer::SkBuff;
use crate::net::ipv4::{route, checksum};

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
/// 对应 Linux 的 tcphdr (include/uapi/linux/tcp.h)
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
/// 对应 Linux 的 TCP 状态机
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

    /// 连接到远程地址
    ///
    /// # 参数
    /// - `ip`: IP 地址
    /// - `port`: 端口号
    pub fn connect(&mut self, ip: u32, port: TcpPort) -> Result<(), ()> {
        self.remote_ip = ip;
        self.remote_port = port;
        self.state = TcpState::TCP_SYN_SENT;

        // TODO: 发送 SYN 包
        // 初始化序列号
        self.snd_nxt = 12345; // 简化实现：固定初始序列号
        self.snd_una = self.snd_nxt;

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

/// 全局 TCP Socket 表
///
/// 简化实现：固定大小的 Socket 表
const TCP_SOCKET_TABLE_SIZE: usize = 64;

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
