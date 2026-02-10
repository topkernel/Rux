//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络设备基类
//!
//! 完全遵循 Linux 内核的 net_device 设计
//! 参考: include/linux/netdevice.h, net/core/dev.c

use crate::net::buffer::SkBuff;
use spin::Mutex;

/// 设备名最大长度
///
/// 对应 Linux 的 IFNAMSIZ
pub const IFNAMSIZ: usize = 16;

/// 硬件地址最大长度
///
/// 对应 Linux 的 MAX_ADDR_LEN
pub const MAX_ADDR_LEN: usize = 32;

/// ARP 硬件类型
///
/// 对应 Linux 的 ARPHRD_* (include/linux/if_arp.h)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpHrdType {
    /// 回环设备
    ARPHRD_LOOPBACK = 772,
    /// 以太网
    ARPHRD_ETHER = 1,
    /// 无 (None)
    ARPHRD_VOID = 0xFFFF,
}

/// 网络设备操作接口
///
/// 对应 Linux 的 net_device_ops
#[repr(C)]
pub struct NetDeviceOps {
    /// 发送数据包
    ///
    /// # 参数
    /// - `skb`: 要发送的数据包
    ///
    /// # 返回
    /// 成功返回 0，失败返回负数错误码
    pub xmit: fn(skb: SkBuff) -> i32,

    /// 设备初始化（可选）
    pub init: Option<fn() -> i32>,

    /// 设备卸载（可选）
    pub uninit: Option<fn() -> i32>,

    /// 获取统计信息（可选）
    pub get_stats: Option<fn() -> DeviceStats>,
}

/// 网络设备统计信息
///
/// 对应 Linux 的 rtnl_link_stats64
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct DeviceStats {
    /// 接收包数
    pub rx_packets: u64,
    /// 发送包数
    pub tx_packets: u64,
    /// 接收字节数
    pub rx_bytes: u64,
    /// 发送字节数
    pub tx_bytes: u64,
    /// 接收错误数
    pub rx_errors: u64,
    /// 发送错误数
    pub tx_errors: u64,
    /// 接收丢弃数
    pub rx_dropped: u64,
    /// 发送丢弃数
    pub tx_dropped: u64,
    /// 多播接收包数
    pub multicast: u64,
}

/// 网络设备
///
/// 对应 Linux 的 net_device
///
/// # 说明
/// - 所有网络设备必须实现此结构
/// - 使用函数指针进行多态调用
#[repr(C)]
pub struct NetDevice {
    /// 设备名 (例如 "lo", "eth0")
    pub name: [u8; IFNAMSIZ],
    /// 设备索引
    pub ifindex: u32,
    /// MTU (最大传输单元)
    pub mtu: u32,
    /// 硬件类型 (ARPHRD_ETHER, ARPHRD_LOOPBACK, etc.)
    pub type_: ArpHrdType,
    /// 硬件地址 (MAC 地址)
    pub addr: [u8; MAX_ADDR_LEN],
    /// 硬件地址长度
    pub addr_len: u8,
    /// 设备操作接口
    pub netdev_ops: &'static NetDeviceOps,
    /// 私有数据
    pub priv_: *mut u8,
    /// 统计信息
    pub stats: DeviceStats,
    /// 设备状态
    pub flags: u32,
    /// 接收队列长度
    pub rx_queue_len: u32,
}

unsafe impl Send for NetDevice {}

/// 设备状态标志
///
/// 对应 Linux 的 IFF_* (include/linux/if.h)
pub mod dev_flags {
    /// 接口已启动
    pub const IFF_UP: u32 = 0x1;
    /// 接口已广播
    pub const IFF_BROADCAST: u32 = 0x2;
    /// 接口是回环设备
    pub const IFF_LOOPBACK: u32 = 0x8;
    /// 接口正在运行
    pub const IFF_RUNNING: u32 = 0x40;
    /// 接口已启用多播
    pub const IFF_MULTICAST: u32 = 0x1000;
}

/// 网络设备注册表
///
/// 简化实现：使用计数器跟踪设备数量（使用 Mutex 保护）
static DEV_COUNT: Mutex<usize> = Mutex::new(0);

impl NetDevice {
    /// 设置硬件地址
    ///
    /// # 参数
    /// - `addr`: 硬件地址
    /// - `len`: 地址长度
    pub fn set_address(&mut self, addr: &[u8], len: u8) {
        self.addr_len = len;
        self.addr[..len as usize].copy_from_slice(&addr[..len as usize]);
    }

    /// 获取设备名
    pub fn get_name(&self) -> &str {
        unsafe {
            let len = self.name.iter().position(|&c| c == 0).unwrap_or(IFNAMSIZ);
            core::str::from_utf8_unchecked(&self.name[..len])
        }
    }

    /// 发送数据包
    ///
    /// # 参数
    /// - `skb`: 要发送的数据包
    ///
    /// # 返回
    /// 成功返回 0，失败返回负数错误码
    pub fn xmit(&mut self, skb: SkBuff) -> i32 {
        (self.netdev_ops.xmit)(skb)
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> DeviceStats {
        if let Some(get_stats_fn) = self.netdev_ops.get_stats {
            get_stats_fn()
        } else {
            self.stats
        }
    }

    /// 启动设备
    pub fn up(&mut self) {
        self.flags |= dev_flags::IFF_UP | dev_flags::IFF_RUNNING;
    }

    /// 关闭设备
    pub fn down(&mut self) {
        self.flags &= !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING);
    }

    /// 检查设备是否已启动
    pub fn is_up(&self) -> bool {
        (self.flags & dev_flags::IFF_UP) != 0
    }

    /// 检查设备是否正在运行
    pub fn is_running(&self) -> bool {
        (self.flags & dev_flags::IFF_RUNNING) != 0
    }
}

/// 注册网络设备
///
/// # 参数
/// - `device`: 要注册的设备
///
/// # 返回
/// 成功返回分配的设备索引，失败返回负数错误码
///
/// # 说明
/// - 将设备添加到全局设备列表
/// - 分配设备索引
pub fn register_netdevice(device: &'static mut NetDevice) -> i32 {
    let mut count = DEV_COUNT.lock();
    // 分配设备索引
    device.ifindex = *count as u32;

    // 增加计数
    *count += 1;

    device.ifindex as i32
}

/// 注销网络设备
///
/// # 参数
/// - `device`: 要注销的设备
pub fn unregister_netdevice(device: &mut NetDevice) {
    // 从全局列表中移除
    // 简化实现：仅标记设备
    device.flags &= !dev_flags::IFF_UP;
}

/// 根据索引查找网络设备
///
/// # 参数
/// - `ifindex`: 设备索引
///
/// # 返回
/// 返回找到的设备，如果未找到则返回 None
pub fn get_netdevice_by_index(ifindex: u32) -> Option<&'static mut NetDevice> {
    // 简化实现：目前只支持查找回环设备
    // 完整实现需要维护设备链表
    if ifindex == 0 {
        crate::drivers::net::get_loopback_device()
    } else {
        None
    }
}

/// 根据名称查找网络设备
///
/// # 参数
/// - `name`: 设备名
///
/// # 返回
/// 返回找到的设备，如果未找到则返回 None
pub fn get_netdevice_by_name(name: &str) -> Option<&'static mut NetDevice> {
    // 简化实现：目前只支持查找回环设备
    // 完整实现需要维护设备链表
    if name == "lo" {
        crate::drivers::net::get_loopback_device()
    } else {
        None
    }
}

/// 获取所有网络设备数量
pub fn get_netdevice_count() -> usize {
    *DEV_COUNT.lock()
}
