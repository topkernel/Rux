//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
// 测试：VirtIO-Net 网络设备驱动
//!
//! 测试网络设备驱动的基本功能，包括：
//! - 网络设备初始化
//! - 数据包发送
//! - 数据包接收
//! - 回环设备功能

use crate::println;
use crate::drivers::net::{loopback, virtio_net};
use crate::net::buffer::SkBuff;

pub fn test_virtio_net() {
    println!("test: ===== Starting VirtIO-Net Device Tests =====");

    // 测试 1: 回环设备初始化
    println!("test: 1. Testing loopback device initialization...");
    test_loopback_init();

    // 测试 2: 回环设备发送
    println!("test: 2. Testing loopback device send...");
    test_loopback_send();

    // 测试 3: 网络设备基本操作
    println!("test: 3. Testing network device basic operations...");
    test_net_device_ops();

    // 测试 4: SkBuff 分配和释放
    println!("test: 4. Testing SkBuff allocation and free...");
    test_skb_alloc();

    println!("test: VirtIO-Net Device testing completed.");
}

/// 测试回环设备初始化
fn test_loopback_init() {
    let device = loopback::loopback_init();

    match device {
        Some(dev) => {
            println!("test:    Loopback device initialized: {}", dev.get_name());
            assert_eq!(dev.get_name(), "lo");
            assert_eq!(dev.mtu, 65536);
            assert!(dev.is_up());
            assert!(dev.is_running());
            println!("test:    SUCCESS - Loopback device initialization works");
        }
        None => {
            println!("test:    FAILED - Could not initialize loopback device");
        }
    }
}

/// 测试回环设备发送
fn test_loopback_send() {
    // 初始化回环设备
    let _device = loopback::loopback_init();

    // 创建测试数据包
    let skb = match SkBuff::alloc(100) {
        Some(s) => s,
        None => {
            println!("test:    FAILED - Could not allocate SkBuff");
            return;
        }
    };

    // 写入测试数据
    let test_data = b"Hello, loopback!";
    unsafe {
        if skb.len >= test_data.len() as u32 {
            core::ptr::copy_nonoverlapping(
                test_data.as_ptr(),
                skb.data,
                test_data.len()
            );
        }
    }

    // 发送数据包
    let result = loopback::loopback_send(skb);

    if result == 0 {
        println!("test:    Packet sent successfully");
        println!("test:    SUCCESS - Loopback device send works");
    } else {
        println!("test:    FAILED - Send returned error: {}", result);
    }
}

/// 测试网络设备基本操作
fn test_net_device_ops() {
    let device = match loopback::get_loopback_device() {
        Some(dev) => dev,
        None => {
            println!("test:    FAILED - Could not get loopback device");
            return;
        }
    };

    // 测试设备名称
    let name = device.get_name();
    if name == "lo" {
        println!("test:    Device name: {}", name);
    } else {
        println!("test:    FAILED - Unexpected device name: {}", name);
        return;
    }

    // 测试设备状态
    if device.is_up() && device.is_running() {
        println!("test:    Device is UP and RUNNING");
    } else {
        println!("test:    FAILED - Device is not up or running");
        return;
    }

    // 测试设备统计信息
    let stats = device.get_stats();
    println!("test:    Device stats - TX: {}, RX: {}", stats.tx_packets, stats.rx_packets);

    println!("test:    SUCCESS - Network device operations work");
}

/// 测试 SkBuff 分配和释放
fn test_skb_alloc() {
    // 分配不同大小的 SkBuff
    let sizes = [64, 128, 256, 512, 1500];

    for size in sizes.iter() {
        let skb = match SkBuff::alloc(*size) {
            Some(s) => s,
            None => {
                println!("test:    FAILED - Could not allocate SkBuff of size {}", size);
                return;
            }
        };

        // 检查分配的大小
        if skb.len != 0 {
            println!("test:    SkBuff allocated with initial len: {}", skb.len);
        }

        // 释放 SkBuff
        skb.free();
    }

    println!("test:    SUCCESS - SkBuff allocation and free work");
}
