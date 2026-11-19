//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 bitmap scanner invariant tests.
//!
//! Types copied from: kernel/src/fs/ext4/allocator.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/ext4/allocator.rs
// ============================================================================

/// Find a free bit in a block bitmap.
///
/// Scans byte-by-byte, skipping fully-occupied bytes (0xFF).
pub fn find_free_bit(bitmap: &[u8], start: u64, max_bits: u64) -> Option<u64> {
    let start_bit = start as usize;

    for (i, &byte) in bitmap.iter().enumerate() {
        let bit_offset = i * 8;

        if bit_offset + 8 <= start_bit {
            continue;
        }

        // Skip fully-occupied bytes (fast path)
        if byte == 0xFF {
            continue;
        }

        for bit in 0..8 {
            let abs_bit = bit_offset + bit;
            if abs_bit as u64 >= max_bits {
                return None;
            }
            if abs_bit < start_bit {
                continue;
            }
            if (byte & (1 << bit)) == 0 {
                return Some(abs_bit as u64);
            }
        }
    }

    None
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-BITMAP-1: All-zeros bitmap finds bit 0
    #[test]
    fn test_all_zeros_find_first(len in 1usize..128usize) {
        let bitmap = vec![0u8; len];
        let result = find_free_bit(&bitmap, 0, (len * 8) as u64);
        prop_assert_eq!(result, Some(0));
    }

    /// INV-BITMAP-2: All-ones bitmap returns None
    #[test]
    fn test_all_ones_find_none(len in 1usize..128usize) {
        let bitmap = vec![0xFFu8; len];
        let result = find_free_bit(&bitmap, 0, (len * 8) as u64);
        prop_assert!(result.is_none());
    }

    /// INV-BITMAP-3: find_free_bit respects start offset
    #[test]
    fn test_start_offset(
        len in 4usize..64usize,
        start in 1u64..63u64,
    ) {
        let bitmap = vec![0u8; len];
        let max = (len * 8) as u64;
        if start >= max {
            return Ok(());
        }
        let result = find_free_bit(&bitmap, start, max);
        prop_assert!(result.is_some());
        prop_assert!(result.unwrap() >= start);
    }

    /// INV-BITMAP-4: find_free_bit respects max_bits
    #[test]
    fn test_max_bits(
        len in 4usize..64usize,
        max_bits in 1u64..63u64,
    ) {
        let bitmap = vec![0u8; len];
        if max_bits == 0 {
            return Ok(());
        }
        let result = find_free_bit(&bitmap, 0, max_bits);
        prop_assert!(result.is_some());
        prop_assert!(result.unwrap() < max_bits);
    }

    /// INV-BITMAP-5: Single free bit is found
    #[test]
    fn test_single_free_bit(
        len in 2usize..64usize,
        free_byte_idx in 0usize..64usize,
        free_bit_idx in 0u8..8u8,
    ) {
        let mut bitmap = vec![0xFFu8; len];
        let byte_idx = free_byte_idx % len;
        bitmap[byte_idx] &= !(1 << free_bit_idx);
        let max = (len * 8) as u64;
        let result = find_free_bit(&bitmap, 0, max);
        prop_assert_eq!(result, Some((byte_idx * 8 + free_bit_idx as usize) as u64));
    }

    /// INV-BITMAP-6: Empty bitmap returns None
    #[test]
    fn test_empty_bitmap(_v in 0u8..1u8) {
        let bitmap: Vec<u8> = vec![];
        prop_assert!(find_free_bit(&bitmap, 0, 0).is_none());
    }

    /// INV-BITMAP-7: max_bits = 0 returns None
    #[test]
    fn test_zero_max_bits(len in 1usize..64usize) {
        let bitmap = vec![0u8; len];
        prop_assert!(find_free_bit(&bitmap, 0, 0).is_none());
    }

    /// INV-BITMAP-8: start beyond max returns None
    #[test]
    fn test_start_beyond_max(
        len in 1usize..64usize,
        max in 1u64..63u64,
    ) {
        let bitmap = vec![0u8; len];
        let result = find_free_bit(&bitmap, max + 1, max);
        prop_assert!(result.is_none());
    }

    /// INV-BITMAP-9: Free bit after start is found
    #[test]
    fn test_free_after_occupied_start(
        len in 4usize..64usize,
        start in 1u64..31u64,
    ) {
        let mut bitmap = vec![0u8; len];
        let max = (len * 8) as u64;
        if start >= max {
            return Ok(());
        }
        // Occupy bits before start
        let start_byte = (start as usize) / 8;
        let start_bit = (start as usize) % 8;
        for i in 0..start_byte {
            bitmap[i] = 0xFF;
        }
        if start_bit > 0 {
            bitmap[start_byte] = (1 << start_bit) - 1;
        }

        let result = find_free_bit(&bitmap, start, max);
        prop_assert!(result.is_some());
        prop_assert!(result.unwrap() >= start);
    }

    /// INV-BITMAP-10: Last bit is the only free one
    #[test]
    fn test_last_bit_free(len in 2usize..64usize) {
        let mut bitmap = vec![0xFFu8; len];
        let last_byte = len - 1;
        bitmap[last_byte] = 0xFE; // Only bit 0 (the MSB) is free
        let max = (len * 8) as u64;
        let result = find_free_bit(&bitmap, 0, max);
        prop_assert_eq!(result, Some((last_byte * 8) as u64));
    }

    /// INV-BITMAP-11: Bitmap with alternating pattern
    #[test]
    fn test_alternating_pattern(len in 4usize..64usize) {
        let bitmap: Vec<u8> = (0..len).map(|i| if i % 2 == 0 { 0xAA } else { 0x55 }).collect();
        let max = (len * 8) as u64;
        let result = find_free_bit(&bitmap, 0, max);
        prop_assert!(result.is_some());
        prop_assert!(result.unwrap() < max);
    }

    /// INV-BITMAP-12: find_free_bit with start at bit boundary
    #[test]
    fn test_byte_boundary_start(
        len in 4usize..64usize,
        byte_offset in 1usize..32usize,
    ) {
        let bitmap = vec![0u8; len];
        let start = (byte_offset * 8) as u64;
        let max = (len * 8) as u64;
        if start >= max {
            return Ok(());
        }
        let result = find_free_bit(&bitmap, start, max);
        prop_assert_eq!(result, Some(start));
    }
}
