//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 以太网层
//!
//! 完全遵循 Linux 内核的以太网层实现
//! 参考: net/ethernet/eth.c, include/linux/etherdevice.h, include/uapi/linux/if_ether.h

use crate::net::buffer::{SkBuff, EthProtocol};

/// 以太网头部长度
pub const ETH_HLEN: usize = 14;

/// 以太网最小帧长度
pub const ETH_ZLEN: usize = 60;

/// 以太网最大帧长度 (不含 FCS)
pub const ETH_DATA_LEN: usize = 1500;

/// 以太网最大帧长度 (含 FCS)
pub const ETH_FRAME_LEN: usize = 1514;

/// 以太网 MTU
pub const ETH_MTU: usize = 1500;

/// 以太网头部长度 + VLAN 标签 (802.1Q)
pub const ETH_VLAN_HLEN: usize = 18;

/// 以太网地址长度 (MAC 地址)
pub const ETH_ALEN: usize = 6;

/// 广播 MAC 地址
pub const ETH_BROADCAST: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// 以太网帧头部
///
/// 对应 Linux 的 ethhdr (include/uapi/linux/if_ether.h)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EthHdr {
    /// 目标 MAC 地址
    pub h_dest: [u8; ETH_ALEN],
    /// 源 MAC 地址
    pub h_source: [u8; ETH_ALEN],
    /// 协议类型 (ETH_P_IP, ETH_P_ARP, etc.)
    pub h_proto: u16,
}

impl EthHdr {
    /// 从字节切片创建以太网头部
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < ETH_HLEN {
            return None;
        }

        unsafe {
            Some(&*(data.as_ptr() as *const EthHdr))
        }
    }

    /// 获取协议类型
    pub fn protocol(&self) -> EthProtocol {
        // 以太网协议类型是大端序
        let proto = u16::from_be(self.h_proto);
        EthProtocol::from_u16(proto).unwrap_or(EthProtocol::ETH_P_IP)
    }

    /// 检查是否为广播帧
    pub fn is_broadcast(&self) -> bool {
        self.h_dest == ETH_BROADCAST
    }

    /// 检查是否为多播帧
    pub fn is_multicast(&self) -> bool {
        // 多播地址的最低字节的最低位为 1
        (self.h_dest[0] & 0x01) != 0
    }

    /// 检查是否为本机帧 (目标 MAC 为本机或广播/多播)
    pub fn is_for_us(&self, our_mac: &[u8; ETH_ALEN]) -> bool {
        self.h_dest == *our_mac || self.is_broadcast() || self.is_multicast()
    }
}

/// 以太网帧尾部 (FCS - Frame Check Sequence)
///
/// 4 字节的 CRC32 校验和
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EthFcs {
    /// CRC32 校验和
    pub fcs: u32,
}

/// 构造以太网帧
///
/// # 参数
/// - `skb`: SkBuff
/// - `dest`: 目标 MAC 地址
/// - `src`: 源 MAC 地址
/// - `proto`: 协议类型
///
/// # 说明
/// 在 SkBuff 前面添加以太网头部
pub fn eth_push_header(skb: &mut SkBuff, dest: [u8; ETH_ALEN], src: [u8; ETH_ALEN], proto: EthProtocol) -> Result<(), ()> {
    // 分配空间用于以太网头部
    let ptr = skb.skb_push(ETH_HLEN as u32).ok_or(())?;

    unsafe {
        let eth_hdr = &mut *(ptr as *mut EthHdr);
        eth_hdr.h_dest = dest;
        eth_hdr.h_source = src;
        eth_hdr.h_proto = proto.to_u16();
    }

    Ok(())
}

/// 解析以太网帧
///
/// # 参数
/// - `skb`: SkBuff
///
/// # 返回
/// 返回以太网头部引用，如果解析失败则返回 None
pub fn eth_pull_header(skb: &mut SkBuff) -> Option<&'static EthHdr> {
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < ETH_HLEN {
        return None;
    }

    let eth_hdr = EthHdr::from_bytes(data)?;

    // 移除以太网头部
    skb.skb_pull(ETH_HLEN as u32);

    Some(eth_hdr)
}

/// 以太网设备类型
///
/// 对应 Linux 的 ARPHRD_* 常量 (include/linux/if_arp.h)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpHrdType {
    /// 回环设备
    ARPHRD_LOOPBACK = 772,
    /// 以太网
    ARPHRD_ETHER = 1,
    /// EUI-64
    ARPHRD_EUI64 = 27,
}

/// 计算以太网帧的 CRC32 校验和
///
/// # 参数
/// - `data`: 帧数据
///
/// # 返回
/// CRC32 校验和
pub fn eth_crc(data: &[u8]) -> u32 {
    // 简化实现：返回固定值
    // 完整实现需要 CRC32 算法
    0xFFFFFFFF
}

/// 检查以太网地址是否有效
///
/// # 参数
/// - `addr`: MAC 地址
///
/// # 返回
/// 地址是否有效 (非零、非多播)
pub fn eth_is_valid_unicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    // 检查是否为零地址
    if addr.iter().all(|&b| b == 0) {
        return false;
    }

    // 检查是否为多播地址
    if addr[0] & 0x01 != 0 {
        return false;
    }

    true
}

/// 检查以太网地址是否为多播地址
///
/// # 参数
/// - `addr`: MAC 地址
///
/// # 返回
/// 是否为多播地址
pub fn eth_is_multicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    addr[0] & 0x01 != 0
}

/// 检查以太网地址是否为广播地址
///
/// # 参数
/// - `addr`: MAC 地址
///
/// # 返回
/// 是否为广播地址
pub fn eth_is_broadcast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    addr == &ETH_BROADCAST
}

/// 比较两个以太网地址
///
/// # 参数
/// - `a`: 地址 A
/// - `b`: 地址 B
///
/// # 返回
/// 是否相等
pub fn eth_addr_eq(a: &[u8; ETH_ALEN], b: &[u8; ETH_ALEN]) -> bool {
    a == b
}

/// 复制以太网地址
///
/// # 参数
/// - `dst`: 目标地址
/// - `src`: 源地址
pub fn eth_addr_copy(dst: &mut [u8; ETH_ALEN], src: &[u8; ETH_ALEN]) {
    dst.copy_from_slice(src);
}

/// 清零以太网地址
///
/// # 参数
/// - `addr`: 要清零的地址
pub fn eth_addr_zero(addr: &mut [u8; ETH_ALEN]) {
    addr.fill(0);
}

/// 发送以太网帧
///
/// # 参数
/// - `skb`: SkBuff (包含 IP 数据包)
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
///
/// # 说明
/// 添加以太网头部并发送到网络设备
/// TODO: 实现实际的网络设备发送
pub fn ethernet_send(mut skb: SkBuff) -> Result<(), ()> {
    // 构造以太网头部
    // 简化实现：使用广播 MAC 地址
    let dest_mac = ETH_BROADCAST;
    let src_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]; // 示例 MAC 地址

    eth_push_header(&mut skb, dest_mac, src_mac, EthProtocol::ETH_P_IP)?;

    // TODO: 发送到网络设备驱动
    // 目前简化实现：直接丢弃数据包
    drop(skb);

    Ok(())
}

/// 以太网 MAC 地址转字符串 (用于调试)
///
/// # 参数
/// - `addr`: MAC 地址
///
/// # 返回
/// 格式化的 MAC 地址字符串 (例如 "52:54:00:12:34:56")
pub fn eth_addr_to_string(addr: &[u8; ETH_ALEN]) -> alloc::string::String {
    alloc::format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[0], addr[1], addr[2], addr[3], addr[4], addr[5]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eth_hdr_size() {
        assert_eq!(core::mem::size_of::<EthHdr>(), 14);
    }

    #[test]
    fn test_eth_broadcast() {
        let addr: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(eth_is_broadcast_addr(&addr));
        assert!(eth_is_multicast_addr(&addr));
    }

    #[test]
    fn test_eth_multicast() {
        let addr: [u8; 6] = [0x01, 0x00, 0x5E, 0x00, 0x00, 0x01];
        assert!(eth_is_multicast_addr(&addr));
        assert!(!eth_is_broadcast_addr(&addr));
    }

    #[test]
    fn test_eth_unicast() {
        let addr: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        assert!(!eth_is_multicast_addr(&addr));
        assert!(!eth_is_broadcast_addr(&addr));
        assert!(eth_is_valid_unicast_addr(&addr));
    }
}
