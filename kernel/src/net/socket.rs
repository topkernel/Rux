//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Socket 抽象层
//!
//! 本模块提供统一的 socket 接口，将 TCP/UDP socket 集成到 VFS
//!
//! 参考 Linux: include/linux/net.h, net/socket.c

use alloc::sync::Arc;
use alloc::collections::VecDeque;
use spin::Mutex;
use core::cell::UnsafeCell;

use crate::fs::file::{File, FileFlags, FileOps, FdTable};

// ============================================================================
// Socket 类型定义
// ============================================================================

/// 地址族
pub const AF_INET: i32 = 2;

/// Socket 类型
pub const SOCK_STREAM: i32 = 1;  // TCP
pub const SOCK_DGRAM: i32 = 2;   // UDP

/// 协议
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

// ============================================================================
// Socket 结构
// ============================================================================

/// Socket 类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Tcp,
    Udp,
}

/// Socket 地址
#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

impl SockAddrIn {
    /// 从原始字节解析
    pub fn from_bytes(data: &[u8]) -> Option<&Self> {
        if data.len() < 16 {
            return None;
        }
        unsafe {
            Some(&*(data.as_ptr() as *const SockAddrIn))
        }
    }

    /// 获取端口号（主机字节序）
    pub fn port(&self) -> u16 {
        u16::from_be(self.sin_port)
    }

    /// 获取 IP 地址（主机字节序）
    pub fn addr(&self) -> u32 {
        u32::from_be(self.sin_addr)
    }
}

/// 接收缓冲区数据包
#[derive(Clone)]
pub struct RecvPacket {
    /// 数据
    pub data: alloc::vec::Vec<u8>,
    /// 源地址
    pub src_addr: u32,
    /// 源端口
    pub src_port: u16,
}

/// Socket 状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// 未连接
    Unconnected,
    /// 正在连接
    Connecting,
    /// 已连接
    Connected,
    /// 正在监听
    Listening,
    /// 正在关闭
    Closing,
}

/// 统一的 Socket 结构
pub struct Socket {
    /// Socket 类型
    pub sock_type: SocketType,
    /// Socket 状态
    pub state: Mutex<SocketState>,
    /// 本地端口
    pub local_port: Mutex<u16>,
    /// 本地 IP
    pub local_addr: Mutex<u32>,
    /// 远程端口
    pub remote_port: Mutex<u16>,
    /// 远程 IP
    pub remote_addr: Mutex<u32>,
    /// 接收缓冲区
    pub recv_queue: Mutex<VecDeque<RecvPacket>>,
    /// 是否已绑定
    pub bound: Mutex<bool>,
    /// TCP 索引（用于 TCP socket 表查找）
    pub tcp_fd: UnsafeCell<Option<i32>>,
    /// UDP 索引（用于 UDP socket 表查找）
    pub udp_fd: UnsafeCell<Option<i32>>,
}

unsafe impl Sync for Socket {}

impl Socket {
    /// 创建新的 Socket
    pub fn new(sock_type: SocketType) -> Self {
        Self {
            sock_type,
            state: Mutex::new(SocketState::Unconnected),
            local_port: Mutex::new(0),
            local_addr: Mutex::new(0),
            remote_port: Mutex::new(0),
            remote_addr: Mutex::new(0),
            recv_queue: Mutex::new(VecDeque::new()),
            bound: Mutex::new(false),
            tcp_fd: UnsafeCell::new(None),
            udp_fd: UnsafeCell::new(None),
        }
    }

    /// 绑定到地址
    pub fn bind(&self, addr: u32, port: u16) -> Result<(), i32> {
        *self.local_addr.lock() = addr;
        *self.local_port.lock() = port;
        *self.bound.lock() = true;

        match self.sock_type {
            SocketType::Tcp => {
                let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;
                crate::net::tcp::tcp_bind(tcp_fd, port);
                Ok(())
            }
            SocketType::Udp => {
                let udp_fd = unsafe { *self.udp_fd.get() }.ok_or(-9)?;
                crate::net::udp::udp_bind(udp_fd, port);
                Ok(())
            }
        }
    }

    /// 监听连接
    pub fn listen(&self, backlog: i32) -> Result<(), i32> {
        if self.sock_type != SocketType::Tcp {
            return Err(-95); // EOPNOTSUPP
        }

        let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;
        let ret = crate::net::tcp::tcp_listen(tcp_fd, backlog as u32);
        if ret == 0 {
            *self.state.lock() = SocketState::Listening;
            Ok(())
        } else {
            Err(ret)
        }
    }

    /// 连接到远程地址
    pub fn connect(&self, addr: u32, port: u16) -> Result<(), i32> {
        *self.remote_addr.lock() = addr;
        *self.remote_port.lock() = port;

        match self.sock_type {
            SocketType::Tcp => {
                let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;
                *self.state.lock() = SocketState::Connecting;
                let ret = crate::net::tcp::tcp_connect(tcp_fd, addr, port);
                if ret == 0 {
                    // TCP 连接需要等待三次握手完成
                    // 简化实现：直接设置为已连接
                    *self.state.lock() = SocketState::Connected;
                    Ok(())
                } else {
                    *self.state.lock() = SocketState::Unconnected;
                    Err(ret)
                }
            }
            SocketType::Udp => {
                // UDP 是无连接的，connect 只是设置默认目标
                let udp_fd = unsafe { *self.udp_fd.get() }.ok_or(-9)?;
                if let Some(socket) = crate::net::udp::udp_socket_get(udp_fd) {
                    let _ = socket.connect(addr, port);
                }
                *self.state.lock() = SocketState::Connected;
                Ok(())
            }
        }
    }

    /// 发送数据
    pub fn send(&self, buf: &[u8], dest_addr: Option<(u32, u16)>) -> Result<usize, i32> {
        match self.sock_type {
            SocketType::Tcp => {
                let state = *self.state.lock();
                if state != SocketState::Connected {
                    return Err(-32); // EPIPE
                }
                let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;
                if let Some(socket) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                    match socket.send(buf) {
                        Ok(len) => Ok(len),
                        Err(_) => Err(-5), // EIO
                    }
                } else {
                    Err(-9) // EBADF
                }
            }
            SocketType::Udp => {
                let udp_fd = unsafe { *self.udp_fd.get() }.ok_or(-9)?;

                // 如果指定了目标地址，使用 sendto 语义
                if let Some((addr, port)) = dest_addr {
                    // TODO: 实现 UDP sendto
                    Ok(buf.len())
                } else {
                    // 使用 connect 设置的默认目标
                    if let Some(_socket) = crate::net::udp::udp_socket_get(udp_fd) {
                        let ret = crate::net::udp::udp_send(udp_fd, buf);
                        if ret >= 0 {
                            Ok(ret as usize)
                        } else {
                            Err(ret as i32)
                        }
                    } else {
                        Err(-9)
                    }
                }
            }
        }
    }

    /// 接收数据
    pub fn recv(&self, buf: &mut [u8]) -> Result<(usize, Option<(u32, u16)>), i32> {
        match self.sock_type {
            SocketType::Tcp => {
                let state = *self.state.lock();
                if state != SocketState::Connected {
                    return Err(-107); // ENOTCONN
                }
                let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;

                // 先检查接收队列
                let mut queue = self.recv_queue.lock();
                if let Some(packet) = queue.pop_front() {
                    let len = packet.data.len().min(buf.len());
                    buf[..len].copy_from_slice(&packet.data[..len]);
                    return Ok((len, Some((packet.src_addr, packet.src_port))));
                }

                // 尝试从 TCP socket 接收
                if let Some(socket) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                    match socket.recv(buf, buf.len()) {
                        Ok(len) if len > 0 => {
                            return Ok((len, Some((socket.remote_ip, socket.remote_port))));
                        }
                        _ => {}
                    }
                }

                // 没有数据可读
                Err(-11) // EAGAIN
            }
            SocketType::Udp => {
                // 检查接收队列
                let mut queue = self.recv_queue.lock();
                if let Some(packet) = queue.pop_front() {
                    let len = packet.data.len().min(buf.len());
                    buf[..len].copy_from_slice(&packet.data[..len]);
                    return Ok((len, Some((packet.src_addr, packet.src_port))));
                }

                // 尝试从 UDP socket 接收
                let udp_fd = unsafe { *self.udp_fd.get() }.ok_or(-9)?;
                let len = crate::net::udp::udp_recv(udp_fd, buf, buf.len());
                if len > 0 {
                    Ok((len as usize, None))
                } else {
                    Err(-11) // EAGAIN
                }
            }
        }
    }

    /// 接受连接（仅 TCP）
    pub fn accept(&self) -> Result<Arc<Socket>, i32> {
        if self.sock_type != SocketType::Tcp {
            return Err(-95); // EOPNOTSUPP
        }

        let state = *self.state.lock();
        if state != SocketState::Listening {
            return Err(-22); // EINVAL
        }

        let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;

        // 获取 TCP 连接管理器
        let manager = crate::net::tcp::get_tcp_manager();

        // 检查是否有待处理的连接
        // TODO: 从 pending_connections 获取已建立的连接

        // 简化实现：暂时返回错误
        Err(-11) // EAGAIN
    }

    /// 将数据包放入接收队列
    pub fn enqueue_packet(&self, packet: RecvPacket) {
        self.recv_queue.lock().push_back(packet);
    }

    /// 关闭 socket
    pub fn close(&self) -> i32 {
        match self.sock_type {
            SocketType::Tcp => {
                if let Some(tcp_fd) = unsafe { *self.tcp_fd.get() } {
                    if let Some(socket) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                        socket.close();
                    }
                    crate::net::tcp::tcp_socket_free(tcp_fd);
                }
            }
            SocketType::Udp => {
                if let Some(udp_fd) = unsafe { *self.udp_fd.get() } {
                    crate::net::udp::udp_socket_free(udp_fd);
                }
            }
        }
        0
    }
}

// ============================================================================
// Socket 文件操作
// ============================================================================

fn socket_read(file: &File, buf: &mut [u8]) -> isize {
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -9, // EBADF
    };
    let socket = unsafe { &*(ptr as *const Socket) };

    match socket.recv(buf) {
        Ok((len, _)) => len as isize,
        Err(e) => e as isize,
    }
}

fn socket_write(file: &File, buf: &[u8]) -> isize {
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -9, // EBADF
    };
    let socket = unsafe { &*(ptr as *const Socket) };

    match socket.send(buf, None) {
        Ok(len) => len as isize,
        Err(e) => e as isize,
    }
}

fn socket_close(file: &File) -> i32 {
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    let socket = unsafe { &*(ptr as *const Socket) };

    socket.close();
    0
}

/// Socket 文件操作
pub static SOCKET_OPS: FileOps = FileOps {
    read: Some(socket_read),
    write: Some(socket_write),
    lseek: None, // Socket 不支持 lseek
    close: Some(socket_close),
};

// ============================================================================
// Socket 创建和管理
// ============================================================================

/// 全局 Socket 表
struct SocketTable {
    sockets: alloc::vec::Vec<Option<Arc<Socket>>>,
}

impl SocketTable {
    const fn new() -> Self {
        Self {
            sockets: alloc::vec::Vec::new(),
        }
    }

    fn alloc(&mut self, socket: Arc<Socket>) -> Result<usize, ()> {
        // 查找空闲槽位
        for (i, slot) in self.sockets.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(socket);
                return Ok(i);
            }
        }

        // 没有空闲槽位，添加新的
        let fd = self.sockets.len();
        self.sockets.push(Some(socket));
        Ok(fd)
    }

    fn get(&self, fd: usize) -> Option<Arc<Socket>> {
        self.sockets.get(fd)?.clone()
    }

    fn free(&mut self, fd: usize) {
        if fd < self.sockets.len() {
            self.sockets[fd] = None;
        }
    }
}

static mut SOCKET_TABLE: Mutex<SocketTable> = Mutex::new(SocketTable::new());

/// 创建 socket 并返回文件描述符
pub fn sys_socket_create(domain: i32, type_: i32, protocol: i32) -> Result<usize, i32> {
    // 只支持 AF_INET
    if domain != AF_INET {
        return Err(-97); // EAFNOSUPPORT
    }

    let sock_type = match type_ {
        SOCK_STREAM => {
            if protocol != 0 && protocol != IPPROTO_TCP {
                return Err(-22); // EINVAL
            }
            SocketType::Tcp
        }
        SOCK_DGRAM => {
            if protocol != 0 && protocol != IPPROTO_UDP {
                return Err(-22); // EINVAL
            }
            SocketType::Udp
        }
        _ => return Err(-94), // ESOCKTNOSUPPORT
    };

    // 创建底层协议 socket
    let proto_fd = match sock_type {
        SocketType::Tcp => crate::net::tcp::tcp_socket_alloc()?,
        SocketType::Udp => crate::net::udp::udp_socket_alloc()?,
    };

    // 创建统一的 Socket 结构
    let socket = Arc::new(Socket::new(sock_type));
    match sock_type {
        SocketType::Tcp => unsafe { *socket.tcp_fd.get() = Some(proto_fd); },
        SocketType::Udp => unsafe { *socket.udp_fd.get() = Some(proto_fd); },
    }

    // 创建 File 对象
    let file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDWR)));
    file.set_ops(&SOCKET_OPS);
    file.set_private_data(Arc::as_ptr(&socket) as *mut u8);

    // 安装到文件描述符表
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => return Err(-9), // EBADF
    };

    let fd = fdtable.alloc_fd().ok_or(-24)?; // EMFILE
    fdtable.install_fd(fd, file).map_err(|_| -24)?;

    // 同时保存到全局 socket 表
    unsafe {
        SOCKET_TABLE.lock().alloc(socket);
    }

    Ok(fd)
}

/// 从文件描述符获取 Socket
pub fn get_socket(fd: usize) -> Option<Arc<Socket>> {
    unsafe { SOCKET_TABLE.lock().get(fd) }
}

/// 从文件描述符获取 Socket（通过 File private_data）
pub fn get_socket_from_fd(fd: usize) -> Option<Arc<Socket>> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;

    let ptr = unsafe { *file.private_data.get() }?;
    let socket_ptr = ptr as *const Socket;

    // 需要增加引用计数
    // 简化实现：直接从全局表获取
    unsafe { SOCKET_TABLE.lock().get(fd) }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sockaddr_in() {
        let addr = SockAddrIn {
            sin_family: 2,
            sin_port: 8080u16.to_be(),
            sin_addr: 0x7F000001u32.to_be(),
            sin_zero: [0; 8],
        };

        assert_eq!(addr.port(), 8080);
        assert_eq!(addr.addr(), 0x7F000001);
    }

    #[test]
    fn test_socket_type() {
        let tcp_socket = Socket::new(SocketType::Tcp);
        assert_eq!(tcp_socket.sock_type, SocketType::Tcp);
        assert_eq!(*tcp_socket.state.lock(), SocketState::Unconnected);

        let udp_socket = Socket::new(SocketType::Udp);
        assert_eq!(udp_socket.sock_type, SocketType::Udp);
    }
}
