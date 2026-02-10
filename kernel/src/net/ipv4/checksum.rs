//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IP 校验和计算
//!
//! 完全遵循 RFC 1071 - Computing the Internet Checksum

/// 计算 IP 校验和
///
/// # 参数
/// - `data`: 数据 (必须是偶数长度)
///
/// # 返回
/// 校验和 (网络字节序)
///
/// # 说明
/// RFC 1071 定义的 Internet 校验和算法
pub fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // 按 16 位字累加
    let mut i = 0;
    while i < data.len() {
        // 处理最后一个字节 (如果长度为奇数)
        if i + 1 == data.len() {
            sum += (data[i] as u32) << 8;
        } else {
            let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            sum += word;
        }
        i += 2;
    }

    // 处理进位
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // 取反
    !sum as u16
}

/// 验证 IP 校验和
///
/// # 参数
/// - `data`: 数据
///
/// # 返回
/// 校验和是否有效
pub fn verify_ip_checksum(data: &[u8]) -> bool {
    ip_checksum(data) == 0
}

/// 计算伪头部校验和 (用于 TCP/UDP)
///
/// # 参数
/// - `src_addr`: 源 IP 地址
/// - `dst_addr`: 目标 IP 地址
/// - `protocol`: 协议号
/// - `tcp_udp_len`: TCP/UDP 数据长度
///
/// # 返回
/// 伪头部校验和
pub fn pseudo_header_checksum(
    src_addr: u32,
    dst_addr: u32,
    protocol: u8,
    tcp_udp_len: u16,
) -> u16 {
    let mut pseudo_header = [0u8; 12];

    // 源 IP 地址 (4 字节)
    pseudo_header[0..4].copy_from_slice(&src_addr.to_be_bytes());

    // 目标 IP 地址 (4 字节)
    pseudo_header[4..8].copy_from_slice(&dst_addr.to_be_bytes());

    // 保留 (1 字节) + 协议 (1 字节)
    pseudo_header[8] = 0;
    pseudo_header[9] = protocol;

    // TCP/UDP 长度 (2 字节)
    pseudo_header[10..12].copy_from_slice(&tcp_udp_len.to_be_bytes());

    ip_checksum(&pseudo_header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_checksum() {
        // 测试数据
        let data = [0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8, 0x01, 0x01, 0xc0, 0xa8, 0x01, 0x02];

        let csum = ip_checksum(&data);
        // 校验和应该使得累加结果为 0
        // 这里我们只测试函数能正常工作
        assert_eq!(csum, 0xb1e6);
    }

    #[test]
    fn test_pseudo_header_checksum() {
        let src = 0xC0A80101; // 192.168.1.1
        let dst = 0xC0A80102; // 192.168.1.2
        let protocol = 6; // TCP
        let len = 20; // TCP 头部长度

        let csum = pseudo_header_checksum(src, dst, protocol, len);
        // 验证函数能正常工作
        assert!(csum != 0);
    }
}
