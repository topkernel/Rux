//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for capability LSM hook dispatch logic.
//! Copied from: kernel/src/security/cap_lsm.rs

use proptest::prelude::*;

// Copied Cap type (minimal)
pub const CAP_VALID_MASK: u64 = (1u64 << 41) - 1;
pub const CAP_LAST_CAP: u32 = 40;
pub const CAP_KILL: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap(u64);

impl Cap {
    pub const EMPTY: Cap = Cap(0);
    pub const FULL: Cap = Cap(CAP_VALID_MASK);

    pub const fn new(mask: u64) -> Self {
        Cap(mask & CAP_VALID_MASK)
    }

    pub fn has(&self, cap: u32) -> bool {
        if cap > CAP_LAST_CAP {
            return false;
        }
        (self.0 >> cap) & 1 == 1
    }
}

// Hook IDs
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

const EPERM: i32 = 1;

// Simplified Cred
pub struct Cred {
    pub cap_effective: Cap,
}

// CapLsm hook dispatch — pure logic extracted from kernel's CapLsm::call
pub fn cap_lsm_call(hook: HookId, cred: Option<&Cred>, cap: u32) -> i32 {
    match hook {
        HookId::Capable => {
            match cred {
                None => return -EPERM,
                Some(c) => {
                    if c.cap_effective.has(cap) {
                        0
                    } else {
                        -EPERM
                    }
                }
            }
        }
        HookId::SignalSend => {
            match cred {
                None => return 0,  // No opinion
                Some(c) => {
                    if c.cap_effective.has(CAP_KILL) {
                        0  // Allowed
                    } else {
                        0  // No opinion — caller does UID-based check
                    }
                }
            }
        }
        HookId::InodePermission => 0,  // Placeholder
        HookId::Execve => 0,           // Handled by execve code path
        HookId::IpcPermission => 0,    // Handled directly by ipc_check_permissions
        HookId::Mount => 0,            // Handled by mount syscall handler
        HookId::Umount => 0,           // Handled by umount syscall handler
    }
}

// Build a Cap with a specific bit set
fn cap_with_bit(bit: u32) -> Cap {
    if bit <= CAP_LAST_CAP {
        Cap(1u64 << bit)
    } else {
        Cap(0)
    }
}

proptest! {
    #[test]
    fn test_capable_allows_with_cap(cap in 0u32..41u32) {
        let cred = Cred { cap_effective: Cap::FULL };
        assert_eq!(cap_lsm_call(HookId::Capable, Some(&cred), cap), 0);
    }

    #[test]
    fn test_capable_denies_without_cap(cap in 0u32..41u32) {
        let cred = Cred { cap_effective: Cap::EMPTY };
        assert_eq!(cap_lsm_call(HookId::Capable, Some(&cred), cap), -EPERM);
    }

    #[test]
    fn test_capable_denies_null_cred(cap in 0u32..41u32) {
        assert_eq!(cap_lsm_call(HookId::Capable, None, cap), -EPERM);
    }

    #[test]
    fn test_capable_selective_cap(requested_cap in 0u32..41u32, held_cap in 0u32..41u32) {
        let cred = Cred { cap_effective: cap_with_bit(held_cap) };
        let result = cap_lsm_call(HookId::Capable, Some(&cred), requested_cap);
        if requested_cap == held_cap {
            assert_eq!(result, 0);
        } else {
            assert_eq!(result, -EPERM);
        }
    }

    #[test]
    fn test_signal_send_no_opinion_without_cap_kill(_v in 0u8..1u8) {
        let cred = Cred { cap_effective: Cap::EMPTY };
        // SignalSend returns 0 even without CAP_KILL (no opinion)
        assert_eq!(cap_lsm_call(HookId::SignalSend, Some(&cred), 0), 0);
    }

    #[test]
    fn test_signal_send_allows_with_cap_kill(_v in 0u8..1u8) {
        let cred = Cred { cap_effective: cap_with_bit(CAP_KILL) };
        assert_eq!(cap_lsm_call(HookId::SignalSend, Some(&cred), 0), 0);
    }

    #[test]
    fn test_signal_send_no_opinion_null_cred(_v in 0u8..1u8) {
        // Null cred → no opinion (returns 0, not -EPERM)
        assert_eq!(cap_lsm_call(HookId::SignalSend, None, 0), 0);
    }

    #[test]
    fn test_other_hooks_always_allow(hook_val in 2u32..7u32) {
        let cred = Cred { cap_effective: Cap::EMPTY };
        let hook = match hook_val {
            2 => HookId::InodePermission,
            3 => HookId::Execve,
            4 => HookId::IpcPermission,
            5 => HookId::Mount,
            6 => HookId::Umount,
            _ => return Err(TestCaseError::reject("invalid hook")),
        };
        assert_eq!(cap_lsm_call(hook, Some(&cred), 0), 0);
    }

    #[test]
    fn test_cap_kill_is_bit_5(_v in 0u8..1u8) {
        assert_eq!(CAP_KILL, 5);
        let cap = cap_with_bit(CAP_KILL);
        assert!(cap.has(CAP_KILL));
        assert!(!cap.has(CAP_KILL - 1));
        assert!(!cap.has(CAP_KILL + 1));
    }

    #[test]
    fn test_cap_valid_mask_41_bits(_v in 0u8..1u8) {
        assert_eq!(CAP_VALID_MASK, (1u64 << 41) - 1);
        assert_eq!(CAP_LAST_CAP, 40);
        assert!(Cap::FULL.has(40));
        assert!(!Cap::FULL.has(41));
    }
}
