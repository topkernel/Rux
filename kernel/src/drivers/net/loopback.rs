//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 回环网络设备
//!
//! 完全遵循 Linux 内核的 loopback 设备实现
//! 参考: drivers/net/loopback.c

use crate::drivers::net::space::{NetDevice, NetDeviceOps, DeviceStats, ArpHrdType, dev_flags};
use crate::net::buffer::SkBuff;

/// 回环设备统计信息
static mut LO_STATS: DeviceStats = DeviceStats {
    rx_packets: 0,
    tx_packets: 0,
    rx_bytes: 0,
    tx_bytes: 0,
    rx_errors: 0,
    tx_errors: 0,
    rx_dropped: 0,
    tx_dropped: 0,
    multicast: 0,
};

/// 回环设备发送函数
///
/// # 参数
/// - `skb`: 要发送的数据包
///
/// # 返回
/// 始终返回 0 (成功)
///
/// # 说明
/// 回环设备的特殊之处在于：
/// - 发送的包立即被接收
/// - 不需要通过硬件
fn loopback_xmit(skb: SkBuff) -> i32 {
    unsafe {
        // 更新统计信息
        LO_STATS.tx_packets += 1;
        LO_STATS.tx_bytes += skb.len as u64;
        LO_STATS.rx_packets += 1;
        LO_STATS.rx_bytes += skb.len as u64;

        // TODO: 将数据包传递到网络协议栈
        // 目前简化实现：直接释放数据包
        // 完整实现应该调用 netif_rx(skb)

        // 释放数据包
        skb.free();
    }

    0
}

/// 回环设备统计信息获取函数
fn loopback_get_stats() -> DeviceStats {
    unsafe { LO_STATS }
}

/// 回环设备操作接口
static LOOPBACK_OPS: NetDeviceOps = NetDeviceOps {
    xmit: loopback_xmit,
    init: None,
    uninit: None,
    get_stats: Some(loopback_get_stats),
};

/// 回环设备
///
/// 静态分配的回环设备实例
static mut LO_DEVICE: Option<NetDevice> = None;

/// 初始化回环设备
///
/// # 返回
/// 成功返回设备指针，失败返回 None
pub fn loopback_init() -> Option<&'static mut NetDevice> {
    unsafe {
        // 检查是否已经初始化
        if LO_DEVICE.is_some() {
            return LO_DEVICE.as_mut();
        }

        // 创建回环设备
        let mut device = NetDevice {
            name: [0u8; 16],
            ifindex: 0,
            mtu: 65536,  // 回环设备 MTU 较大
            type_: ArpHrdType::ARPHRD_LOOPBACK,
            addr: [0u8; 32],
            addr_len: 0,
            netdev_ops: &LOOPBACK_OPS,
            priv_: core::ptr::null_mut(),
            stats: DeviceStats::default(),
            flags: dev_flags::IFF_UP | dev_flags::IFF_RUNNING | dev_flags::IFF_LOOPBACK,
            rx_queue_len: 0,
        };

        // 设置设备名
        let name = b"lo\0";
        device.name[..name.len()].copy_from_slice(name);

        // 设置地址 (回环设备没有 MAC 地址)
        device.addr_len = 0;

        // 存储设备
        LO_DEVICE = Some(device);

        LO_DEVICE.as_mut()
    }
}

/// 获取回环设备
///
/// # 返回
/// 返回回环设备指针，如果未初始化则返回 None
pub fn get_loopback_device() -> Option<&'static mut NetDevice> {
    unsafe { LO_DEVICE.as_mut() }
}

/// 发送数据包到回环设备
///
/// # 参数
/// - `skb`: 要发送的数据包
///
/// # 返回
/// 成功返回 0，失败返回负数错误码
pub fn loopback_send(skb: SkBuff) -> i32 {
    loopback_xmit(skb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loopback_init() {
        let device = loopback_init();
        assert!(device.is_some());

        let device = device.unwrap();
        assert_eq!(device.get_name(), "lo");
        assert_eq!(device.mtu, 65536);
        assert!(device.is_up());
        assert!(device.is_running());
    }

    #[test]
    fn test_loopback_xmit() {
        // 初始化回环设备
        loopback_init();

        // 创建测试数据包
        let skb = SkBuff::alloc(100).unwrap();

        // 发送数据包
        let result = loopback_send(skb);
        assert_eq!(result, 0);

        // 检查统计信息
        let stats = unsafe { LO_STATS };
        assert_eq!(stats.tx_packets, 1);
        assert_eq!(stats.rx_packets, 1);
    }
}
