//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IP Checksum Calculation

/// Calculate IP checksum
///
/// # Arguments
/// - `data`: Data (must be even length)
///
/// # Returns
/// Checksum (network byte order)
///
/// # Notes
/// Internet checksum algorithm
pub fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    let mut i = 0;
    while i < data.len() {
        if i + 1 == data.len() {
            sum += (data[i] as u32) << 8;
        } else {
            let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            sum += word;
        }
        i += 2;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

/// Verify IP checksum
///
/// # Arguments
/// - `data`: Data
///
/// # Returns
/// Whether checksum is valid
pub fn verify_ip_checksum(data: &[u8]) -> bool {
    ip_checksum(data) == 0
}

/// Calculate pseudo-header checksum (for TCP/UDP)
///
/// # Arguments
/// - `src_addr`: Source IP address
/// - `dst_addr`: Destination IP address
/// - `protocol`: Protocol number
/// - `tcp_udp_len`: TCP/UDP data length
///
/// # Returns
/// Pseudo-header checksum
pub fn pseudo_header_checksum(
    src_addr: u32,
    dst_addr: u32,
    protocol: u8,
    tcp_udp_len: u16,
) -> u16 {
    let mut pseudo_header = [0u8; 12];

    pseudo_header[0..4].copy_from_slice(&src_addr.to_be_bytes());

    pseudo_header[4..8].copy_from_slice(&dst_addr.to_be_bytes());

    pseudo_header[8] = 0;
    pseudo_header[9] = protocol;

    pseudo_header[10..12].copy_from_slice(&tcp_udp_len.to_be_bytes());

    ip_checksum(&pseudo_header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_checksum() {
        let data = [0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8, 0x01, 0x01, 0xc0, 0xa8, 0x01, 0x02];

        let csum = ip_checksum(&data);
        assert_eq!(csum, 0xb1e6);
    }

    #[test]
    fn test_pseudo_header_checksum() {
        let src = 0xC0A80101;
        let dst = 0xC0A80102;
        let protocol = 6;
        let len = 20;

        let csum = pseudo_header_checksum(src, dst, protocol, len);
        assert!(csum != 0);
    }
}
