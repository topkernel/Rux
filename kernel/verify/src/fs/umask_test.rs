//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for umask bit operations.
//! Copied from: kernel/src/fs/fs_struct.rs

use proptest::prelude::*;

// Copied FsStruct umask logic
pub const DEFAULT_UMASK: u32 = 0o022;

pub fn apply_umask(mode: u32, umask: u32) -> u32 {
    mode & !(umask & 0o777)
}

pub fn set_umask(mask: u32) -> u32 {
    mask & 0o777
}

proptest! {
    #[test]
    fn test_apply_umask_clears_masked_bits(mode in 0u32..0o777u32, umask in 0u32..0o777u32) {
        let result = apply_umask(mode, umask);
        // Cleared bits should not be set in result
        let masked = umask & 0o777;
        assert_eq!(result & masked, 0,
            "apply_umask should clear bits set in umask");
    }

    #[test]
    fn test_apply_umask_preserves_unmasked(mode in 0u32..0o777u32, umask in 0u32..0o777u32) {
        let result = apply_umask(mode, umask);
        let unmasked = mode & !(umask & 0o777);
        assert_eq!(result, unmasked);
    }

    #[test]
    fn test_apply_umask_idempotent(mode in 0u32..0o777u32, umask in 0u32..0o777u32) {
        let once = apply_umask(mode, umask);
        let twice = apply_umask(once, umask);
        assert_eq!(once, twice, "apply_umask should be idempotent");
    }

    #[test]
    fn test_apply_umask_zero_mode(umask in 0u32..0o777u32) {
        assert_eq!(apply_umask(0, umask), 0);
    }

    #[test]
    fn test_apply_umask_zero_umask(mode in 0u32..0o777u32) {
        assert_eq!(apply_umask(mode, 0), mode);
    }

    #[test]
    fn test_default_umask(mode in 0u32..0o777u32) {
        let result = apply_umask(mode, DEFAULT_UMASK);
        // Default 0o022: clear group-write and other-write
        assert_eq!(result & 0o022, 0);
    }

    #[test]
    fn test_default_umask_777(_v in 0u8..1u8) {
        // 0o777 with umask 0o022 → 0o755
        assert_eq!(apply_umask(0o777, DEFAULT_UMASK), 0o755);
    }

    #[test]
    fn test_default_umask_666(_v in 0u8..1u8) {
        // 0o666 with umask 0o022 → 0o644
        assert_eq!(apply_umask(0o666, DEFAULT_UMASK), 0o644);
    }

    #[test]
    fn test_set_umask_masks_to_9_bits(mask in 0u32..0o7777u32) {
        let result = set_umask(mask);
        assert!(result <= 0o777, "set_umask should mask to 9 bits");
    }

    #[test]
    fn test_set_umask_full_preserved(_v in 0u8..1u8) {
        assert_eq!(set_umask(0o777), 0o777);
    }

    #[test]
    fn test_apply_umask_only_affects_low_9_bits(mode in 0u32..0xFFFF_FFFFu32, umask in 0u32..0o777u32) {
        let result = apply_umask(mode, umask);
        // High bits (>9) are preserved
        assert_eq!(result & !0o777, mode & !0o777);
    }
}
