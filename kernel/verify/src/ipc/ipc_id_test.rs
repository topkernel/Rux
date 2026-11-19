//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for IPC ID encoding/decoding and permission bitfield math.
//! Copied from: kernel/src/ipc/util.rs

use proptest::prelude::*;

// Copied IPC ID functions
pub fn ipc_build_id(index: usize, seq: u32) -> i32 {
    (((index as u32) << 16) | (seq & 0xFFFF)) as i32
}

pub fn ipc_id_to_index(id: i32) -> usize {
    ((id as u32) >> 16) as usize
}

pub fn ipc_id_seq(id: i32) -> u32 {
    (id as u32) & 0xFFFF
}

// Copied update_mode logic
pub fn ipc_update_mode(old_mode: u16, new_mode: u16) -> u16 {
    (new_mode & 0o777) | (old_mode & !0o777)
}

// Permission bitfield extraction
pub fn owner_bits(mode: u16) -> u16 { (mode >> 6) & 0o7 }
pub fn group_bits(mode: u16) -> u16 { (mode >> 3) & 0o7 }
pub fn other_bits(mode: u16) -> u16 { mode & 0o7 }

proptest! {
    #[test]
    fn test_ipc_id_roundtrip(index in 0usize..65536usize, seq in 0u32..65536u32) {
        let id = ipc_build_id(index, seq);
        prop_assert_eq!(ipc_id_to_index(id), index);
        prop_assert_eq!(ipc_id_seq(id), seq & 0xFFFF);
    }

    #[test]
    fn test_ipc_id_seq_truncates(seq in 0u32..) {
        let id = ipc_build_id(0, seq);
        prop_assert_eq!(ipc_id_seq(id), seq & 0xFFFF);
    }

    #[test]
    fn test_ipc_id_negative_id(index in 32768usize..65536usize, seq in 0u32..65536u32) {
        // index >= 32768 sets bit 31, producing a "negative" i32
        let id = ipc_build_id(index, seq);
        prop_assert!(id < 0, "High index should produce negative ID");
        prop_assert_eq!(ipc_id_to_index(id), index);
    }

    #[test]
    fn test_ipc_id_max_index(index in 0usize..65536usize) {
        // index fits in u16 (bits 31:16)
        let id = ipc_build_id(index, 0);
        let extracted = ipc_id_to_index(id);
        prop_assert_eq!(extracted, index);
    }

    #[test]
    fn test_ipc_creat_excl_nowait_distinct(_v in 0u8..1u8) {
        let creat: i32 = 0o1000;
        let excl: i32 = 0o2000;
        let nowait: i32 = 0o4000;
        assert!(creat > 0 && (creat & (creat - 1)) == 0);
        assert!(excl > 0 && (excl & (excl - 1)) == 0);
        assert!(nowait > 0 && (nowait & (nowait - 1)) == 0);
        assert_eq!(creat & excl, 0);
        assert_eq!(creat & nowait, 0);
        assert_eq!(excl & nowait, 0);
    }

    #[test]
    fn test_ipc_cmd_distinct(_v in 0u8..1u8) {
        let cmds = [0i32, 1, 2, 3, 11, 12, 13, 14, 15, 16, 17];
        for i in 0..cmds.len() {
            for j in (i+1)..cmds.len() {
                assert_ne!(cmds[i], cmds[j]);
            }
        }
    }

    #[test]
    fn test_update_mode_preserves_non_perm_bits(old_mode in 0u16.., new_mode in 0u16..0o7777u16) {
        let result = ipc_update_mode(old_mode, new_mode);
        // Lower 9 bits come from new_mode
        assert_eq!(result & 0o777, new_mode & 0o777);
        // Upper bits come from old_mode
        assert_eq!(result & !0o777, old_mode & !0o777);
    }

    #[test]
    fn test_update_mode_idempotent(_v in 0u8..1u8) {
        // update_mode with same permission bits is identity
        let mode: u16 = 0o644;
        assert_eq!(ipc_update_mode(mode, mode), mode);
    }

    #[test]
    fn test_perm_bits_extraction(mode in 0u16..0o777u16) {
        let ow = owner_bits(mode);
        let gr = group_bits(mode);
        let ot = other_bits(mode);
        // Each fits in 3 bits
        assert!(ow <= 0o7);
        assert!(gr <= 0o7);
        assert!(ot <= 0o7);
        // Reassembly: (ow << 6) | (gr << 3) | ot == mode
        assert_eq!((ow << 6) | (gr << 3) | ot, mode);
    }

    #[test]
    fn test_shm_flags_distinct(_v in 0u8..1u8) {
        let flags = [0o10000i32, 0o20000i32, 0o40000i32, 0o100000i32];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0);
            }
        }
    }

    #[test]
    fn test_msg_flags_distinct(_v in 0u8..1u8) {
        let flags = [0o10000i32, 0o20000i32, 0o40000i32];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0);
            }
        }
    }

    #[test]
    fn test_mq_flags_distinct(_v in 0u8..1u8) {
        let flags = [0o100i32, 0o200i32, 0o400i32, 0o2000000i32];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0);
            }
        }
    }
}
