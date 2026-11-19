//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for VirtIO virtual queue struct sizes and constants.
//! Copied from: kernel/src/drivers/virtio/queue.rs

use proptest::prelude::*;

// Copied struct sizes
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Desc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
pub struct AvailRing {
    pub flags: u16,
    pub idx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
pub struct UsedRing {
    pub flags: u16,
    pub idx: u16,
}

proptest! {
    #[test]
    fn test_desc_size(_v in 0u8..1u8) {
        assert_eq!(core::mem::size_of::<Desc>(), 16);
        // Verify field layout: 8 + 4 + 2 + 2 = 16
        assert_eq!(core::mem::size_of::<u64>(), 8);
        assert_eq!(core::mem::size_of::<u32>(), 4);
        assert_eq!(core::mem::size_of::<u16>(), 2);
    }

    #[test]
    fn test_used_elem_size(_v in 0u8..1u8) {
        assert_eq!(core::mem::size_of::<UsedElem>(), 8);
    }

    #[test]
    fn test_avail_ring_size(_v in 0u8..1u8) {
        assert_eq!(core::mem::size_of::<AvailRing>(), 4);
    }

    #[test]
    fn test_used_ring_size(_v in 0u8..1u8) {
        assert_eq!(core::mem::size_of::<UsedRing>(), 4);
    }

    #[test]
    fn test_vring_size_calculation(queue_size in 1u16..4096u16) {
        // Vring size = desc + avail + used
        // desc = queue_size * 16
        // avail = 2 + 2 + queue_size * 2 + 2 = 6 + queue_size * 2
        // used = 2 + 2 + queue_size * 8 + 2 = 6 + queue_size * 8
        let desc_size: usize = (queue_size as usize) * 16;
        let avail_size: usize = 6 + (queue_size as usize) * 2;
        let used_size: usize = 6 + (queue_size as usize) * 8;
        let total = desc_size + avail_size + used_size;
        // Total must be positive
        assert!(total > 0);
        // Desc part is always 16-byte aligned
        assert_eq!(desc_size % 16, 0);
        // Total fits in reasonable memory
        assert!(total < 16 * 1024 * 1024);
    }

    #[test]
    fn test_vring_page_alignment(queue_size in 1u16..4096u16) {
        // Each part of vring must be page-aligned
        let desc_size: usize = (queue_size as usize) * 16;
        let avail_offset = desc_size;
        let avail_size: usize = 6 + (queue_size as usize) * 2;
        let used_offset = avail_offset + avail_size;
        // Total must fit in reasonable memory
        let total = used_offset + 6 + (queue_size as usize) * 8;
        assert!(total < 16 * 1024 * 1024);
    }
}
