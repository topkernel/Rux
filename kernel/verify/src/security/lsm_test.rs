//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for LSM hook framework (sorted insertion, dispatch).
//! Copied from: kernel/src/security/lsm.rs

use proptest::prelude::*;

// Copied types
pub type LsmResult = i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HookId {
    Capable = 0,
    SignalSend = 1,
    InodePermission = 2,
    Execve = 3,
    IpcPermission = 4,
    Mount = 5,
    Umount = 6,
}

// Mock LSM module for testing
struct MockLsm {
    name_str: &'static str,
    order_val: u32,
    // For each hook: 0 = allow, negative = deny, None = not handled
    results: [Option<i32>; 7],
}

impl MockLsm {
    fn new(name: &'static str, order: u32, results: [Option<i32>; 7]) -> Self {
        Self { name_str: name, order_val: order, results }
    }
}

const MAX_LSM_COUNT: usize = 4;

// Replicate register_lsm sorted-insertion logic with a Vec-based mock
fn register_lsm_sorted(chain: &mut Vec<(u32, u32, [Option<i32>; 7])>, lsm: (u32, u32, [Option<i32>; 7])) {
    if chain.len() >= MAX_LSM_COUNT {
        return;
    }
    let mut pos = chain.len();
    for i in 0..chain.len() {
        if lsm.1 < chain[i].1 {
            pos = i;
            break;
        }
    }
    chain.insert(pos, lsm);
}

// Replicate security_hook_call dispatch: returns first negative
fn security_hook_call(chain: &[(u32, u32, [Option<i32>; 7])], hook_idx: usize) -> i32 {
    for lsm in chain {
        if let Some(result) = lsm.2[hook_idx] {
            if result < 0 {
                return result;
            }
        }
    }
    0
}

proptest! {
    #[test]
    fn test_hook_id_discriminants(_v in 0u8..1u8) {
        assert_eq!(HookId::Capable as u32, 0);
        assert_eq!(HookId::SignalSend as u32, 1);
        assert_eq!(HookId::InodePermission as u32, 2);
        assert_eq!(HookId::Execve as u32, 3);
        assert_eq!(HookId::IpcPermission as u32, 4);
        assert_eq!(HookId::Mount as u32, 5);
        assert_eq!(HookId::Umount as u32, 6);
    }

    #[test]
    fn test_hook_id_count(_v in 0u8..1u8) {
        // 7 hook IDs total (0..=6)
        let all = [
            HookId::Capable, HookId::SignalSend, HookId::InodePermission,
            HookId::Execve, HookId::IpcPermission, HookId::Mount, HookId::Umount,
        ];
        assert_eq!(all.len(), 7);
        // Each discriminant matches its position
        for (i, hook) in all.iter().enumerate() {
            assert_eq!(*hook as u32, i as u32);
        }
    }

    #[test]
    fn test_sorted_insertion_single(orders in proptest::collection::vec(0u32..100u32, 1..4)) {
        let mut chain: Vec<(u32, u32, [Option<i32>; 7])> = Vec::new();
        let allow_all: [Option<i32>; 7] = [Some(0); 7];

        for (i, &order) in orders.iter().enumerate() {
            register_lsm_sorted(&mut chain, (i as u32, order, allow_all));
        }

        // Verify sorted by order
        for i in 1..chain.len() {
            assert!(chain[i - 1].1 <= chain[i].1,
                "chain not sorted at index {}: {} > {}", i - 1, chain[i-1].1, chain[i].1);
        }
    }

    #[test]
    fn test_max_lsm_count(orders in proptest::collection::vec(10u32..100u32, 5..8)) {
        let mut chain: Vec<(u32, u32, [Option<i32>; 7])> = Vec::new();
        let allow_all: [Option<i32>; 7] = [Some(0); 7];

        for (i, &order) in orders.iter().enumerate() {
            register_lsm_sorted(&mut chain, (i as u32, order, allow_all));
        }

        // Should not exceed MAX_LSM_COUNT
        assert!(chain.len() <= MAX_LSM_COUNT, "chain length {} exceeds max {}", chain.len(), MAX_LSM_COUNT);
    }

    #[test]
    fn test_dispatch_all_allow(hook_idx in 0usize..7usize) {
        let allow_all: [Option<i32>; 7] = [Some(0); 7];
        let chain = vec![
            (0, 0, allow_all),
            (1, 100, allow_all),
        ];
        assert_eq!(security_hook_call(&chain, hook_idx), 0);
    }

    #[test]
    fn test_dispatch_first_deny_wins(hook_idx in 0usize..7usize, deny_pos in 0usize..3usize) {
        let mut chain = Vec::new();
        for i in 0..3 {
            let mut results: [Option<i32>; 7] = [Some(0); 7];
            if i == deny_pos {
                results[hook_idx] = Some(-1); // This LSM denies
            }
            chain.push((i as u32, (i * 10) as u32, results));
        }

        let result = security_hook_call(&chain, hook_idx);
        assert_eq!(result, -1, "first deny should win");
    }

    #[test]
    fn test_dispatch_no_opinion(hook_idx in 0usize..7usize) {
        // LSMs that return None (no opinion) should be skipped
        let no_opinion: [Option<i32>; 7] = [None; 7];
        let chain = vec![
            (0, 0, no_opinion),
            (1, 1, no_opinion),
        ];
        assert_eq!(security_hook_call(&chain, hook_idx), 0);
    }

    #[test]
    fn test_dispatch_empty_chain(hook_idx in 0usize..7usize) {
        let chain: Vec<(u32, u32, [Option<i32>; 7])> = Vec::new();
        assert_eq!(security_hook_call(&chain, hook_idx), 0);
    }

    #[test]
    fn test_sorted_insertion_preserves_count(orders in proptest::collection::vec(0u32..50u32, 1..4)) {
        let mut chain: Vec<(u32, u32, [Option<i32>; 7])> = Vec::new();
        let allow_all: [Option<i32>; 7] = [Some(0); 7];
        let count = orders.len().min(MAX_LSM_COUNT);

        for (i, &order) in orders.iter().enumerate() {
            if i >= MAX_LSM_COUNT { break; }
            register_lsm_sorted(&mut chain, (i as u32, order, allow_all));
        }

        assert_eq!(chain.len(), count);
    }

    #[test]
    fn test_lsm_result_type(_v in 0u8..1u8) {
        // LsmResult is i32: 0 = allow, negative = deny
        let allow: LsmResult = 0;
        let deny: LsmResult = -1;
        assert!(allow >= 0);
        assert!(deny < 0);
    }
}
