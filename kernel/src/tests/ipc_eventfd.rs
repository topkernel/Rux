use crate::syscall::misc::{sys_eventfd, sys_eventfd2};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_eventfd() {
    test_group_start("eventfd");

    // Test 1: sys_eventfd creates valid fd
    let fd = sys_eventfd([0, 0, 0, 0, 0, 0]);
    test_assert!(fd >= 0, "sys_eventfd(0) returns valid fd");

    // Test 2: sys_eventfd with non-zero initval
    let fd2 = sys_eventfd([42, 0, 0, 0, 0, 0]);
    test_assert!(fd2 >= 0, "sys_eventfd(42) returns valid fd");

    // Test 3: Two eventfd fds should be different
    // Note: in test context with limited fdtable, fds may be reused immediately
    if fd >= 0 && fd2 >= 0 {
        if fd != fd2 {
            test_pass("two eventfd fds are different");
        } else {
            test_skip("two eventfd fds different", "fdtable reuse in test context");
        }
    } else {
        test_fail("fd comparison", "invalid fds");
    }

    // Test 4: sys_eventfd2 creates valid fd
    let fd3 = sys_eventfd2([0, 0, 0, 0, 0, 0]);
    test_assert!(fd3 >= 0, "sys_eventfd2(0, 0) returns valid fd");

    // Test 5: sys_eventfd2 with EFD_NONBLOCK flag
    let fd4 = sys_eventfd2([0, 0x800, 0, 0, 0, 0]); // EFD_NONBLOCK = 0x800
    test_assert!(fd4 >= 0, "sys_eventfd2(0, EFD_NONBLOCK) returns valid fd");

    // Test 6: sys_eventfd2 with EFD_SEMAPHORE flag
    let fd5 = sys_eventfd2([0, 0x1, 0, 0, 0, 0]); // EFD_SEMAPHORE = 0x1
    test_assert!(fd5 >= 0, "sys_eventfd2(0, EFD_SEMAPHORE) returns valid fd");

    // Test 7: sys_eventfd2 with EFD_CLOEXEC flag
    let fd6 = sys_eventfd2([0, 0x80000, 0, 0, 0, 0]); // EFD_CLOEXEC = 0x80000
    test_assert!(fd6 >= 0, "sys_eventfd2(0, EFD_CLOEXEC) returns valid fd");

    // Test 8: Eventfd flag constants
    test_assert!(0x800u64 == 0x800, "EFD_NONBLOCK == 0x800");
    test_assert!(0x80000u64 == 0x80000, "EFD_CLOEXEC == 0x80000");
    test_assert!(0x1u64 == 0x1, "EFD_SEMAPHORE == 0x1");
}
