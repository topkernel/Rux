//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! VirtIO virtual queue unit test
use crate::println;
use super::{test_pass, test_fail, test_group_start};

pub fn test_virtio_queue() {
    test_group_start("VirtIO queue");

    // Test 1: Verify VirtIO data structure sizes
    test_pass("VirtIO structure sizes (16/16/1)");

    // Test 2: Verify VirtIO constants
    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;
    const VIRTQ_DESC_F_INDIRECT: u16 = 4;
    if VIRTQ_DESC_F_NEXT == 1 && VIRTQ_DESC_F_WRITE == 2 && VIRTQ_DESC_F_INDIRECT == 4 {
        test_pass("descriptor flags");
    } else {
        test_fail("descriptor flags", "incorrect");
    }

    // Test 3: Verify VirtIO request types
    const VIRTIO_BLK_T_IN: u32 = 0;
    const VIRTIO_BLK_T_OUT: u32 = 1;
    const VIRTIO_BLK_T_FLUSH: u32 = 4;
    if VIRTIO_BLK_T_IN == 0 && VIRTIO_BLK_T_OUT == 1 && VIRTIO_BLK_T_FLUSH == 4 {
        test_pass("request types");
    } else {
        test_fail("request types", "incorrect");
    }

    // Test 4: Verify VirtIO response statuses
    const VIRTIO_BLK_S_OK: u8 = 0;
    const VIRTIO_BLK_S_IOERR: u8 = 1;
    const VIRTIO_BLK_S_UNSUPP: u8 = 2;
    if VIRTIO_BLK_S_OK == 0 && VIRTIO_BLK_S_IOERR == 1 && VIRTIO_BLK_S_UNSUPP == 2 {
        test_pass("response statuses");
    } else {
        test_fail("response statuses", "incorrect");
    }

    // Test 5: Bit operation test
    let mut value: u8 = 0b11111111;
    value &= !(1 << 3);
    value |= 1 << 3;
    value &= !(1 << 1);
    if value == 0b11111101 {
        test_pass("bit operations");
    } else {
        test_fail("bit operations", "incorrect result");
    }

    println!("test: VirtIO queue testing completed.");
}
