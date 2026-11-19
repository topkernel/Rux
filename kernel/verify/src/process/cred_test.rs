//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for Cred (process credentials) initialization.
//! Copied from: kernel/src/process/task.rs

use proptest::prelude::*;

// Minimal Cap type for Cred
pub const CAP_VALID_MASK: u64 = (1u64 << 41) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap(u64);

impl Cap {
    pub const EMPTY: Cap = Cap(0);
    pub const FULL: Cap = Cap(CAP_VALID_MASK);

    pub fn is_empty(&self) -> bool { self.0 == 0 }
    pub fn is_full(&self) -> bool { self.0 == CAP_VALID_MASK }
}

// Copied Cred struct
pub struct Cred {
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
    pub fsuid: u32,
    pub fsgid: u32,
    pub cap_inheritable: Cap,
    pub cap_permitted: Cap,
    pub cap_effective: Cap,
    pub cap_bounding: Cap,
    pub cap_ambient: Cap,
}

impl Cred {
    pub fn new_init() -> Self {
        Self {
            uid: 0, gid: 0, euid: 0, egid: 0, suid: 0, sgid: 0, fsuid: 0, fsgid: 0,
            cap_inheritable: Cap::EMPTY,
            cap_permitted: Cap::FULL,
            cap_effective: Cap::FULL,
            cap_bounding: Cap::FULL,
            cap_ambient: Cap::EMPTY,
        }
    }

    pub fn new_user(uid: u32, gid: u32) -> Self {
        Self {
            uid, gid, euid: uid, egid: gid, suid: uid, sgid: gid, fsuid: uid, fsgid: gid,
            cap_inheritable: Cap::EMPTY,
            cap_permitted: Cap::EMPTY,
            cap_effective: Cap::EMPTY,
            cap_bounding: Cap::FULL,
            cap_ambient: Cap::EMPTY,
        }
    }
}

proptest! {
    #[test]
    fn test_new_init_all_zero_ids(_v in 0u8..1u8) {
        let c = Cred::new_init();
        assert_eq!(c.uid, 0);
        assert_eq!(c.gid, 0);
        assert_eq!(c.euid, 0);
        assert_eq!(c.egid, 0);
        assert_eq!(c.suid, 0);
        assert_eq!(c.sgid, 0);
        assert_eq!(c.fsuid, 0);
        assert_eq!(c.fsgid, 0);
    }

    #[test]
    fn test_new_init_cap_full_effective(_v in 0u8..1u8) {
        let c = Cred::new_init();
        assert!(c.cap_effective.is_full());
        assert!(c.cap_permitted.is_full());
        assert!(c.cap_bounding.is_full());
    }

    #[test]
    fn test_new_init_cap_empty(_v in 0u8..1u8) {
        let c = Cred::new_init();
        assert!(c.cap_inheritable.is_empty());
        assert!(c.cap_ambient.is_empty());
    }

    #[test]
    fn test_new_user_ids(uid in 0u32..65536u32, gid in 0u32..65536u32) {
        let c = Cred::new_user(uid, gid);
        // All user IDs match
        assert_eq!(c.uid, uid);
        assert_eq!(c.gid, gid);
        assert_eq!(c.euid, uid);
        assert_eq!(c.egid, gid);
        assert_eq!(c.suid, uid);
        assert_eq!(c.sgid, gid);
        assert_eq!(c.fsuid, uid);
        assert_eq!(c.fsgid, gid);
    }

    #[test]
    fn test_new_user_no_caps(uid in 0u32..65536u32, gid in 0u32..65536u32) {
        let c = Cred::new_user(uid, gid);
        assert!(c.cap_inheritable.is_empty());
        assert!(c.cap_permitted.is_empty());
        assert!(c.cap_effective.is_empty());
        assert!(c.cap_ambient.is_empty());
    }

    #[test]
    fn test_new_user_bounding_full(uid in 0u32..65536u32, gid in 0u32..65536u32) {
        let c = Cred::new_user(uid, gid);
        assert!(c.cap_bounding.is_full());
    }

    #[test]
    fn test_new_init_vs_new_user(uid in 1u32..65536u32, gid in 1u32..65536u32) {
        let init = Cred::new_init();
        let user = Cred::new_user(uid, gid);
        // Init has all caps, user has none (except bounding)
        assert!(init.cap_effective.is_full());
        assert!(user.cap_effective.is_empty());
        // Both have full bounding
        assert!(init.cap_bounding.is_full());
        assert!(user.cap_bounding.is_full());
    }

    #[test]
    fn test_new_user_root_is_not_init(_v in 0u8..1u8) {
        let root_user = Cred::new_user(0, 0);
        let init = Cred::new_init();
        // Both have uid=0, gid=0 but different capabilities
        assert_eq!(root_user.uid, init.uid);
        assert_eq!(root_user.gid, init.gid);
        // Root user has no effective caps, init has all
        assert!(root_user.cap_effective.is_empty());
        assert!(init.cap_effective.is_full());
    }
}
