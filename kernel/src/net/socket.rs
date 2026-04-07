//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Socket Abstraction Layer

use alloc::sync::Arc;
use alloc::collections::VecDeque;
use crate::sync::spinlock::Spinlock;
use core::cell::UnsafeCell;

use crate::fs::file::{File, FileFlags, FileOps, FdTable};

// ============================================================================
// Socket Type Definitions
// ============================================================================

/// Address family
pub const AF_INET: i32 = 2;

/// Socket types
pub const SOCK_STREAM: i32 = 1;  // TCP
pub const SOCK_DGRAM: i32 = 2;   // UDP

/// Protocols
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

// ============================================================================
// Socket Structures
// ============================================================================

/// Socket type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Tcp,
    Udp,
}

/// Socket address
#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

impl SockAddrIn {
    /// Parse from raw bytes
    pub fn from_bytes(data: &[u8]) -> Option<&Self> {
        if data.len() < 16 {
            return None;
        }
        // SAFETY: data is at least 16 bytes (size_of::<SockAddrIn>), and the
        // pointer is valid for that duration since it comes from a slice reference.
        unsafe {
            Some(&*(data.as_ptr() as *const SockAddrIn))
        }
    }

    /// Get port number (host byte order)
    pub fn port(&self) -> u16 {
        u16::from_be(self.sin_port)
    }

    /// Get IP address (host byte order)
    pub fn addr(&self) -> u32 {
        u32::from_be(self.sin_addr)
    }
}

/// Receive buffer packet
#[derive(Clone)]
pub struct RecvPacket {
    /// Data
    pub data: alloc::vec::Vec<u8>,
    /// Source address
    pub src_addr: u32,
    /// Source port
    pub src_port: u16,
}

/// Socket states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// Not connected
    Unconnected,
    /// Connecting
    Connecting,
    /// Connected
    Connected,
    /// Listening
    Listening,
    /// Closing
    Closing,
}

/// Unified Socket structure
pub struct Socket {
    /// Socket type
    pub sock_type: SocketType,
    /// Socket state
    pub state: Spinlock<SocketState>,
    /// Local port
    pub local_port: Spinlock<u16>,
    /// Local IP
    pub local_addr: Spinlock<u32>,
    /// Remote port
    pub remote_port: Spinlock<u16>,
    /// Remote IP
    pub remote_addr: Spinlock<u32>,
    /// Receive buffer
    pub recv_queue: Spinlock<VecDeque<RecvPacket>>,
    /// Whether bound
    pub bound: Spinlock<bool>,
    /// TCP index (for TCP socket table lookup)
    pub tcp_fd: UnsafeCell<Option<i32>>,
    /// UDP index (for UDP socket table lookup)
    pub udp_fd: UnsafeCell<Option<i32>>,
}

// SAFETY: Socket uses Spinlocks for all mutable shared state; UnsafeCell fields
// (tcp_fd, udp_fd) are only accessed from methods that hold appropriate locks
// or are called from a single thread (before the socket is shared).
unsafe impl Sync for Socket {}

impl Socket {
    /// Create a new Socket
    pub fn new(sock_type: SocketType) -> Self {
        Self {
            sock_type,
            state: Spinlock::new(SocketState::Unconnected),
            local_port: Spinlock::new(0),
            local_addr: Spinlock::new(0),
            remote_port: Spinlock::new(0),
            remote_addr: Spinlock::new(0),
            recv_queue: Spinlock::new(VecDeque::new()),
            bound: Spinlock::new(false),
            tcp_fd: UnsafeCell::new(None),
            udp_fd: UnsafeCell::new(None),
        }
    }

    /// Bind to address
    pub fn bind(&self, addr: u32, port: u16) -> Result<(), i32> {
        *self.local_addr.lock() = addr;
        *self.local_port.lock() = port;
        *self.bound.lock() = true;

        match self.sock_type {
            SocketType::Tcp => {
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;
                crate::net::tcp::tcp_bind(tcp_fd, port);
                Ok(())
            }
            SocketType::Udp => {
                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let udp_fd = unsafe { *self.udp_fd.get() }.ok_or(-9)?;
                crate::net::udp::udp_bind(udp_fd, port);
                Ok(())
            }
        }
    }

    /// Listen for connections
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

    /// Connect to remote address
    pub fn connect(&self, addr: u32, port: u16) -> Result<(), i32> {
        *self.remote_addr.lock() = addr;
        *self.remote_port.lock() = port;

        match self.sock_type {
            SocketType::Tcp => {
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;
                *self.state.lock() = SocketState::Connecting;
                let ret = crate::net::tcp::tcp_connect(tcp_fd, addr, port);
                if ret == 0 {
                    *self.state.lock() = SocketState::Connected;
                    Ok(())
                } else {
                    *self.state.lock() = SocketState::Unconnected;
                    Err(ret)
                }
            }
            SocketType::Udp => {
                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let udp_fd = unsafe { *self.udp_fd.get() }.ok_or(-9)?;
                if let Some(socket) = crate::net::udp::udp_socket_get(udp_fd) {
                    let _ = socket.connect(addr, port);
                }
                *self.state.lock() = SocketState::Connected;
                Ok(())
            }
        }
    }

    /// Send data
    pub fn send(&self, buf: &[u8], dest_addr: Option<(u32, u16)>) -> Result<usize, i32> {
        match self.sock_type {
            SocketType::Tcp => {
                let state = *self.state.lock();
                if state != SocketState::Connected {
                    return Err(-32); // EPIPE
                }
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
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
                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let udp_fd = unsafe { *self.udp_fd.get() }.ok_or(-9)?;

                if let Some((_addr, _port)) = dest_addr {
                    Ok(buf.len())
                } else {
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

    /// Receive data
    pub fn recv(&self, buf: &mut [u8]) -> Result<(usize, Option<(u32, u16)>), i32> {
        match self.sock_type {
            SocketType::Tcp => {
                let state = *self.state.lock();
                if state != SocketState::Connected {
                    return Err(-107); // ENOTCONN
                }
                // SAFETY: tcp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
                let tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;

                let mut queue = self.recv_queue.lock();
                if let Some(packet) = queue.pop_front() {
                    let len = packet.data.len().min(buf.len());
                    buf[..len].copy_from_slice(&packet.data[..len]);
                    return Ok((len, Some((packet.src_addr, packet.src_port))));
                }

                if let Some(socket) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                    match socket.recv(buf, buf.len()) {
                        Ok(len) if len > 0 => {
                            return Ok((len, Some((socket.remote_ip, socket.remote_port))));
                        }
                        _ => {}
                    }
                }

                Err(-11) // EAGAIN
            }
            SocketType::Udp => {
                let mut queue = self.recv_queue.lock();
                if let Some(packet) = queue.pop_front() {
                    let len = packet.data.len().min(buf.len());
                    buf[..len].copy_from_slice(&packet.data[..len]);
                    return Ok((len, Some((packet.src_addr, packet.src_port))));
                }

                // SAFETY: udp_fd is only written once during socket creation and
                // read only from this single-threaded socket context.
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

    /// Accept connection (TCP only)
    pub fn accept(&self) -> Result<Arc<Socket>, i32> {
        if self.sock_type != SocketType::Tcp {
            return Err(-95); // EOPNOTSUPP
        }

        let state = *self.state.lock();
        if state != SocketState::Listening {
            return Err(-22); // EINVAL
        }

        let _tcp_fd = unsafe { *self.tcp_fd.get() }.ok_or(-9)?;

        Err(-11) // EAGAIN
    }

    /// Enqueue packet to receive buffer
    pub fn enqueue_packet(&self, packet: RecvPacket) {
        self.recv_queue.lock().push_back(packet);
    }

    /// Close socket
    pub fn close(&self) -> i32 {
        match self.sock_type {
            SocketType::Tcp => {
                if let Some(tcp_fd) = unsafe { *self.tcp_fd.get() } {
                    if let Some(socket) = crate::net::tcp::tcp_socket_get(tcp_fd) {
                        socket.close();
                        // Only free immediately if connection is fully closed.
                        // Otherwise, let the timer tick clean up after TIME_WAIT expires.
                        if socket.state == crate::net::tcp::TcpState::TCP_CLOSE {
                            crate::net::tcp::tcp_socket_free(tcp_fd);
                        }
                    } else {
                        crate::net::tcp::tcp_socket_free(tcp_fd);
                    }
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
// Socket File Operations
// ============================================================================

fn socket_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data was set during socket creation to a valid Arc<Socket> pointer.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -9, // EBADF
    };
    // SAFETY: ptr is a valid Arc<Socket> pointer set during file creation.
    let socket = unsafe { &*(ptr as *const Socket) };

    match socket.recv(buf) {
        Ok((len, _)) => len as isize,
        Err(e) => e as isize,
    }
}

fn socket_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: private_data was set during socket creation to a valid Arc<Socket> pointer.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -9, // EBADF
    };
    // SAFETY: ptr is a valid Arc<Socket> pointer set during file creation.
    let socket = unsafe { &*(ptr as *const Socket) };

    match socket.send(buf, None) {
        Ok(len) => len as isize,
        Err(e) => e as isize,
    }
}

fn socket_close(file: &File) -> i32 {
    // SAFETY: private_data was set during socket creation.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    // SAFETY: ptr is a valid Arc<Socket> pointer.
    let socket = unsafe { &*(ptr as *const Socket) };

    socket.close();
    0
}

fn socket_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    // SAFETY: private_data was set during socket creation.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr is a valid Arc<Socket> pointer.
    let socket = unsafe { &*(ptr as *const Socket) };

    if events & POLLIN != 0 {
        if !socket.recv_queue.lock().is_empty() {
            ready |= POLLIN | POLLRDNORM;
        }
    }

    if events & POLLOUT != 0 {
        let state = *socket.state.lock();
        if state == SocketState::Connected {
            ready |= POLLOUT | POLLWRNORM;
        }
    }

    ready
}

/// Socket file operations
pub static SOCKET_OPS: FileOps = FileOps {
    read: Some(socket_read),
    write: Some(socket_write),
    lseek: None,
    close: Some(socket_close),
    poll: Some(socket_file_poll),
};

// ============================================================================
// Socket Creation and Management
// ============================================================================

/// Global socket table
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
        for (i, slot) in self.sockets.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(socket);
                return Ok(i);
            }
        }

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

static mut SOCKET_TABLE: Spinlock<SocketTable> = Spinlock::new(SocketTable::new());

/// Create socket and return file descriptor
pub fn sys_socket_create(domain: i32, type_: i32, protocol: i32) -> Result<usize, i32> {
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

    let proto_fd = match sock_type {
        SocketType::Tcp => crate::net::tcp::tcp_socket_alloc()?,
        SocketType::Udp => crate::net::udp::udp_socket_alloc()?,
    };

    let socket = Arc::new(Socket::new(sock_type));
    match sock_type {
        // SAFETY: Socket was just created and not yet shared; exclusive access.
        SocketType::Tcp => unsafe { *socket.tcp_fd.get() = Some(proto_fd); },
        // SAFETY: Socket was just created and not yet shared; exclusive access.
        SocketType::Udp => unsafe { *socket.udp_fd.get() = Some(proto_fd); },
    }

    let file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDWR)));
    file.set_ops(&SOCKET_OPS);
    file.set_private_data(Arc::as_ptr(&socket) as *mut u8);

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => return Err(-9), // EBADF
    };

    let fd = fdtable.alloc_fd().ok_or(-24)?; // EMFILE
    fdtable.install_fd(fd, file).map_err(|_| -24)?;

    // SAFETY: SOCKET_TABLE is a global protected by Spinlock; we hold the lock.
    unsafe {
        SOCKET_TABLE.lock().alloc(socket);
    }

    Ok(fd)
}

/// Get socket from file descriptor
pub fn get_socket(fd: usize) -> Option<Arc<Socket>> {
    // SAFETY: SOCKET_TABLE is a global protected by Spinlock; we hold the lock.
    unsafe { SOCKET_TABLE.lock().get(fd) }
}

/// Get socket from file descriptor (via File private_data)
pub fn get_socket_from_fd(fd: usize) -> Option<Arc<Socket>> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;

    // SAFETY: private_data was set during socket creation.
    let ptr = unsafe { *file.private_data.get() }?;
    let _socket_ptr = ptr as *const Socket;

    // SAFETY: SOCKET_TABLE is a global protected by Spinlock; we hold the lock.
    unsafe { SOCKET_TABLE.lock().get(fd) }
}

// ============================================================================
// Tests
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
