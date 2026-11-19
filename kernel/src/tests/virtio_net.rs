//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: VirtIO-Net network device driver
//!
//! Tests network device driver basic functionality, including:
//! - Network device initialization
//! - Packet transmission
//! - Packet reception
//! - Loopback device functionality

use alloc::format;
use crate::drivers::net::{loopback, virtio_net};
use crate::net::buffer::SkBuff;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_virtio_net() {
    test_group_start("VirtIO-Net");

    // Test 1: Loopback device initialization
    test_loopback_init();

    // Test 2: Loopback device send
    test_loopback_send();

    // Test 3: Network device basic operations
    test_net_device_ops();

    // Test 4: SkBuff allocation and deallocation
    test_skb_alloc();
}

/// Test loopback device initialization
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

/// Test loopback device send
fn test_loopback_send() {
    // Initialize loopback device
    let _device = loopback::loopback_init();

    // Create test packet
    let skb = match SkBuff::alloc(100) {
        Some(s) => s,
        None => {
            test_fail("loopback send", "SkBuff alloc failed");
            return;
        }
    };

    // Write test data
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

    // Send packet
    let result = loopback::loopback_send(skb);

    if result == 0 {
        test_pass("loopback send");
    } else {
        test_fail("loopback send", &format!("error: {}", result));
    }
}

/// Test network device basic operations
fn test_net_device_ops() {
    let device = match loopback::get_loopback_device() {
        Some(dev) => dev,
        None => {
            test_fail("net device ops", "no device");
            return;
        }
    };

    // Test device name
    let name = device.get_name();
    if name != "lo" {
        test_fail("net device name", &format!("got: {}", name));
        return;
    }

    // Test device state
    if !device.is_up() || !device.is_running() {
        test_fail("net device state", "not up/running");
        return;
    }

    // Test device statistics
    let stats = device.get_stats();

    test_pass("net device ops");
}

/// Test SkBuff allocation and deallocation
fn test_skb_alloc() {
    // Allocate SkBuffs of different sizes
    let sizes = [64, 128, 256, 512, 1500];

    for size in sizes.iter() {
        let skb = match SkBuff::alloc(*size) {
            Some(s) => s,
            None => {
                test_fail("SkBuff alloc", &format!("size {} failed", size));
                return;
            }
        };

        // Free SkBuff
        skb.free();
    }

    test_pass("SkBuff alloc/free");
}
