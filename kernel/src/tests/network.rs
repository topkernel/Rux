//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Network subsystem test

use crate::net::buffer::{alloc_skb, kfree_skb};
use crate::drivers::net::{loopback_init, loopback_send, loopback};
use super::{test_pass, test_fail, test_group_start};

#[cfg(feature = "unit-test")]
pub fn test_network() {
    test_group_start("network");

    // Test 1: SkBuff allocation and deallocation
    test_skb_alloc();

    // Test 2: SkBuff data operations
    test_skb_data_ops();

    // Test 3: SkBuff push/pull operations
    test_skb_push_pull();

    // Test 4: Loopback device
    test_loopback();
}

fn test_skb_alloc() {
    // Allocate 1500 byte SkBuff
    let skb = alloc_skb(1500);
    if skb.is_none() {
        test_fail("SkBuff alloc", "failed");
        return;
    }

    let skb = skb.unwrap();
    let len_ok = skb.len() == 0;
    let empty_ok = skb.is_empty();

    // Free SkBuff
    kfree_skb(skb);

    if len_ok && empty_ok {
        test_pass("SkBuff alloc/free");
    } else {
        test_fail("SkBuff alloc", "invalid initial state");
    }
}

fn test_skb_data_ops() {
    let mut skb = match alloc_skb(1500) {
        Some(s) => s,
        None => {
            test_fail("SkBuff data ops", "alloc failed");
            return;
        }
    };

    // Test skb_put
    let data = b"Hello, World!";
    let result = skb.skb_put_data(data);
    if result.is_err() {
        test_fail("SkBuff data ops", "put_data failed");
        return;
    }

    let len_ok = skb.len() == data.len() as u32;
    let not_empty = !skb.is_empty();

    // Test skb_copy_bits
    let mut buf = [0u8; 32];
    let copied = skb.skb_copy_bits(0, &mut buf, data.len() as u32);
    let copy_ok = copied == data.len() as u32;
    let data_ok = &buf[..data.len()] == data;

    if len_ok && not_empty && copy_ok && data_ok {
        test_pass("SkBuff data ops");
    } else {
        test_fail("SkBuff data ops", "mismatch");
    }
}

fn test_skb_push_pull() {
    let mut skb = match alloc_skb(1500) {
        Some(s) => s,
        None => {
            test_fail("SkBuff push/pull", "alloc failed");
            return;
        }
    };

    // First put some data
    if skb.skb_put_data(b"World!").is_err() {
        test_fail("SkBuff push/pull", "put_data failed");
        return;
    }

    // Test skb_push
    let push_len = 7;
    let ptr = match skb.skb_push(push_len) {
        Some(p) => p,
        None => {
            test_fail("SkBuff push", "failed");
            return;
        }
    };
    unsafe {
        core::ptr::copy_nonoverlapping(b"Hello, ".as_ptr(), ptr, push_len as usize);
    }

    if skb.len() != 13 {
        test_fail("SkBuff push", "length mismatch");
        return;
    }

    // Test skb_pull
    if skb.skb_pull(7).is_none() {
        test_fail("SkBuff pull", "failed");
        return;
    }
    if skb.len() == 6 {
        test_pass("SkBuff push/pull");
    } else {
        test_fail("SkBuff pull", "length mismatch");
    }
}

fn test_loopback() {
    // Reset loopback device statistics
    loopback::loopback_reset_stats();

    // Initialize loopback device
    let device = match loopback_init() {
        Some(d) => d,
        None => {
            test_fail("loopback", "init failed");
            return;
        }
    };

    let name_ok = device.get_name() == "lo";
    let mtu_ok = device.mtu == 65536;
    let up_ok = device.is_up();
    let running_ok = device.is_running();

    if !name_ok || !mtu_ok || !up_ok || !running_ok {
        test_fail("loopback init", "invalid state");
        return;
    }

    // Test sending packet
    let skb = match alloc_skb(100) {
        Some(s) => s,
        None => {
            test_fail("loopback send", "SkBuff alloc failed");
            return;
        }
    };
    let result = loopback_send(skb);
    if result != 0 {
        test_fail("loopback send", "failed");
        return;
    }

    // Check statistics
    let stats = device.get_stats();
    if stats.tx_packets == 1 && stats.rx_packets == 1 {
        test_pass("loopback device");
    } else {
        test_fail("loopback stats", "mismatch");
    }
}
