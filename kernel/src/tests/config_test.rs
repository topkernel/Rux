//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::config::*;
use super::{test_pass, test_fail, test_group_start};

pub fn test_config() {
    test_group_start("config");

    // Test 1: Kernel identity
    test_assert_eq!(KERNEL_NAME, "Rux", "KERNEL_NAME == Rux");
    test_assert_eq!(KERNEL_VERSION, "0.1.0", "KERNEL_VERSION == 0.1.0");
    test_assert_eq!(TARGET_PLATFORM, "riscv64", "TARGET_PLATFORM == riscv64");

    // Test 2: Page constants
    test_assert_eq!(PAGE_SIZE, 4096, "PAGE_SIZE == 4096");
    test_assert_eq!(PAGE_SHIFT, 12, "PAGE_SHIFT == 12");

    // Test 3: CPU and task limits
    test_assert!(MAX_CPUS >= 1 && MAX_CPUS <= 4, "MAX_CPUS in valid range");
    test_assert_eq!(MAX_TASKS, 256, "MAX_TASKS == 256");

    // Test 4: Memory sizes
    test_assert_eq!(KERNEL_HEAP_SIZE, 33554432, "KERNEL_HEAP_SIZE == 32MB");
    test_assert_eq!(PHYS_MEMORY_SIZE, 2147483648, "PHYS_MEMORY_SIZE == 2GB");
    test_assert_eq!(USER_STACK_SIZE, 8388608, "USER_STACK_SIZE == 8MB");
    test_assert_eq!(KERNEL_STACK_SIZE, 32768, "KERNEL_STACK_SIZE == 32KB");

    // Test 5: Scheduler
    test_assert_eq!(KERNEL_HZ, 100, "KERNEL_HZ == 100");
    test_assert_eq!(DEFAULT_TIME_SLICE_MS, 100, "DEFAULT_TIME_SLICE_MS == 100");

    // Test 6: Memory management
    test_assert_eq!(BUDDY_MAX_ORDER, 20, "BUDDY_MAX_ORDER == 20");
    test_assert_eq!(MAX_PAGE_TABLES, 1024, "MAX_PAGE_TABLES == 1024");

    // Test 7: PID
    test_assert_eq!(PID_MAX_LIMIT, 4194304, "PID_MAX_LIMIT == 4194304");

    // Test 8: Network
    test_assert_eq!(IP_DEFAULT_TTL, 64, "IP_DEFAULT_TTL == 64");
    test_assert_eq!(ETH_MTU, 1500, "ETH_MTU == 1500");

    // Test 9: Feature flags
    test_assert_eq!(ENABLE_SMP, true, "ENABLE_SMP == true");
    test_assert_eq!(ENABLE_NETWORK, true, "ENABLE_NETWORK == true");

    // Test 10: Cache sizes
    test_assert_eq!(ICACHE_SIZE, 256, "ICACHE_SIZE == 256");
    test_assert_eq!(DCACHE_SIZE, 256, "DCACHE_SIZE == 256");

    // Test 11: Pipe
    test_assert_eq!(PIPE_BUFFER_SIZE, 16384, "PIPE_BUFFER_SIZE == 16384");

    // Test 12: Futex
    test_assert_eq!(FUTEX_WAITER_POOL_SIZE, 256, "FUTEX_WAITER_POOL_SIZE == 256");
    test_assert_eq!(FUTEX_HASH_SIZE, 64, "FUTEX_HASH_SIZE == 64");
}
