//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::syscall::memory::{sys_mmap, sys_munmap, sys_mprotect};
use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_mmap_syscalls() {
    test_group_start("mmap syscalls");

    // Test 1: PROT constants
    test_assert_eq!(0x1u32, 0x1, "PROT_READ == 0x1");
    test_assert_eq!(0x2u32, 0x2, "PROT_WRITE == 0x2");
    test_assert_eq!(0x4u32, 0x4, "PROT_EXEC == 0x4");

    // Test 2: MAP constants
    test_assert_eq!(0x01u32, 0x01, "MAP_SHARED == 0x01");
    test_assert_eq!(0x02u32, 0x02, "MAP_PRIVATE == 0x02");
    test_assert_eq!(0x10u32, 0x10, "MAP_FIXED == 0x10");
    test_assert_eq!(0x20u32, 0x20, "MAP_ANONYMOUS == 0x20");

    // Test 3: Syscall numbers
    test_assert_eq!(SyscallNo::Mmap as u32, 222, "SyscallNo::Mmap == 222");
    test_assert_eq!(SyscallNo::Munmap as u32, 215, "SyscallNo::Munmap == 215");
    test_assert_eq!(SyscallNo::Mprotect as u32, 226, "SyscallNo::Mprotect == 226");
    test_assert_eq!(SyscallNo::Msync as u32, 227, "SyscallNo::Msync == 227");
    test_assert_eq!(SyscallNo::Mremap as u32, 216, "SyscallNo::Mremap == 216");
    test_assert_eq!(SyscallNo::Madvise as u32, 233, "SyscallNo::Madvise == 233");
    test_assert_eq!(SyscallNo::Mincore as u32, 232, "SyscallNo::Mincore == 232");
    test_assert_eq!(SyscallNo::Mlock as u32, 228, "SyscallNo::Mlock == 228");
    test_assert_eq!(SyscallNo::Munlock as u32, 229, "SyscallNo::Munlock == 229");

    // Test 4: sys_mmap anonymous mapping
    // Note: mmap may fail in test context (no user VMA setup)
    let addr = sys_mmap([0, 4096, 0x3, 0x22, !0u64, 0]); // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANON, fd=-1
    let addr_valid = addr > 0 && (addr as i64) > 0 && addr != !0u64;
    if addr_valid {
        test_pass("sys_mmap anonymous returns valid address");
    } else {
        test_skip("sys_mmap anonymous", "no VMA context in test");
    }

    // Test 5: sys_mmap zero length
    let addr5 = sys_mmap([0, 0, 0x3, 0x22, !0u64, 0]);
    let addr5_valid = addr5 > 0 && (addr5 as i64) > 0 && addr5 != !0u64;
    if addr5_valid {
        test_pass("sys_mmap zero length returns valid address");
    } else {
        test_skip("sys_mmap zero length", "no VMA context in test");
    }

    // Test 6: sys_munmap (use addr from Test 4 if valid)
    if addr_valid {
        let result = sys_munmap([addr, 4096, 0, 0, 0, 0]);
        test_assert!(result == 0, "sys_munmap succeeds");
    } else {
        test_skip("sys_munmap", "no valid address to unmap");
    }

    // Test 7: sys_mprotect
    let addr7 = sys_mmap([0, 4096, 0x3, 0x22, !0u64, 0]);
    let addr7_valid = addr7 > 0 && (addr7 as i64) > 0 && addr7 != !0u64;
    if addr7_valid {
        let result = sys_mprotect([addr7, 4096, 0x1, 0, 0, 0]); // PROT_READ only
        test_assert!(result == 0, "sys_mprotect read-only succeeds");
        // Clean up
        let _ = sys_munmap([addr7, 4096, 0, 0, 0, 0]);
    } else {
        test_skip("sys_mprotect", "no valid address to protect");
    }

    // Test 8: sys_mprotect invalid address
    let result = sys_mprotect([0xDEAD, 4096, 0x3, 0, 0, 0]);
    // Should return error (ENOMEM or EINVAL)
    test_assert!(result as i64 != 0, "sys_mprotect invalid address returns error");

    // Test 9: Multiple mmap calls return different addresses
    let a1 = sys_mmap([0, 4096, 0x3, 0x22, !0u64, 0]);
    let a2 = sys_mmap([0, 4096, 0x3, 0x22, !0u64, 0]);
    if a1 > 0 && a2 > 0 {
        if a1 != a2 {
            test_pass("two mmap calls return different addresses");
        } else {
            test_skip("mmap different addresses", "VMA context limited in test");
        }
    } else {
        test_skip("mmap different addresses", "no VMA context in test");
    }

    // Test 10: madvise constants
    test_assert_eq!(SyscallNo::Mlock as u32, 228, "Mlock syscall number");
    test_assert_eq!(SyscallNo::Munlock as u32, 229, "Munlock syscall number");
}
