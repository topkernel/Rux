//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Unix DAC permission check invariant tests.
//!
//! Functions copied from: kernel/src/fs/permission.rs
//! NOTE: simplified Cred struct (no security::has_capability dependency)

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/permission.rs
// ============================================================================

pub const MAY_EXEC: u32 = 0o001;
pub const MAY_WRITE: u32 = 0o002;
pub const MAY_READ: u32 = 0o004;

/// Minimal Cred for testing (no kernel dependency)
#[derive(Clone, Copy)]
pub struct Cred {
    pub euid: u32,
    pub egid: u32,
}

/// Simplified generic_permission (no CAP_DAC_OVERRIDE for testability)
pub fn generic_permission(
    inode_mode: u16,
    inode_uid: u32,
    inode_gid: u32,
    mask: u32,
    cred: &Cred,
) -> bool {
    let mode = inode_mode as u32;

    if cred.euid == inode_uid {
        // Owner permission bits (bits 8-6)
        ((mode >> 6) & 0o7) & mask == mask
    } else if cred.egid == inode_gid {
        // Group permission bits (bits 5-3)
        ((mode >> 3) & 0o7) & mask == mask
    } else {
        // Other permission bits (bits 2-0)
        (mode & 0o7) & mask == mask
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-PERM-1: MAY_READ | MAY_WRITE | MAY_EXEC == 0o7
    #[test]
    fn test_perm_bits(_v in 0u8..1u8) {
        prop_assert_eq!(MAY_READ | MAY_WRITE | MAY_EXEC, 0o7);
    }

    /// INV-PERM-2: owner read on 0o644
    #[test]
    fn test_owner_read_644(_v in 0u8..1u8) {
        let cred = Cred { euid: 42, egid: 0 };
        prop_assert!(generic_permission(0o644, 42, 100, MAY_READ, &cred));
    }

    /// INV-PERM-3: other read on 0o644
    #[test]
    fn test_other_read_644(_v in 0u8..1u8) {
        let cred = Cred { euid: 99, egid: 99 };
        prop_assert!(generic_permission(0o644, 42, 100, MAY_READ, &cred));
    }

    /// INV-PERM-4: other write denied on 0o644
    #[test]
    fn test_other_write_denied_644(_v in 0u8..1u8) {
        let cred = Cred { euid: 99, egid: 99 };
        prop_assert!(!generic_permission(0o644, 42, 100, MAY_WRITE, &cred));
    }

    /// INV-PERM-5: group matches for read on 0o640
    #[test]
    fn test_group_read_640(_v in 0u8..1u8) {
        let cred = Cred { euid: 99, egid: 100 };
        prop_assert!(generic_permission(0o640, 42, 100, MAY_READ, &cred));
    }

    /// INV-PERM-6: owner can do everything on 0o700
    #[test]
    fn test_owner_700(_v in 0u8..1u8) {
        let cred = Cred { euid: 1, egid: 1 };
        let mask = MAY_READ | MAY_WRITE | MAY_EXEC;
        prop_assert!(generic_permission(0o700, 1, 2, mask, &cred));
    }

    /// INV-PERM-7: nobody can do anything on 0o000
    #[test]
    fn test_mode_000(mask in 1u32..8u32) {
        let cred = Cred { euid: 1, egid: 1 };
        prop_assert!(!generic_permission(0o000, 1, 1, mask, &cred));
    }

    /// INV-PERM-8: 0o777 allows all for matching category
    #[test]
    fn test_777(
        uid in 0u32..100u32,
        gid in 0u32..100u32,
    ) {
        let cred = Cred { euid: uid, egid: gid };
        let mask = MAY_READ | MAY_WRITE | MAY_EXEC;
        prop_assert!(generic_permission(0o777, uid, gid, mask, &cred));
    }

    /// INV-PERM-9: owner takes priority over group/other
    #[test]
    fn test_owner_priority(
        mode in 0u32..0o777u32,
    ) {
        let cred = Cred { euid: 1, egid: 1 };
        // Owner check uses bits 8-6
        let owner_bits = (mode >> 6) & 0o7;
        let other_bits = mode & 0o7;
        // If owner bits don't have read but other does, owner still denied
        if (owner_bits & MAY_READ as u32) == 0 && (other_bits & MAY_READ as u32) != 0 {
            prop_assert!(!generic_permission(mode as u16, 1, 2, MAY_READ, &cred));
        }
    }

    /// INV-PERM-10: exec on 0o111 allows execute for all
    #[test]
    fn test_exec_111(_v in 0u8..1u8) {
        let cred = Cred { euid: 999, egid: 999 };
        prop_assert!(generic_permission(0o111, 1, 1, MAY_EXEC, &cred));
    }

    /// INV-PERM-11: 0o644 denies execute for all non-owners
    #[test]
    fn test_no_exec_644(_v in 0u8..1u8) {
        let cred = Cred { euid: 99, egid: 99 };
        prop_assert!(!generic_permission(0o644, 1, 1, MAY_EXEC, &cred));
    }

    /// INV-PERM-12: permission bits are in correct positions
    #[test]
    fn test_bit_positions(_v in 0u8..1u8) {
        // Owner perms extracted by >> 6, group by >> 3, other by & 0o7
        prop_assert_eq!((0o400 >> 6) & 0o7, 0o4);  // owner read
        prop_assert_eq!((0o200 >> 6) & 0o7, 0o2);  // owner write
        prop_assert_eq!((0o100 >> 6) & 0o7, 0o1);  // owner exec
        // Group perms
        prop_assert_eq!((0o040 >> 3) & 0o7, 0o4);  // group read
        prop_assert_eq!((0o020 >> 3) & 0o7, 0o2);  // group write
        // Other perms
        prop_assert_eq!(0o004 & 0o7, 0o4);          // other read
        prop_assert_eq!(0o002 & 0o7, 0o2);          // other write
        prop_assert_eq!(0o001 & 0o7, 0o1);          // other exec
    }
}
