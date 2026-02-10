//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 路由表
//!
//! 完全遵循 Linux 内核的路由表实现
//! 参考: net/ipv4/route.c, include/net/route.h

use crate::net::buffer::SkBuff;
use crate::config::ROUTE_TABLE_SIZE;

/// 路由表条目
///
/// 对应 Linux 的 rtable (include/net/route.h)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RouteEntry {
    /// 目标网络地址
    pub dst: u32,
    /// 网络掩码
    pub mask: u32,
    /// 网关地址
    pub gateway: u32,
    /// 输出设备索引
    pub oif: u32,
    /// MTU
    pub mtu: u32,
    /// 标志
    pub flags: RouteFlags,
}

/// 路由标志
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteFlags(pub u32);

impl RouteFlags {
    /// 路由已启动
    pub const RTF_UP: u32 = 0x0001;
    /// 网关路由
    pub const RTF_GATEWAY: u32 = 0x0002;
    /// 主机路由
    pub const RTF_HOST: u32 = 0x0004;
    /// 重启后恢复
    pub const RTF_REINSTATE: u32 = 0x0008;
    /// 动态路由
    pub const RTF_DYNAMIC: u32 = 0x0010;
    /// 修改的路由
    pub const RTF_MODIFIED: u32 = 0x0020;
    /// 恶意重定向
    pub const RTF_MALICED: u32 = 0x0040;
    /// 转发
    pub const RTF_FWD: u32 = 0x0080;
    /// 本地地址
    pub const RTF_LOCAL: u32 = 0x0100;
    /// 广播路由
    pub const RTF_BROADCAST: u32 = 0x0200;
    /// 网络地址
    pub const RTF_NETWORK: u32 = 0x0400;
}

impl RouteEntry {
    /// 创建新的路由条目
    pub fn new(dst: u32, mask: u32, gateway: u32, oif: u32, mtu: u32) -> Self {
        Self {
            dst,
            mask,
            gateway,
            oif,
            mtu,
            flags: RouteFlags(0),
        }
    }

    /// 检查是否为网关路由
    pub fn is_gateway(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_GATEWAY) != 0
    }

    /// 检查是否为主机路由
    pub fn is_host(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_HOST) != 0
    }

    /// 检查是否为网络路由
    pub fn is_network(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_NETWORK) != 0
    }

    /// 检查地址是否匹配此路由
    pub fn matches(&self, addr: u32) -> bool {
        (addr & self.mask) == (self.dst & self.mask)
    }
}

/// 路由表
///
/// 简化实现：固定大小的路由表
struct RouteTable {
    entries: [Option<RouteEntry>; ROUTE_TABLE_SIZE],
    count: usize,
}

impl RouteTable {
    const fn new() -> Self {
        const NONE: Option<RouteEntry> = None;
        Self {
            entries: [NONE; ROUTE_TABLE_SIZE],
            count: 0,
        }
    }

    /// 查找路由
    fn lookup(&self, dst: u32) -> Option<RouteEntry> {
        // 最长前缀匹配
        let mut best_match: Option<RouteEntry> = None;
        let mut best_mask = 0u32;

        for entry in self.entries.iter() {
            if let Some(route) = entry {
                if route.matches(dst) && route.mask >= best_mask {
                    best_match = Some(*route);
                    best_mask = route.mask;
                }
            }
        }

        best_match
    }

    /// 添加路由
    fn add(&mut self, route: RouteEntry) -> Result<(), ()> {
        if self.count >= ROUTE_TABLE_SIZE {
            return Err(());
        }

        self.entries[self.count] = Some(route);
        self.count += 1;
        Ok(())
    }

    /// 删除路由
    fn remove(&mut self, dst: u32, mask: u32) -> bool {
        for i in 0..self.count {
            if let Some(route) = self.entries[i] {
                if route.dst == dst && route.mask == mask {
                    // 移除条目
                    for j in i..self.count - 1 {
                        self.entries[j] = self.entries[j + 1];
                    }
                    self.entries[self.count - 1] = None;
                    self.count -= 1;
                    return true;
                }
            }
        }
        false
    }

    /// 清空路由表
    fn clear(&mut self) {
        self.count = 0;
        for entry in self.entries.iter_mut() {
            *entry = None;
        }
    }
}

/// 全局路由表
static mut ROUTE_TABLE: RouteTable = RouteTable::new();

/// 查找路由
///
/// # 参数
/// - `dst`: 目标 IP 地址 (主机字节序)
///
/// # 返回
/// 返回找到的路由条目，如果未找到则返回 None
pub fn route_lookup(dst: u32) -> Option<RouteEntry> {
    unsafe { ROUTE_TABLE.lookup(dst) }
}

/// 添加路由
///
/// # 参数
/// - `dst`: 目标网络地址 (主机字节序)
/// - `mask`: 网络掩码 (主机字节序)
/// - `gateway`: 网关地址 (主机字节序)
/// - `oif`: 输出设备索引
/// - `mtu`: MTU
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn route_add(dst: u32, mask: u32, gateway: u32, oif: u32, mtu: u32) -> Result<(), ()> {
    let route = RouteEntry::new(dst, mask, gateway, oif, mtu);
    unsafe { ROUTE_TABLE.add(route) }
}

/// 删除路由
///
/// # 参数
/// - `dst`: 目标网络地址 (主机字节序)
/// - `mask`: 网络掩码 (主机字节序)
///
/// # 返回
/// 是否成功删除
pub fn route_remove(dst: u32, mask: u32) -> bool {
    unsafe { ROUTE_TABLE.remove(dst, mask) }
}

/// 清空路由表
pub fn route_clear() {
    unsafe { ROUTE_TABLE.clear() }
}

/// 初始化默认路由
///
/// 添加本地回环路由和直连路由
pub fn route_init() {
    // 本地回环路由: 127.0.0.0/8
    let _ = route_add(
        0x7F000000, // 127.0.0.0
        0xFF000000, // 255.0.0.0
        0,          // 无网关
        0,          // 回环设备 (lo)
        16436,      // 回环设备 MTU
    );

    // 直连网络: 192.168.1.0/24
    let _ = route_add(
        0xC0A80100, // 192.168.1.0
        0xFFFFFF00, // 255.255.255.0
        0,          // 无网关
        1,          // 以太网设备 (eth0)
        1500,       // 以太网 MTU
    );
}

/// 根据路由发送数据包
///
/// # 参数
/// - `skb`: SkBuff
/// - `dst`: 目标 IP 地址
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(())
pub fn route_output(skb: SkBuff, dst: u32) -> Result<(), ()> {
    // 查找路由
    let route = route_lookup(dst).ok_or(())?;

    // TODO: 根据路由发送数据包
    // 1. 如果有网关，使用网关 MAC 地址
    // 2. 如果没有网关，使用目标 IP 的 MAC 地址
    // 3. 查找 ARP 缓存获取 MAC 地址
    // 4. 调用设备发送函数

    // 简化实现：直接释放数据包
    skb.free();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_entry_match() {
        let route = RouteEntry::new(
            0xC0A80100, // 192.168.1.0
            0xFFFFFF00, // 255.255.255.0
            0,
            1,
            1500,
        );

        assert!(route.matches(0xC0A80101)); // 192.168.1.1
        assert!(route.matches(0xC0A801FF)); // 192.168.1.255
        assert!(!route.matches(0xC0A80201)); // 192.168.2.1
    }

    #[test]
    fn test_route_lookup() {
        unsafe {
            ROUTE_TABLE.clear();
        }

        // 添加路由
        let _ = route_add(
            0xC0A80100, // 192.168.1.0
            0xFFFFFF00, // 255.255.255.0
            0,
            1,
            1500,
        );

        // 查找路由
        let route = route_lookup(0xC0A80101);
        assert!(route.is_some());
    }
}
