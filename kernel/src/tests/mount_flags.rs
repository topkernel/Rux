//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::fs::mount::MntFlags;
use super::{test_pass, test_fail, test_group_start};

pub fn test_mount_flags() {
    test_group_start("mount_flags");

    // Test 1: MNT_READONLY
    let ro = MntFlags::new(0x01);
    test_assert!(ro.is_readonly(), "MNT_READONLY is_readonly");

    // Test 2: MNT_NOATIME
    let noatime = MntFlags::new(0x02);
    test_assert!(!noatime.is_readonly(), "MNT_NOATIME not readonly");

    // Test 3: MNT_NOEXEC
    let noexec = MntFlags::new(0x10);
    test_assert!(noexec.is_noexec(), "MNT_NOEXEC is_noexec");

    // Test 4: MNT_NOSUID
    let nosuid = MntFlags::new(0x20);
    test_assert!(nosuid.is_nosuid(), "MNT_NOSUID is_nosuid");

    // Test 5: Zero flags
    let none = MntFlags::new(0);
    test_assert!(!none.is_readonly(), "zero not readonly");
    test_assert!(!none.is_noexec(), "zero not noexec");
    test_assert!(!none.is_nosuid(), "zero not nosuid");

    // Test 6: bits() returns raw value
    let flags = MntFlags::new(0x01 | 0x10);
    test_assert_eq!(flags.bits(), 0x11, "bits() returns raw combined value");

    // Test 7: Combined flags
    let combined = MntFlags::new(0x01 | 0x10);
    test_assert!(combined.is_readonly() && combined.is_noexec(), "combined readonly && noexec");
    test_assert!(!combined.is_nosuid(), "combined not nosuid");

    // Test 8: All mount flag constants
    test_assert_eq!(MntFlags::new(0x01).bits(), 0x01, "MNT_READONLY value");
    test_assert_eq!(MntFlags::new(0x02).bits(), 0x02, "MNT_NOATIME value");
    test_assert_eq!(MntFlags::new(0x04).bits(), 0x04, "MNT_NODIRATIME value");
    test_assert_eq!(MntFlags::new(0x08).bits(), 0x08, "MNT_SYNCHRONOUS value");
    test_assert_eq!(MntFlags::new(0x10).bits(), 0x10, "MNT_NOEXEC value");
    test_assert_eq!(MntFlags::new(0x20).bits(), 0x20, "MNT_NOSUID value");
    test_assert_eq!(MntFlags::new(0x40).bits(), 0x40, "MNT_NODEV value");
    test_assert_eq!(MntFlags::new(0x80).bits(), 0x80, "MNT_PRIVATE value");
    test_assert_eq!(MntFlags::new(0x100).bits(), 0x100, "MNT_SHARED value");
    test_assert_eq!(MntFlags::new(0x800).bits(), 0x800, "MNT_FORCE value");
}
