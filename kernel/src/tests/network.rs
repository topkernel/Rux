//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络子系统测试

use crate::println;
use crate::net::buffer::{SkBuff, alloc_skb, kfree_skb, PacketType, EthProtocol, IpProtocol};
use crate::drivers::net::{loopback_init, get_loopback_device, loopback_send};

#[cfg(feature = "unit-test")]
pub fn test_network() {
    println!("test: ===== Starting Network Subsystem Tests =====");

    // 测试 1: SkBuff 分配和释放
    println!("test: 1. Testing SkBuff allocation...");
    test_skb_alloc();

    // 测试 2: SkBuff 数据操作
    println!("test: 2. Testing SkBuff data operations...");
    test_skb_data_ops();

    // 测试 3: SkBuff push/pull 操作
    println!("test: 3. Testing SkBuff push/pull...");
    test_skb_push_pull();

    // 测试 4: 回环设备
    println!("test: 4. Testing loopback device...");
    test_loopback();

    println!("test: ===== Network Subsystem Tests Completed =====");
}

fn test_skb_alloc() {
    // 分配 1500 字节的 SkBuff
    let skb = alloc_skb(1500);
    assert!(skb.is_some(), "Failed to allocate SkBuff");

    let skb = skb.unwrap();
    assert_eq!(skb.len(), 0, "New SkBuff should have zero length");
    assert!(skb.is_empty(), "New SkBuff should be empty");

    // 释放 SkBuff
    kfree_skb(skb);

    println!("test:    SUCCESS - SkBuff allocation/deallocation works");
}

fn test_skb_data_ops() {
    let mut skb = alloc_skb(1500).expect("Failed to allocate SkBuff");

    // 测试 skb_put
    let data = b"Hello, World!";
    let result = skb.skb_put_data(data);
    assert!(result.is_ok(), "Failed to put data");

    assert_eq!(skb.len(), data.len() as u32, "Length mismatch");
    assert!(!skb.is_empty(), "SkBuff should not be empty");

    // 测试 skb_copy_bits
    let mut buf = [0u8; 32];
    let copied = skb.skb_copy_bits(0, &mut buf, data.len() as u32);
    assert_eq!(copied, data.len() as u32, "Copy length mismatch");
    assert_eq!(&buf[..data.len()], data, "Data mismatch");

    println!("test:    SUCCESS - SkBuff data operations work");
}

fn test_skb_push_pull() {
    let mut skb = alloc_skb(1500).expect("Failed to allocate SkBuff");

    // 先 put 一些数据
    skb.skb_put_data(b"World!").expect("Failed to put data");

    // 测试 skb_push
    let push_len = 7;
    let ptr = skb.skb_push(push_len).expect("Failed to push");
    unsafe {
        core::ptr::copy_nonoverlapping(b"Hello, ".as_ptr(), ptr, push_len as usize);
    }

    assert_eq!(skb.len(), 13, "Length after push mismatch");

    // 测试 skb_pull
    skb.skb_pull(7).expect("Failed to pull");
    assert_eq!(skb.len(), 6, "Length after pull mismatch");

    println!("test:    SUCCESS - SkBuff push/pull operations work");
}

fn test_loopback() {
    // 初始化回环设备
    let device = loopback_init();
    assert!(device.is_some(), "Failed to initialize loopback device");

    let device = device.unwrap();
    assert_eq!(device.get_name(), "lo", "Device name mismatch");
    assert_eq!(device.mtu, 65536, "MTU mismatch");
    assert!(device.is_up(), "Loopback should be up");
    assert!(device.is_running(), "Loopback should be running");

    // 测试发送数据包
    let skb = alloc_skb(100).expect("Failed to allocate SkBuff");
    let result = loopback_send(skb);
    assert_eq!(result, 0, "Loopback send failed");

    // 检查统计信息
    let stats = device.get_stats();
    assert_eq!(stats.tx_packets, 1, "TX packets mismatch");
    assert_eq!(stats.rx_packets, 1, "RX packets mismatch");

    println!("test:    SUCCESS - Loopback device works");
}
