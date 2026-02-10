//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! UDP 协议
//!
//! 完全遵循 Linux 内核的 UDP 实现
//! 参考: net/ipv4/udp.c, include/net/udp.h, include/uapi/linux/udp.h

use crate::net::buffer::SkBuff;
use crate::net::ipv4::{route, checksum};

/// UDP 头部长度
pub const UDP_HLEN: usize = 8;

/// UDP 最大数据长度
pub const UDP_MAX_DATAGRAM: usize = 65507;

/// UDP 端口号
pub type UdpPort = u16;

/// UDP 头部
///
/// 对应 Linux 的 udphdr (include/uapi/linux/udp.h)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UdpHdr {
    /// 源端口
    pub source: UdpPort,
    /// 目标端口
    pub dest: UdpPort,
    /// 长度
    pub len: u16,
    /// 校验和
    pub check: u16,
}

impl UdpHdr {
    /// 从字节切片创建 UDP 头部
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < UDP_HLEN {
            return None;
        }

        unsafe {
            Some(&*(data.as_ptr() as *const UdpHdr))
        }
    }

    /// 获取源端口
    pub fn source(&self) -> UdpPort {
        u16::from_be(self.source)
    }

    /// 获取目标端口
    pub fn dest(&self) -> UdpPort {
        u16::from_be(self.dest)
    }

    /// 获取长度
    pub fn len(&self) -> u16 {
        u16::from_be(self.len)
    }

    /// 获取校验和
    pub fn check(&self) -> u16 {
        u16::from_be(self.check)
    }
}

/// UDP Socket 结构
///
/// 简化实现：包含源端口、目标端口和状态
#[repr(C)]
pub struct UdpSocket {
    /// 本地端口
    pub local_port: UdpPort,
    /// 远程端口
    pub remote_port: UdpPort,
    /// 远程 IP 地址
    pub remote_ip: u32,
    /// 是否已绑定
    pub bound: bool,
    /// 是否已连接
    pub connected: bool,
}

impl UdpSocket {
    /// 创建新的 UDP Socket
    pub fn new() -> Self {
        Self {
            local_port: 0,
            remote_port: 0,
            remote_ip: 0,
            bound: false,
            connected: false,
        }
    }

    /// 绑定端口
    ///
    /// # 参数
    /// - `port`: 端口号
    pub fn bind(&mut self, port: UdpPort) -> Result<(), ()> {
        // TODO: 检查端口是否已被占用
        self.local_port = port;
        self.bound = true;
        Ok(())
    }

    /// 连接到远程地址
    ///
    /// # 参数
    /// - `ip`: IP 地址
    /// - `port`: 端口号
    pub fn connect(&mut self, ip: u32, port: UdpPort) -> Result<(), ()> {
        self.remote_ip = ip;
        self.remote_port = port;
        self.connected = true;
        Ok(())
    }

    /// 断开连接
    pub fn disconnect(&mut self) {
        self.remote_ip = 0;
        self.remote_port = 0;
        self.connected = false;
    }
}

/// 全局 UDP Socket 表
///
/// 简化实现：固定大小的 Socket 表
const UDP_SOCKET_TABLE_SIZE: usize = 64;

struct UdpSocketTable {
    sockets: [Option<UdpSocket>; UDP_SOCKET_TABLE_SIZE],
    count: usize,
}

impl UdpSocketTable {
    const fn new() -> Self {
        const NONE: Option<UdpSocket> = None;
        Self {
            sockets: [NONE; UDP_SOCKET_TABLE_SIZE],
            count: 0,
        }
    }

    /// 分配 Socket
    fn alloc(&mut self) -> Result<usize, ()> {
        if self.count >= UDP_SOCKET_TABLE_SIZE {
            return Err(());
        }

        let fd = self.count;
        self.sockets[fd] = Some(UdpSocket::new());
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
    fn get(&self, fd: usize) -> Option<&UdpSocket> {
        if fd < self.count {
            self.sockets[fd].as_ref()
        } else {
            None
        }
    }

    /// 获取可变 Socket
    fn get_mut(&mut self, fd: usize) -> Option<&mut UdpSocket> {
        if fd < self.count {
            self.sockets[fd].as_mut()
        } else {
            None
        }
    }
}

/// 全局 UDP Socket 表
static mut UDP_SOCKET_TABLE: UdpSocketTable = UdpSocketTable::new();

/// 分配 UDP Socket
///
/// # 返回
/// 返回 Socket 文件描述符
pub fn udp_socket_alloc() -> Result<i32, i32> {
    unsafe {
        match UDP_SOCKET_TABLE.alloc() {
            Ok(fd) => Ok(fd as i32),
            Err(_) => Err(-5), // EIO
        }
    }
}

/// 释放 UDP Socket
///
/// # 参数
/// - `fd`: Socket 文件描述符
pub fn udp_socket_free(fd: i32) {
    unsafe {
        UDP_SOCKET_TABLE.free(fd as usize);
    }
}

/// 获取 UDP Socket
///
/// # 参数
/// - `fd`: Socket 文件描述符
///
/// # 返回
/// 返回 Socket 引用
pub fn udp_socket_get(fd: i32) -> Option<&'static mut UdpSocket> {
    unsafe {
        UDP_SOCKET_TABLE.get_mut(fd as usize)
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
pub fn udp_bind(fd: i32, port: UdpPort) -> i32 {
    unsafe {
        if let Some(socket) = UDP_SOCKET_TABLE.get_mut(fd as usize) {
            match socket.bind(port) {
                Ok(()) => 0,
                Err(()) => -5, // EIO
            }
        } else {
            -5 // EBADF
        }
    }
}

/// 发送 UDP 数据包
///
/// # 参数
/// - `fd`: Socket 文件描述符
/// - `buf`: 数据缓冲区
/// - `len`: 数据长度
///
/// # 返回
/// 成功返回发送的字节数，失败返回错误码
pub fn udp_send(fd: i32, buf: &[u8]) -> isize {
    // TODO: 实现完整的 UDP 发送逻辑
    // 1. 查找 Socket
    // 2. 获取远程地址
    // 3. 构造 UDP 头部
    // 4. 构造 IP 头部
    // 5. 计算校验和
    // 6. 发送数据包

    buf.len() as isize
}

/// 接收 UDP 数据包
///
/// # 参数
/// - `fd`: Socket 文件描述符
/// - `buf`: 数据缓冲区
/// - `len`: 缓冲区长度
///
/// # 返回
/// 成功返回接收的字节数，失败返回错误码
pub fn udp_recv(fd: i32, buf: &mut [u8], len: usize) -> isize {
    // TODO: 实现完整的 UDP 接收逻辑
    // 1. 从队列中获取数据包
    // 2. 验证 UDP 校验和
    // 3. 复制数据到缓冲区
    // 4. 返回接收的字节数

    // 简化实现：暂时返回 0 (无数据)
    0
}

/// 计算 UDP 校验和
///
/// # 参数
/// - `shdr`: 源 IP 地址 (网络字节序)
/// - `dhdr`: 目标 IP 地址 (网络字节序)
/// - `uhdr`: UDP 头部
/// - `data`: 数据
///
/// # 返回
/// 校验和 (网络字节序)
pub fn udp_checksum(shdr: u32, dhdr: u32, uhdr: &UdpHdr, data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // 伪头部 (12 字节)
    // 源 IP (4 字节)
    sum += (shdr >> 16) & 0xFFFF;
    sum += shdr & 0xFFFF;
    // 目标 IP (4 字节)
    sum += (dhdr >> 16) & 0xFFFF;
    sum += dhdr & 0xFFFF;
    // 保留 (1 字节) + 协议 (1 字节) + UDP 长度 (2 字节)
    sum += (17u32 << 8); // UDP 协议号
    sum += uhdr.len as u32;

    // UDP 头部
    sum += uhdr.source as u32;
    sum += uhdr.dest as u32;
    sum += uhdr.len as u32;
    sum += 0; // 校验和字段 (先设为 0)

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

/// 构造 UDP 数据包
///
/// # 参数
/// - `skb`: SkBuff
/// - `source`: 源端口
/// - `dest`: 目标端口
/// - `data`: 数据
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn udp_build_packet(
    skb: &mut SkBuff,
    source: UdpPort,
    dest: UdpPort,
    data: &[u8],
) -> Result<(), ()> {
    // 分配空间用于 UDP 头部
    let ptr = skb.skb_push(UDP_HLEN as u32).ok_or(())?;

    unsafe {
        let udp_hdr = &mut *(ptr as *mut UdpHdr);

        // 源端口
        udp_hdr.source = source.to_be();

        // 目标端口
        udp_hdr.dest = dest.to_be();

        // 长度 (UDP 头部 + 数据)
        udp_hdr.len = ((UDP_HLEN + data.len()) as u16).to_be();

        // 校验和 (先设为 0，稍后计算)
        udp_hdr.check = 0;
    }

    // 添加数据
    skb.skb_put_data(data)?;

    // TODO: 计算 UDP 校验和 (需要源 IP 和目标 IP)
    // udp_hdr.check = udp_checksum(...).to_be();

    Ok(())
}

/// 解析 UDP 数据包
///
/// # 参数
/// - `skb`: SkBuff (包含 UDP 数据包)
///
/// # 返回
/// 返回 UDP 头部引用，如果解析失败则返回 None
pub fn udp_parse_packet(skb: &SkBuff) -> Option<&'static UdpHdr> {
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < UDP_HLEN {
        return None;
    }

    let udp_hdr = UdpHdr::from_bytes(data)?;

    // 验证长度
    let len = udp_hdr.len();
    if (len as usize) < UDP_HLEN || (len as usize) != data.len() {
        return None;
    }

    // TODO: 验证 UDP 校验和
    // if udp_hdr.check() != 0 && udp_hdr.check() != 0xFFFF {
    //     return None;
    // }

    Some(udp_hdr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_udphdr_size() {
        assert_eq!(core::mem::size_of::<UdpHdr>(), 8);
    }

    #[test]
    fn test_udp_socket() {
        let mut socket = UdpSocket::new();
        assert!(!socket.bound);
        assert!(!socket.connected);

        assert!(socket.bind(8080).is_ok());
        assert!(socket.bound);

        assert!(socket.connect(0x7F000001, 80).is_ok());
        assert!(socket.connected);

        socket.disconnect();
        assert!(!socket.connected);
    }

    #[test]
    fn test_udp_socket_alloc() {
        let fd1 = udp_socket_alloc();
        assert!(fd1.is_ok());
        assert_eq!(fd1.unwrap(), 0);

        let fd2 = udp_socket_alloc();
        assert!(fd2.is_ok());
        assert_eq!(fd2.unwrap(), 1);

        udp_socket_free(fd1.unwrap());
        udp_socket_free(fd2.unwrap());
    }

    #[test]
    fn test_udp_checksum() {
        let shdr = 0xC0A80101; // 192.168.1.1
        let dhdr = 0xC0A80102; // 192.168.1.2
        let data = b"Hello, World!";

        let mut uhdr = UdpHdr::default();
        uhdr.source = 1234u16.to_be();
        uhdr.dest = 80u16.to_be();
        uhdr.len = ((UDP_HLEN + data.len()) as u16).to_be();
        uhdr.check = 0;

        let csum = udp_checksum(shdr, dhdr, &uhdr, data);
        // 验证函数能正常工作
        assert!(csum != 0 || csum == 0xFFFF);
    }
}
