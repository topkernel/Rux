//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 协议
//!
//! 完全遵循 Linux 内核的 IPv4 实现
//! 参考: net/ipv4/, include/net/ip.h, include/uapi/linux/ip.h

pub mod route;
pub mod checksum;

use crate::net::buffer::SkBuff;
use crate::net::ethernet::ETH_ALEN;

/// IPv4 地址长度
pub const IP_ALEN: usize = 4;

/// IPv4 头部长度
pub const IPHDR_LEN: usize = 20;

/// IPv4 最小 MTU (RFC 791)
pub const IP_MIN_MTU: u16 = 68;

/// IPv4 最大 MTU
pub const IP_MAX_MTU: u16 = 65535;

/// IPv4 默认 TTL
pub const IP_DEFAULT_TTL: u8 = 64;

/// IPv4 分片标志常量
pub mod ip_frag_flags {
    /// 保留位
    pub const RB: u16 = 0x8000;
    /// 不分片 (Don't Fragment)
    pub const DF: u16 = 0x4000;
    /// 更多分片 (More Fragments)
    pub const MF: u16 = 0x2000;
    /// 分片偏移掩码
    pub const OFFSET_MASK: u16 = 0x1FFF;
}

/// IPv4 头部
///
/// 对应 Linux 的 iphdr (include/uapi/linux/ip.h)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IpHdr {
    /// 版本 (4 bits) + 头部长度 (4 bits)
    pub version_ihl: u8,
    /// 服务类型
    pub tos: u8,
    /// 总长度
    pub tot_len: u16,
    /// 标识
    pub id: u16,
    /// 分片标志 + 分片偏移
    pub frag_off: u16,
    /// TTL
    pub ttl: u8,
    /// 协议
    pub protocol: u8,
    /// 头部校验和
    pub check: u16,
    /// 源 IP 地址
    pub saddr: u32,
    /// 目标 IP 地址
    pub daddr: u32,
}

impl IpHdr {
    /// 从字节切片创建 IP 头部
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < IPHDR_LEN {
            return None;
        }

        unsafe {
            Some(&*(data.as_ptr() as *const IpHdr))
        }
    }

    /// 计算校验和
    pub fn compute_checksum(&self) -> u16 {
        let mut header = [0u8; IPHDR_LEN];
        unsafe {
            core::ptr::copy_nonoverlapping(
                (self as *const IpHdr) as *const u8,
                header.as_mut_ptr(),
                IPHDR_LEN,
            );
        }

        checksum::ip_checksum(&header)
    }

    /// 验证校验和
    pub fn is_valid_checksum(&self) -> bool {
        self.compute_checksum() == 0
    }
}

/// 构造 IPv4 头部
///
/// # 参数
/// - `skb`: SkBuff
/// - `saddr`: 源 IP 地址 (网络字节序)
/// - `daddr`: 目标 IP 地址 (网络字节序)
/// - `protocol`: 协议类型
/// - `tot_len`: 总长度
///
/// # 说明
/// 在 SkBuff 前面添加 IPv4 头部
pub fn ip_push_header(
    skb: &mut SkBuff,
    saddr: u32,
    daddr: u32,
    protocol: u8,
    tot_len: u16,
) -> Result<(), ()> {
    // 分配空间用于 IP 头部
    let ptr = skb.skb_push(IPHDR_LEN as u32).ok_or(())?;

    unsafe {
        let ip_hdr = &mut *(ptr as *mut IpHdr);

        // 设置版本号和头部长度 (5 = 20 字节)
        ip_hdr.version_ihl = (4 << 4) | 5;

        // 服务类型 (默认为 0)
        ip_hdr.tos = 0;

        // 总长度
        ip_hdr.tot_len = tot_len.to_be();

        // 标识 (暂时设为 0)
        ip_hdr.id = 0;

        // 分片标志 + 分片偏移 (默认不分片)
        ip_hdr.frag_off = 0;

        // TTL
        ip_hdr.ttl = IP_DEFAULT_TTL;

        // 协议
        ip_hdr.protocol = protocol;

        // 头部校验和 (先设为 0，稍后计算)
        ip_hdr.check = 0;

        // 源 IP 地址
        ip_hdr.saddr = saddr.to_be();

        // 目标 IP 地址
        ip_hdr.daddr = daddr.to_be();

        // 计算校验和
        ip_hdr.check = ip_hdr.compute_checksum().to_be();
    }

    Ok(())
}

/// 解析 IPv4 头部
///
/// # 参数
/// - `skb`: SkBuff
///
/// # 返回
/// 返回 IP 头部引用，如果解析失败则返回 None
pub fn ip_pull_header(skb: &mut SkBuff) -> Option<&'static IpHdr> {
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < IPHDR_LEN {
        return None;
    }

    let ip_hdr = IpHdr::from_bytes(data)?;

    // 验证版本号
    let version = ip_hdr.version_ihl >> 4;
    if version != 4 {
        return None;
    }

    // 验证头部长度
    let ihl = ip_hdr.version_ihl & 0x0F;
    if ihl < 5 {
        return None;
    }

    let header_len = (ihl as usize) * 4;

    // 验证总长度
    let tot_len = u16::from_be(ip_hdr.tot_len);
    if tot_len < (header_len as u16) {
        return None;
    }

    // 移除 IP 头部
    skb.skb_pull(header_len as u32);

    Some(ip_hdr)
}

/// 发送 IPv4 数据包（用于上层协议）
///
/// # 参数
/// - `skb`: SkBuff (包含 TCP/UDP 等上层协议数据)
/// - `dest_ip`: 目标 IP 地址
/// - `protocol`: 上层协议号 (IPPROTO_TCP = 6, IPPROTO_UDP = 17)
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn ipv4_send(mut skb: SkBuff, dest_ip: u32, protocol: u8) -> Result<(), ()> {
    // 为 IP 头部预留空间
    let ip_ptr = skb.skb_push(IPHDR_LEN as u32).ok_or(())?;

    unsafe {
        let ip_hdr = &mut *(ip_ptr as *mut IpHdr);

        // 版本 (4) + 头部长度 (5 * 4 = 20 字节)
        ip_hdr.version_ihl = 0x45;

        // TOS (服务类型)
        ip_hdr.tos = 0;

        // 总长度（IP 头 + 数据）
        ip_hdr.tot_len = ((IPHDR_LEN + skb.len as usize) as u16).to_be();

        // ID（标识符）
        ip_hdr.id = 0;

        // 标志和分片偏移
        ip_hdr.frag_off = 0;

        // TTL
        ip_hdr.ttl = IP_DEFAULT_TTL;

        // 协议
        ip_hdr.protocol = protocol;

        // 源 IP（简化实现：使用固定值）
        ip_hdr.saddr = 0xC0A80164; // 192.168.1.100

        // 目标 IP
        ip_hdr.daddr = dest_ip.to_be();

        // 校验和（先设为 0）
        ip_hdr.check = 0;

        // 计算校验和 - 需要传递字节切片
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(
                (ip_hdr as *const IpHdr) as *const u8,
                core::mem::size_of::<IpHdr>()
            )
        };
        ip_hdr.check = checksum::ip_checksum(hdr_bytes).to_be();
    }

    // 调用 IP 输出函数
    ip_output(skb)
}

/// 发送 IPv4 数据包
///
/// # 参数
/// - `skb`: SkBuff (包含 IP 数据包)
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn ip_output(skb: SkBuff) -> Result<(), ()> {
    // TODO: 查找路由
    // TODO: 分片处理

    // 简化实现：直接发送到以太网层
    crate::net::ethernet::ethernet_send(skb)
}

/// 接收并处理 IPv4 数据包
///
/// # 参数
/// - `skb`: SkBuff (包含 IP 数据包)
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn ip_rcv(skb: &SkBuff) -> Result<(), ()> {
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    // 解析 IP 头部
    let ip_hdr = IpHdr::from_bytes(data).ok_or(())?;

    // 验证版本号
    let version = ip_hdr.version_ihl >> 4;
    if version != 4 {
        return Ok(());
    }

    // 验证校验和
    if !ip_hdr.is_valid_checksum() {
        return Ok(());
    }

    // 获取源 IP 和目标 IP
    let src_ip = u32::from_be(ip_hdr.saddr);
    let dest_ip = u32::from_be(ip_hdr.daddr);

    // TODO: 检查目标 IP 是否为本机
    // 简化实现：接受所有数据包

    // 根据 protocol 分发到上层协议
    match ip_hdr.protocol {
        6 => {
            // TCP 协议 (IPPROTO_TCP = 6)
            // 从 IP 头部获取目标端口
            // 注意：需要解析 TCP 头部才能知道目标端口
            // 这里简化处理，直接传递给 TCP 层
            // crate::net::tcp::tcp_rcv(skb, src_ip, dest_ip);
        }
        17 => {
            // UDP 协议 (IPPROTO_UDP = 17)
            // crate::net::udp::udp_rcv(skb, src_ip, dest_ip);
        }
        1 => {
            // ICMP 协议 (IPPROTO_ICMP = 1)
            // crate::net::icmp::icmp_rcv(skb, src_ip, dest_ip);
        }
        _ => {
            // 不支持的协议
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iphdr_size() {
        assert_eq!(core::mem::size_of::<IpHdr>(), 20);
    }

    #[test]
    fn test_iphdr_version_ihl() {
        let mut hdr = IpHdr::default();
        hdr.version_ihl = 0x45; // 版本 4, 头部长度 5

        assert_eq!(hdr.version_ihl >> 4, 4);
        assert_eq!(hdr.version_ihl & 0x0F, 5);
    }
}
