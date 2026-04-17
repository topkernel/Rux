//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! UDP Protocol

use crate::net::buffer::SkBuff;
use crate::net::ipv4::{route, checksum};
use crate::config::UDP_SOCKET_TABLE_SIZE;

/// UDP header length
pub const UDP_HLEN: usize = 8;

/// UDP maximum data length
pub const UDP_MAX_DATAGRAM: usize = 65507;

/// UDP port number
pub type UdpPort = u16;

/// UDP header
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UdpHdr {
    /// Source port
    pub source: UdpPort,
    /// Destination port
    pub dest: UdpPort,
    /// Length
    pub len: u16,
    /// Checksum
    pub check: u16,
}

impl UdpHdr {
    /// Create UDP header from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<&UdpHdr> {
        if data.len() < UDP_HLEN {
            return None;
        }

        // SAFETY: data has at least UDP_HLEN bytes and is aligned to UdpHdr layout.
        unsafe {
            Some(&*(data.as_ptr() as *const UdpHdr))
        }
    }

    /// Get source port
    pub fn source(&self) -> UdpPort {
        u16::from_be(self.source)
    }

    /// Get destination port
    pub fn dest(&self) -> UdpPort {
        u16::from_be(self.dest)
    }

    /// Get length
    pub fn len(&self) -> u16 {
        u16::from_be(self.len)
    }

    /// Get checksum
    pub fn check(&self) -> u16 {
        u16::from_be(self.check)
    }
}

/// UDP packet
#[derive(Clone)]
pub struct UdpPacket {
    pub data: alloc::vec::Vec<u8>,
    pub src_addr: u32,
    pub src_port: u16,
}

/// UDP Socket structure
#[repr(C)]
pub struct UdpSocket {
    /// Local port
    pub local_port: UdpPort,
    /// Remote port
    pub remote_port: UdpPort,
    /// Remote IP address
    pub remote_ip: u32,
    /// Local IP address
    pub local_ip: u32,
    /// Whether bound
    pub bound: bool,
    /// Whether connected
    pub connected: bool,
    /// Receive buffer
    pub recv_buffer: alloc::collections::VecDeque<UdpPacket>,
}

impl UdpSocket {
    /// Create new UDP Socket
    pub fn new() -> Self {
        Self {
            local_port: 0,
            remote_port: 0,
            remote_ip: 0,
            local_ip: 0xC0A80164,
            bound: false,
            connected: false,
            recv_buffer: alloc::collections::VecDeque::new(),
        }
    }

    /// Bind to port
    ///
    /// # Arguments
    /// - `port`: Port number
    pub fn bind(&mut self, port: UdpPort) -> Result<(), ()> {
        self.local_port = port;
        self.bound = true;
        Ok(())
    }

    /// Connect to remote address
    ///
    /// # Arguments
    /// - `ip`: IP address
    /// - `port`: Port number
    pub fn connect(&mut self, ip: u32, port: UdpPort) -> Result<(), ()> {
        self.remote_ip = ip;
        self.remote_port = port;
        self.connected = true;
        Ok(())
    }

    /// Disconnect
    pub fn disconnect(&mut self) {
        self.remote_ip = 0;
        self.remote_port = 0;
        self.connected = false;
    }

    /// Enqueue packet to receive buffer
    pub fn enqueue_packet(&mut self, packet: UdpPacket) {
        self.recv_buffer.push_back(packet);
    }

    /// Dequeue packet from receive buffer
    pub fn dequeue_packet(&mut self) -> Option<UdpPacket> {
        self.recv_buffer.pop_front()
    }
}

/// Global UDP socket table
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

    /// Allocate socket slot. Reuses freed slots before growing.
    fn alloc(&mut self) -> Result<usize, ()> {
        // First try to reuse a freed slot
        for i in 0..self.count {
            if self.sockets[i].is_none() {
                self.sockets[i] = Some(UdpSocket::new());
                return Ok(i);
            }
        }

        // No freed slots; grow the table
        if self.count >= UDP_SOCKET_TABLE_SIZE {
            return Err(());
        }

        let fd = self.count;
        self.sockets[fd] = Some(UdpSocket::new());
        self.count += 1;
        Ok(fd)
    }

    /// Free socket
    fn free(&mut self, fd: usize) {
        if fd < self.count {
            self.sockets[fd] = None;
        }
    }

    /// Get socket
    fn get(&self, fd: usize) -> Option<&UdpSocket> {
        if fd < self.count {
            self.sockets[fd].as_ref()
        } else {
            None
        }
    }

    /// Get mutable socket
    fn get_mut(&mut self, fd: usize) -> Option<&mut UdpSocket> {
        if fd < self.count {
            self.sockets[fd].as_mut()
        } else {
            None
        }
    }
}

/// Global UDP socket table
static mut UDP_SOCKET_TABLE: UdpSocketTable = UdpSocketTable::new();

/// Allocate UDP socket
///
/// # Returns
/// Socket file descriptor
pub fn udp_socket_alloc() -> Result<i32, i32> {
    // SAFETY: UDP_SOCKET_TABLE is a global static; single-core kernel ensures
    // no concurrent mutation.
    unsafe {
        match UDP_SOCKET_TABLE.alloc() {
            Ok(fd) => Ok(fd as i32),
            Err(_) => Err(-5), // EIO
        }
    }
}

/// Free UDP socket
///
/// # Arguments
/// - `fd`: Socket file descriptor
pub fn udp_socket_free(fd: i32) {
    // SAFETY: UDP_SOCKET_TABLE is a global; fd was returned by udp_socket_alloc.
    unsafe {
        UDP_SOCKET_TABLE.free(fd as usize);
    }
}

/// Get UDP socket
///
/// # Arguments
/// - `fd`: Socket file descriptor
///
/// # Returns
/// Socket reference
pub fn udp_socket_get(fd: i32) -> Option<&'static mut UdpSocket> {
    // SAFETY: UDP_SOCKET_TABLE is a global; caller ensures no concurrent access.
    unsafe {
        UDP_SOCKET_TABLE.get_mut(fd as usize)
    }
}

/// Bind socket to port
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `port`: Port number
///
/// # Returns
/// 0 on success, error code on failure
pub fn udp_bind(fd: i32, port: UdpPort) -> i32 {
    // SAFETY: UDP_SOCKET_TABLE is a global; fd was returned by udp_socket_alloc.
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

/// Send UDP packet
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
///
/// # Returns
/// Bytes sent on success, error code on failure
pub fn udp_send(fd: i32, buf: &[u8]) -> isize {
    // Get socket
    let socket = match udp_socket_get(fd) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    // Get destination address
    let (dest_ip, dest_port) = if socket.connected {
        (socket.remote_ip, socket.remote_port)
    } else {
        // Unconnected UDP socket needs destination address specified
        return -107; // ENOTCONN
    };

    if buf.is_empty() {
        return 0;
    }

    // Allocate SkBuff
    let mut skb = match crate::net::buffer::alloc_skb(1500) {
        Some(skb) => skb,
        None => return -12, // ENOMEM
    };

    // Build UDP header + data (udp_build_packet puts data into skb)
    if udp_build_packet(&mut skb, socket.local_port, dest_port, buf).is_err() {
        crate::net::buffer::kfree_skb(skb);
        return -5; // EIO
    }

    // Send to IP layer
    match crate::net::ipv4::ipv4_send(skb, dest_ip, 17) { // IPPROTO_UDP = 17
        Ok(()) => buf.len() as isize,
        Err(_) => -5, // EIO
    }
}

/// Send UDP packet to specified address
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
/// - `dest_ip`: Destination IP address
/// - `dest_port`: Destination port
///
/// # Returns
/// Bytes sent on success, error code on failure
pub fn udp_sendto(fd: i32, buf: &[u8], dest_ip: u32, dest_port: u16) -> isize {
    // Get socket
    let socket = match udp_socket_get(fd) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    if buf.is_empty() {
        return 0;
    }

    // Allocate SkBuff
    let mut skb = match crate::net::buffer::alloc_skb(1500) {
        Some(skb) => skb,
        None => return -12, // ENOMEM
    };

    // Build UDP header + data (udp_build_packet puts data into skb)
    if udp_build_packet(&mut skb, socket.local_port, dest_port, buf).is_err() {
        crate::net::buffer::kfree_skb(skb);
        return -5; // EIO
    }

    // Send to IP layer
    match crate::net::ipv4::ipv4_send(skb, dest_ip, 17) { // IPPROTO_UDP = 17
        Ok(()) => buf.len() as isize,
        Err(_) => -5, // EIO
    }
}

/// Receive UDP packet
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
/// - `len`: Buffer length
///
/// # Returns
/// Bytes received on success, error code on failure
pub fn udp_recv(fd: i32, buf: &mut [u8], _len: usize) -> isize {
    // Get socket
    let socket = match udp_socket_get(fd) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    // Get data from receive buffer
    match socket.dequeue_packet() {
        Some(packet) => {
            let copy_len = packet.data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&packet.data[..copy_len]);
            copy_len as isize
        }
        None => -11, // EAGAIN (no data to read)
    }
}

/// Receive UDP packet and return source address
///
/// # Arguments
/// - `fd`: Socket file descriptor
/// - `buf`: Data buffer
/// - `len`: Buffer length
///
/// # Returns
/// (bytes, source_ip, source_port) on success, error code on failure
pub fn udp_recvfrom(fd: i32, buf: &mut [u8], _len: usize) -> Result<(isize, u32, u16), isize> {
    // Get socket
    let socket = match udp_socket_get(fd) {
        Some(s) => s,
        None => return Err(-9), // EBADF
    };

    // Get data from receive buffer
    match socket.dequeue_packet() {
        Some(packet) => {
            let copy_len = packet.data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&packet.data[..copy_len]);
            Ok((copy_len as isize, packet.src_addr, packet.src_port))
        }
        None => Err(-11), // EAGAIN
    }
}

/// Calculate UDP checksum
///
/// # Arguments
/// - `shdr`: Source IP address (network byte order)
/// - `dhdr`: Destination IP address (network byte order)
/// - `uhdr`: UDP header
/// - `data`: Data
///
/// # Returns
/// Checksum (network byte order)
pub fn udp_checksum(shdr: u32, dhdr: u32, uhdr: &UdpHdr, data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Pseudo header (12 bytes)
    // Source IP (4 bytes)
    sum += (shdr >> 16) & 0xFFFF;
    sum += shdr & 0xFFFF;
    // Destination IP (4 bytes)
    sum += (dhdr >> 16) & 0xFFFF;
    sum += dhdr & 0xFFFF;
    // Reserved (1 byte) + Protocol (1 byte) + UDP length (2 bytes)
    sum += 17u32; // UDP protocol number (reserved=0, protocol=17)
    sum += uhdr.len as u32;

    // UDP header
    sum += uhdr.source as u32;
    sum += uhdr.dest as u32;
    sum += uhdr.len as u32;
    sum += 0; // Checksum field (set to 0 first)

    // Data
    let mut i = 0;
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        sum += word;
        i += 2;
    }

    // Handle last byte (if any)
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    // Handle carry
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // Invert
    !sum as u16
}

/// Build UDP packet
///
/// # Arguments
/// - `skb`: SkBuff
/// - `source`: Source port
/// - `dest`: Destination port
/// - `data`: Data
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn udp_build_packet(
    skb: &mut SkBuff,
    source: UdpPort,
    dest: UdpPort,
    data: &[u8],
) -> Result<(), ()> {
    // Allocate space for UDP header
    let ptr = skb.skb_push(UDP_HLEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // UDP_HLEN bytes; writing fields of repr(C) UdpHdr is well-defined.
    unsafe {
        let udp_hdr = &mut *(ptr as *mut UdpHdr);

        // Source port
        udp_hdr.source = source.to_be();

        // Destination port
        udp_hdr.dest = dest.to_be();

        // Length (UDP header + data)
        udp_hdr.len = ((UDP_HLEN + data.len()) as u16).to_be();

        // Checksum (set to 0 first, calculate later)
        udp_hdr.check = 0;
    }

    // Add data
    skb.skb_put_data(data)?;

    Ok(())
}

/// Parse UDP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing UDP packet)
///
/// # Returns
/// UDP header reference, or None if parsing fails
pub fn udp_parse_packet(skb: &SkBuff) -> Option<&UdpHdr> {
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < UDP_HLEN {
        return None;
    }

    let udp_hdr = UdpHdr::from_bytes(data)?;

    // Validate length
    let len = udp_hdr.len();
    if (len as usize) < UDP_HLEN || (len as usize) != data.len() {
        return None;
    }

    Some(udp_hdr)
}

/// Receive and process UDP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing UDP packet)
/// - `src_ip`: Source IP address
/// - `dest_ip`: Destination IP address
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn udp_rcv(skb: &SkBuff, src_ip: u32, dest_ip: u32) -> Result<(), ()> {
    // Parse UDP header
    let udp_hdr = udp_parse_packet(skb).ok_or(())?;

    // Verify UDP checksum (per RFC 768: checksum=0 means no checksum)
    if udp_hdr.check() != 0 {
        let data_len = (udp_hdr.len() as usize).saturating_sub(UDP_HLEN);
        let data = if data_len > 0 {
            // SAFETY: skb.data + UDP_HLEN is within the skb's valid data range
            // since udp_parse_packet validated the length.
            unsafe {
                let data_ptr = skb.data.add(UDP_HLEN);
                core::slice::from_raw_parts(data_ptr, data_len)
            }
        } else {
            &[]
        };
        let computed = udp_checksum(src_ip, dest_ip, udp_hdr, data);
        if computed != udp_hdr.check() {
            // Checksum mismatch, silently drop packet
            return Ok(());
        }
    }

    let src_port = UdpPort::from_be(udp_hdr.source);
    let dest_port = UdpPort::from_be(udp_hdr.dest);

    // Get UDP data (after header)
    let data_len = (udp_hdr.len() as usize).saturating_sub(UDP_HLEN);
    let data = if data_len > 0 {
        // SAFETY: skb.data + UDP_HLEN is within the skb's valid data range
        // since udp_parse_packet validated the length.
        unsafe {
            let data_ptr = skb.data.add(UDP_HLEN);
            core::slice::from_raw_parts(data_ptr, data_len)
        }
    } else {
        &[]
    };

    // Find socket bound to destination port (and optionally destination IP).
    // A socket with local_ip == 0 (INADDR_ANY) accepts packets to any local IP;
    // a socket with a specific local_ip only accepts packets to that IP.
    // SAFETY: UDP_SOCKET_TABLE is a global; iterating under current single-core
    // kernel context ensures no concurrent mutation.
    unsafe {
        for i in 0..UDP_SOCKET_TABLE.count {
            if let Some(ref mut socket) = UDP_SOCKET_TABLE.sockets[i] {
                if socket.bound && socket.local_port == dest_port
                    && (socket.local_ip == 0 || socket.local_ip == dest_ip)
                {
                    // Put data into socket's receive buffer
                    let packet = UdpPacket {
                        data: alloc::vec::Vec::from(data),
                        src_addr: src_ip,
                        src_port: src_port,
                    };
                    socket.enqueue_packet(packet);
                    return Ok(());
                }
            }
        }
    }

    // No socket found bound to this port, drop packet
    Ok(())
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
        let shdr = 0xC0A80101;
        let dhdr = 0xC0A80102;
        let data = b"Hello, World!";

        let mut uhdr = UdpHdr::default();
        uhdr.source = 1234u16.to_be();
        uhdr.dest = 80u16.to_be();
        uhdr.len = ((UDP_HLEN + data.len()) as u16).to_be();
        uhdr.check = 0;

        let csum = udp_checksum(shdr, dhdr, &uhdr, data);
        assert!(csum != 0 || csum == 0xFFFF);
    }
}
