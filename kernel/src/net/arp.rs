//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ARP 协议
//!
//! 完全遵循 Linux 内核的 ARP 实现
//! 参考: net/ipv4/arp.c, include/uapi/linux/if_arp.h

use crate::net::buffer::SkBuff;
use crate::net::ethernet::{ETH_ALEN, eth_is_broadcast_addr};
use crate::config::ARP_CACHE_SIZE;

/// ARP 硬件类型
///
/// 对应 Linux 的 ARPHRD_* (include/linux/if_arp.h)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpHrd {
    /// 以太网
    ARPHRD_ETHER = 1,
    /// 回环设备
    ARPHRD_LOOPBACK = 772,
    /// 无
    ARPHRD_VOID = 0xFFFF,
}

/// ARP 协议类型
///
/// 对应 Linux 的 EtherType (include/linux/if_ether.h)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpPro {
    /// IPv4
    ARPPROTO_IP = 0x0800,
    /// IPv6
    ARPPROTO_IPV6 = 0x86DD,
}

/// ARP 操作类型
///
/// 对应 ARP 操作码
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpOp {
    /// ARP 请求
    ARPOP_REQUEST = 1,
    /// ARP 响应
    ARPOP_REPLY = 2,
    /// RARP 请求
    ARPOP_RREQUEST = 3,
    /// RARP 响应
    ARPOP_RREPLY = 4,
}

/// ARP 报文头部
///
/// 对应 Linux 的 arphdr (include/uapi/linux/if_arp.h)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpHdr {
    /// 硬件类型 (例如 ARPHRD_ETHER = 1)
    pub ar_hrd: u16,
    /// 协议类型 (例如 ETH_P_IP = 0x0800)
    pub ar_pro: u16,
    /// 硬件地址长度 (以太网 = 6)
    pub ar_hln: u8,
    /// 协议地址长度 (IPv4 = 4)
    pub ar_pln: u8,
    /// 操作类型 (ARPOP_REQUEST/ARPOP_REPLY)
    pub ar_op: u16,
}

/// ARP 报文 (以太网 + IPv4)
///
/// 完整的 ARP 报文，包括头部和数据
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    /// ARP 头部
    pub hdr: ArpHdr,
    /// 发送方硬件地址 (MAC)
    pub ar_sha: [u8; ETH_ALEN],
    /// 发送方协议地址 (IP)
    pub ar_sip: u32,
    /// 目标硬件地址 (MAC)
    pub ar_tha: [u8; ETH_ALEN],
    /// 目标协议地址 (IP)
    pub ar_tip: u32,
}

impl ArpPacket {
    /// ARP 报文总长度 (以太网 + IPv4)
    pub const LEN: usize = core::mem::size_of::<ArpPacket>();

    /// 从字节切片创建 ARP 报文
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < Self::LEN {
            return None;
        }

        unsafe {
            Some(&*(data.as_ptr() as *const ArpPacket))
        }
    }

    /// 检查是否为 ARP 请求
    pub fn is_request(&self) -> bool {
        u16::from_be(self.hdr.ar_op) == ArpOp::ARPOP_REQUEST as u16
    }

    /// 检查是否为 ARP 响应
    pub fn is_reply(&self) -> bool {
        u16::from_be(self.hdr.ar_op) == ArpOp::ARPOP_REPLY as u16
    }

    /// 获取发送方 MAC 地址
    pub fn sender_mac(&self) -> [u8; ETH_ALEN] {
        self.ar_sha
    }

    /// 获取发送方 IP 地址
    pub fn sender_ip(&self) -> u32 {
        u32::from_be(self.ar_sip)
    }

    /// 获取目标 MAC 地址
    pub fn target_mac(&self) -> [u8; ETH_ALEN] {
        self.ar_tha
    }

    /// 获取目标 IP 地址
    pub fn target_ip(&self) -> u32 {
        u32::from_be(self.ar_tip)
    }
}

/// ARP 缓存条目
///
/// 缓存 IP 地址到 MAC 地址的映射
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpEntry {
    /// IP 地址
    pub ip: u32,
    /// MAC 地址
    pub mac: [u8; ETH_ALEN],
    /// 最后更新时间
    pub last_updated: u64,
    /// 是否有效
    pub valid: bool,
}

impl ArpEntry {
    /// 创建新的 ARP 缓存条目
    pub fn new(ip: u32, mac: [u8; ETH_ALEN]) -> Self {
        // TODO: 获取当前时间戳
        Self {
            ip,
            mac,
            last_updated: 0,
            valid: true,
        }
    }

    /// 检查条目是否过期
    ///
    /// # 参数
    /// - `timeout`: 超时时间 (秒)
    ///
    /// # 返回
    /// 是否过期
    pub fn is_expired(&self, timeout: u64) -> bool {
        // TODO: 实现时间比较
        false
    }
}

/// ARP 缓存
///
/// 简化实现：固定大小的哈希表
struct ArpCache {
    entries: [ArpEntry; ARP_CACHE_SIZE],
    count: usize,
}

impl ArpCache {
    const fn new() -> Self {
        const EMPTY_ENTRY: ArpEntry = ArpEntry {
            ip: 0,
            mac: [0; ETH_ALEN],
            last_updated: 0,
            valid: false,
        };

        Self {
            entries: [EMPTY_ENTRY; ARP_CACHE_SIZE],
            count: 0,
        }
    }

    /// 查找 ARP 缓存条目
    fn lookup(&self, ip: u32) -> Option<ArpEntry> {
        for entry in self.entries.iter() {
            if entry.valid && entry.ip == ip {
                return Some(*entry);
            }
        }
        None
    }

    /// 添加或更新 ARP 缓存条目
    fn update(&mut self, ip: u32, mac: [u8; ETH_ALEN]) {
        // 首先尝试更新现有条目
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.mac = mac;
                entry.last_updated = 0; // TODO: 获取当前时间
                return;
            }
        }

        // 如果未找到，添加新条目
        if self.count < ARP_CACHE_SIZE {
            self.entries[self.count] = ArpEntry::new(ip, mac);
            self.count += 1;
        } else {
            // 缓存已满，替换最旧的条目
            // 简化实现：替换第一个条目
            self.entries[0] = ArpEntry::new(ip, mac);
        }
    }

    /// 删除 ARP 缓存条目
    fn remove(&mut self, ip: u32) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.valid = false;
                self.count -= 1;
                return;
            }
        }
    }

    /// 清空 ARP 缓存
    fn clear(&mut self) {
        self.count = 0;
        for entry in self.entries.iter_mut() {
            entry.valid = false;
        }
    }
}

/// 全局 ARP 缓存
static mut ARP_CACHE: ArpCache = ArpCache::new();

/// 查找 ARP 缓存
///
/// # 参数
/// - `ip`: IP 地址 (网络字节序)
///
/// # 返回
/// 返回找到的 MAC 地址，如果未找到则返回 None
pub fn arp_lookup(ip: u32) -> Option<[u8; ETH_ALEN]> {
    unsafe {
        if let Some(entry) = ARP_CACHE.lookup(ip) {
            Some(entry.mac)
        } else {
            None
        }
    }
}

/// 更新 ARP 缓存
///
/// # 参数
/// - `ip`: IP 地址 (网络字节序)
/// - `mac`: MAC 地址
pub fn arp_update(ip: u32, mac: [u8; ETH_ALEN]) {
    unsafe {
        ARP_CACHE.update(ip, mac);
    }
}

/// 删除 ARP 缓存条目
///
/// # 参数
/// - `ip`: IP 地址 (网络字节序)
pub fn arp_remove(ip: u32) {
    unsafe {
        ARP_CACHE.remove(ip);
    }
}

/// 清空 ARP 缓存
pub fn arp_clear() {
    unsafe {
        ARP_CACHE.clear();
    }
}

/// 构造 ARP 请求报文
///
/// # 参数
/// - `skb`: SkBuff
/// - `sender_mac`: 发送方 MAC 地址
/// - `sender_ip`: 发送方 IP 地址 (网络字节序)
/// - `target_ip`: 目标 IP 地址 (网络字节序)
///
/// # 说明
/// 在 SkBuff 中添加 ARP 请求报文
pub fn arp_build_request(
    skb: &mut SkBuff,
    sender_mac: [u8; ETH_ALEN],
    sender_ip: u32,
    target_ip: u32,
) -> Result<(), ()> {
    // 分配空间用于 ARP 报文
    let ptr = skb.skb_put(ArpPacket::LEN as u32).ok_or(())?;

    unsafe {
        let arp_pkt = &mut *(ptr as *mut ArpPacket);

        // ARP 头部
        arp_pkt.hdr.ar_hrd = (ArpHrd::ARPHRD_ETHER as u16).to_be();
        arp_pkt.hdr.ar_pro = (ArpPro::ARPPROTO_IP as u16).to_be();
        arp_pkt.hdr.ar_hln = ETH_ALEN as u8;
        arp_pkt.hdr.ar_pln = 4; // IPv4 地址长度
        arp_pkt.hdr.ar_op = (ArpOp::ARPOP_REQUEST as u16).to_be();

        // 发送方地址
        arp_pkt.ar_sha = sender_mac;
        arp_pkt.ar_sip = sender_ip;

        // 目标地址
        arp_pkt.ar_tha = [0; ETH_ALEN]; // 请求时为空
        arp_pkt.ar_tip = target_ip;
    }

    Ok(())
}

/// 构造 ARP 响应报文
///
/// # 参数
/// - `skb`: SkBuff
/// - `sender_mac`: 发送方 MAC 地址
/// - `sender_ip`: 发送方 IP 地址 (网络字节序)
/// - `target_mac`: 目标 MAC 地址
/// - `target_ip`: 目标 IP 地址 (网络字节序)
///
/// # 说明
/// 在 SkBuff 中添加 ARP 响应报文
pub fn arp_build_reply(
    skb: &mut SkBuff,
    sender_mac: [u8; ETH_ALEN],
    sender_ip: u32,
    target_mac: [u8; ETH_ALEN],
    target_ip: u32,
) -> Result<(), ()> {
    // 分配空间用于 ARP 报文
    let ptr = skb.skb_put(ArpPacket::LEN as u32).ok_or(())?;

    unsafe {
        let arp_pkt = &mut *(ptr as *mut ArpPacket);

        // ARP 头部
        arp_pkt.hdr.ar_hrd = (ArpHrd::ARPHRD_ETHER as u16).to_be();
        arp_pkt.hdr.ar_pro = (ArpPro::ARPPROTO_IP as u16).to_be();
        arp_pkt.hdr.ar_hln = ETH_ALEN as u8;
        arp_pkt.hdr.ar_pln = 4; // IPv4 地址长度
        arp_pkt.hdr.ar_op = (ArpOp::ARPOP_REPLY as u16).to_be();

        // 发送方地址
        arp_pkt.ar_sha = sender_mac;
        arp_pkt.ar_sip = sender_ip;

        // 目标地址
        arp_pkt.ar_tha = target_mac;
        arp_pkt.ar_tip = target_ip;
    }

    Ok(())
}

/// 处理接收到的 ARP 报文
///
/// # 参数
/// - `skb`: SkBuff (包含 ARP 报文)
///
/// 接收并处理 ARP 数据包
///
/// # 参数
/// - `skb`: SkBuff (包含 ARP 数据包)
/// - `eth_hdr`: 以太网头部
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn arp_rcv(skb: &SkBuff, _eth_hdr: &crate::net::ethernet::EthHdr) -> Result<(), ()> {
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    // 解析 ARP 报文
    let arp_pkt = ArpPacket::from_bytes(data).ok_or(())?;

    // 检查硬件类型和协议类型
    if u16::from_be(arp_pkt.hdr.ar_hrd) != (ArpHrd::ARPHRD_ETHER as u16) {
        return Ok(()); // 忽略非以太网 ARP
    }

    if u16::from_be(arp_pkt.hdr.ar_pro) != (ArpPro::ARPPROTO_IP as u16) {
        return Ok(()); // 忽略非 IPv4 ARP
    }

    // 更新 ARP 缓存 (学习发送方的地址映射)
    let sender_ip = arp_pkt.sender_ip();
    let sender_mac = arp_pkt.sender_mac();
    arp_update(sender_ip, sender_mac);

    // 处理 ARP 请求
    if arp_pkt.is_request() {
        let target_ip = arp_pkt.target_ip();

        // TODO: 检查目标 IP 是否为本机 IP
        // 如果是，则发送 ARP 响应
        let _ = target_ip; // 暂时忽略警告
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arp_packet_size() {
        assert_eq!(core::mem::size_of::<ArpPacket>(), 28);
    }

    #[test]
    fn test_arp_cache_lookup() {
        let ip = 0xC0A80101; // 192.168.1.1
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

        unsafe {
            ARP_CACHE.update(ip, mac);
        }

        let result = arp_lookup(ip);
        assert_eq!(result, Some(mac));
    }
}
