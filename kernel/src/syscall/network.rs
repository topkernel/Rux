//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络相关系统调用
//!
//! 包含：socket, bind, listen, accept, connect, sendto, recvfrom

use super::*;

/// sys_socket - 创建 socket
///
/// # 参数
/// - args[0]: domain - 协议族 (AF_INET=2)
/// - args[1]: type - socket 类型 (SOCK_STREAM=1, SOCK_DGRAM=2)
/// - args[2]: protocol - 协议类型 (IPPROTO_TCP=6, IPPROTO_UDP=17)
///
/// # 返回
/// 成功返回文件描述符，失败返回负错误码
pub fn sys_socket(args: SyscallArgs) -> u64 {
    let domain = args[0] as i32;
    let type_ = args[1] as i32;
    let protocol = args[2] as i32;

    // 尝试使用新的 socket 层
    match crate::net::socket::sys_socket_create(domain, type_, protocol) {
        Ok(fd) => return fd as u64,
        Err(e) => {
            // 如果新 socket 层失败，回退到旧实现
            // 但只在特定错误时回退
            if e != -97 && e != -94 && e != -22 {
                // 不是参数错误，可能是 socket 层未初始化
                // 回退到旧的实现
            } else {
                return e as u64;
            }
        }
    }

    // 旧的实现（回退）
    // 目前只支持 AF_INET (IPv4)
    if domain != 2 {
        return -errno::EAFNOSUPPORT as u64;
    }

    match type_ {
        1 => {
            // SOCK_STREAM (TCP)
            if protocol != 0 && protocol != 6 {
                return -errno::EINVAL as u64;
            }

            use crate::net::tcp;
            match tcp::tcp_socket_alloc() {
                Ok(fd) => fd as u64,
                Err(e) => e as u64
            }
        }
        2 => {
            // SOCK_DGRAM (UDP)
            if protocol != 0 && protocol != 17 {
                return -errno::EINVAL as u64;
            }

            use crate::net::udp;
            match udp::udp_socket_alloc() {
                Ok(fd) => fd as u64,
                Err(e) => e as u64
            }
        }
        _ => {
            -errno::ESOCKTNOSUPPORT as u64
        }
    }
}

/// sys_bind - 绑定 socket 到地址
///
/// # 参数
/// - args[0]: fd - socket 文件描述符
/// - args[1]: addr - sockaddr 结构指针
/// - args[2]: addrlen - 地址长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_bind(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    // 检查地址指针有效性
    if addr_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 读取 sockaddr_in 结构（简化实现）
    // struct sockaddr_in {
    //     sa_family_t sin_family;  // 2 bytes
    //     in_port_t sin_port;      // 2 bytes (network byte order)
    //     struct in_addr sin_addr; // 4 bytes
    //     char sin_zero[8];        // 8 bytes
    // };

    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };

    // 目前只支持 AF_INET
    if sin_family != 2 {
        return -errno::EAFNOSUPPORT as u64;
    }

    // TODO: 需要一种方法确定 fd 是 TCP 还是 UDP socket
    // 简化实现：尝试两种协议
    use crate::net::{tcp, udp};

    // 先尝试 TCP
    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        return tcp::tcp_bind(fd, sin_port) as u64;
    }

    // 再尝试 UDP
    if let Some(_socket) = udp::udp_socket_get(fd) {
        return udp::udp_bind(fd, sin_port) as u64;
    }

    -errno::EBADF as u64
}

/// sys_listen - 监听 socket
///
/// # 参数
/// - args[0]: fd - socket 文件描述符
/// - args[1]: backlog - 等待连接队列长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_listen(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let backlog = args[1] as i32;

    use crate::net::tcp;

    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        tcp::tcp_listen(fd, backlog as u32) as u64
    } else {
        -errno::EBADF as u64
    }
}

/// sys_accept - 接受连接
///
/// # 参数
/// - args[0]: fd - socket 文件描述符
/// - args[1]: addr - sockaddr 结构指针（输出）
/// - args[2]: addrlen - 地址长度指针（输入/输出）
///
/// # 返回
/// 成功返回新 socket 的文件描述符，失败返回负错误码
pub fn sys_accept(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let _addr_ptr = args[1] as *mut u8;
    let _addrlen_ptr = args[2] as *mut u32;

    use crate::net::tcp;

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_accept(fd) as u64,
        None => -errno::EBADF as u64
    }
}

/// sys_connect - 连接到远程地址
///
/// # 参数
/// - args[0]: fd - socket 文件描述符
/// - args[1]: addr - sockaddr 结构指针
/// - args[2]: addrlen - 地址长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_connect(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    // 检查地址指针有效性
    if addr_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 读取 sockaddr_in 结构
    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };
    let sin_addr = unsafe { u32::from_be_bytes(*((addr_ptr.add(4)) as *const [u8; 4])) };

    // 目前只支持 AF_INET
    if sin_family != 2 {
        return -errno::EAFNOSUPPORT as u64;
    }

    use crate::net::tcp;

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_connect(fd, sin_addr, sin_port) as u64,
        None => -errno::EBADF as u64
    }
}

/// sys_sendto - 发送数据（可能指定目标地址）
///
/// # 参数
/// - args[0]: fd - socket 文件描述符
/// - args[1]: buf - 数据缓冲区指针
/// - args[2]: len - 数据长度
/// - args[3]: flags - 标志位
/// - args[4]: addr - 目标地址指针（可选）
/// - args[5]: addrlen - 地址长度（可选）
///
/// # 返回
/// 成功返回发送的字节数，失败返回负错误码
pub fn sys_sendto(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *const u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let addr_ptr = args[4] as *const u8;
    let _addrlen = args[5] as u32;

    // 检查缓冲区指针有效性
    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    if len == 0 {
        return 0;
    }

    // 获取 socket
    let socket = match crate::net::socket::get_socket(fd) {
        Some(s) => s,
        None => {
            // 尝试从旧的 socket 表查找
            // 先尝试 TCP
            if let Some(_) = crate::net::tcp::tcp_socket_get(fd as i32) {
                let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
                return data.len() as u64;  // 简化实现
            }
            // 再尝试 UDP
            if let Some(_) = crate::net::udp::udp_socket_get(fd as i32) {
                let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
                return crate::net::udp::udp_send(fd as i32, data) as u64;
            }
            return -errno::EBADF as u64;
        }
    };

    // 读取数据
    let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };

    // 解析目标地址（如果提供）
    let dest_addr = if !addr_ptr.is_null() {
        if let Some(sockaddr) = crate::net::socket::SockAddrIn::from_bytes(unsafe {
            core::slice::from_raw_parts(addr_ptr, 16)
        }) {
            Some((sockaddr.addr(), sockaddr.port()))
        } else {
            None
        }
    } else {
        None
    };

    // 发送数据
    match socket.send(data, dest_addr) {
        Ok(bytes_sent) => bytes_sent as u64,
        Err(e) => e as u64,
    }
}

/// sys_recvfrom - 接收数据（可能获取源地址）
///
/// # 参数
/// - args[0]: fd - socket 文件描述符
/// - args[1]: buf - 数据缓冲区指针
/// - args[2]: len - 缓冲区长度
/// - args[3]: flags - 标志位
/// - args[4]: addr - 源地址指针（可选，输出）
/// - args[5]: addrlen - 地址长度指针（可选，输入/输出）
///
/// # 返回
/// 成功返回接收的字节数，失败返回负错误码
pub fn sys_recvfrom(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *mut u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let addr_ptr = args[4] as *mut u8;
    let addrlen_ptr = args[5] as *mut u32;

    // 检查缓冲区指针有效性
    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    if len == 0 {
        return 0;
    }

    // 获取 socket
    let socket = match crate::net::socket::get_socket(fd) {
        Some(s) => s,
        None => {
            // 尝试从旧的 socket 表查找
            // 先尝试 TCP
            if let Some(tcp_sock) = crate::net::tcp::tcp_socket_get(fd as i32) {
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return match tcp_sock.recv(buf, len) {
                    Ok(n) => n as u64,
                    Err(_) => -errno::EAGAIN as u64,
                };
            }
            // 再尝试 UDP
            if let Some(_) = crate::net::udp::udp_socket_get(fd as i32) {
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return crate::net::udp::udp_recv(fd as i32, buf, len) as u64;
            }
            return -errno::EBADF as u64;
        }
    };

    // 接收数据
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };

    match socket.recv(buf) {
        Ok((bytes_read, src_addr)) => {
            // 如果提供了地址指针，写入源地址
            if let Some((addr, port)) = src_addr {
                if !addr_ptr.is_null() && !addrlen_ptr.is_null() {
                    unsafe {
                        // 写入 sockaddr_in 结构
                        core::ptr::write(addr_ptr as *mut u16, 2);  // sin_family = AF_INET
                        core::ptr::write(addr_ptr.add(2) as *mut u16, port.to_be());
                        core::ptr::write(addr_ptr.add(4) as *mut u32, addr.to_be());
                        // sin_zero 保持为 0
                        core::ptr::write_bytes(addr_ptr.add(8), 0, 8);
                        // 写入地址长度
                        core::ptr::write(addrlen_ptr, 16);
                    }
                }
            }
            bytes_read as u64
        }
        Err(e) => e as u64,
    }
}
