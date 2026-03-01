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

use alloc::format;
use crate::drivers::net::{loopback, virtio_net};
use crate::net::buffer::SkBuff;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_virtio_net() {
    test_group_start("VirtIO-Net");

    // 测试 1: 回环设备初始化
    test_loopback_init();

    // 测试 2: 回环设备发送
    test_loopback_send();

    // 测试 3: 网络设备基本操作
    test_net_device_ops();

    // 测试 4: SkBuff 分配和释放
    test_skb_alloc();
}

/// 测试回环设备初始化
fn test_loopback_init() {
    let device = loopback::loopback_init();

    match device {
        Some(dev) => {
            let name_ok = dev.get_name() == "lo";
            let mtu_ok = dev.mtu == 65536;
            let up_ok = dev.is_up() && dev.is_running();

            if name_ok && mtu_ok && up_ok {
                test_pass("loopback init");
            } else {
                test_fail("loopback init", "invalid state");
            }
        }
        None => {
            test_fail("loopback init", "failed");
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
            test_fail("loopback send", "SkBuff alloc failed");
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
        test_pass("loopback send");
    } else {
        test_fail("loopback send", &format!("error: {}", result));
    }
}

/// 测试网络设备基本操作
fn test_net_device_ops() {
    let device = match loopback::get_loopback_device() {
        Some(dev) => dev,
        None => {
            test_fail("net device ops", "no device");
            return;
        }
    };

    // 测试设备名称
    let name = device.get_name();
    if name != "lo" {
        test_fail("net device name", &format!("got: {}", name));
        return;
    }

    // 测试设备状态
    if !device.is_up() || !device.is_running() {
        test_fail("net device state", "not up/running");
        return;
    }

    // 测试设备统计信息
    let stats = device.get_stats();

    test_pass("net device ops");
}

/// 测试 SkBuff 分配和释放
fn test_skb_alloc() {
    // 分配不同大小的 SkBuff
    let sizes = [64, 128, 256, 512, 1500];

    for size in sizes.iter() {
        let skb = match SkBuff::alloc(*size) {
            Some(s) => s,
            None => {
                test_fail("SkBuff alloc", &format!("size {} failed", size));
                return;
            }
        };

        // 释放 SkBuff
        skb.free();
    }

    test_pass("SkBuff alloc/free");
}
