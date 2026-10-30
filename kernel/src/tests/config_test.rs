//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use crate::config::*;
use super::{test_pass, test_fail, test_group_start};

pub fn test_config() {
    test_group_start("config");

    // Test 1: Kernel identity. TARGET_PLATFORM follows the active build
    // platform (build.rs: cargo feature must match the config), so the
    // pin is per-arch — the suite runs on every supported arch, not
    // just riscv64.
    test_assert_eq!(KERNEL_NAME, "Rux", "KERNEL_NAME == Rux");
    test_assert_eq!(KERNEL_VERSION, "0.1.0", "KERNEL_VERSION == 0.1.0");
    #[cfg(feature = "riscv64")]
    test_assert_eq!(TARGET_PLATFORM, "riscv64", "TARGET_PLATFORM == riscv64");
    #[cfg(feature = "aarch64")]
    test_assert_eq!(TARGET_PLATFORM, "aarch64", "TARGET_PLATFORM == aarch64");
    #[cfg(feature = "x86_64")]
    test_assert_eq!(TARGET_PLATFORM, "x86_64", "TARGET_PLATFORM == x86_64");

    // Test 2: Page constants
    test_assert_eq!(PAGE_SIZE, 4096, "PAGE_SIZE == 4096");
    test_assert_eq!(PAGE_SHIFT, 12, "PAGE_SHIFT == 12");

    // Test 3: CPU and task limits
    test_assert!(MAX_CPUS >= 1 && MAX_CPUS <= 4, "MAX_CPUS in valid range");
    test_assert_eq!(MAX_TASKS, 256, "MAX_TASKS == 256");

    // Test 4: Memory sizes
    // Kernel heap grew 32MB -> 128MB -> 512MB -> 1024MB (distro-scale
    // userspace, then the OH/GNOME service set, then the OH sandbox
    // workload); kernel stacks doubled again (64KB -> 128KB) after the
    // r19 stack fix. build.rs clamps kernel_heap_size to 128MB on
    // riscv64 (Kernel.toml's 1024MB is the x86_64 value; a 1GB heap
    // overruns the riscv64 memory map — merge gate 2026-10-19); aarch64
    // joins the clamp, x86_64 runs the raw Kernel.toml value.
    #[cfg(not(feature = "x86_64"))]
    test_assert_eq!(KERNEL_HEAP_SIZE, 134217728, "KERNEL_HEAP_SIZE == 128MB (clamped)");
    #[cfg(feature = "x86_64")]
    test_assert_eq!(KERNEL_HEAP_SIZE, 1073741824, "KERNEL_HEAP_SIZE == 1024MB (unclamped)");
    test_assert_eq!(PHYS_MEMORY_SIZE, 2147483648, "PHYS_MEMORY_SIZE == 2GB");
    test_assert_eq!(USER_STACK_SIZE, 8388608, "USER_STACK_SIZE == 8MB");
    // kernel_stack_size is arch-gated since leg-17: 128KB base for
    // riscv64/unmeasured arches (fee7774f: the 80KB slot overflowed on
    // the riscv64 test kernel), 80KB packed-slab slots on x86_64.
    #[cfg(target_arch = "x86_64")]
    test_assert_eq!(KERNEL_STACK_SIZE, 81920, "KERNEL_STACK_SIZE == 80KB (x86_64 packed slab)");
    #[cfg(not(target_arch = "x86_64"))]
    test_assert_eq!(KERNEL_STACK_SIZE, 131072, "KERNEL_STACK_SIZE == 128KB");

    // Test 5: Scheduler
    test_assert_eq!(KERNEL_HZ, 100, "KERNEL_HZ == 100");
    test_assert_eq!(DEFAULT_TIME_SLICE_MS, 100, "DEFAULT_TIME_SLICE_MS == 100");

    // Test 6: Memory management
    test_assert_eq!(BUDDY_MAX_ORDER, 20, "BUDDY_MAX_ORDER == 20");
    test_assert_eq!(MAX_PAGE_TABLES, 1024, "MAX_PAGE_TABLES == 1024");

    // Test 7: PID
    test_assert_eq!(PID_MAX_LIMIT, 4194304, "PID_MAX_LIMIT == 4194304");
    test_assert_eq!(PID_MAX_DEFAULT, 32768, "PID_MAX_DEFAULT == 32768");
    test_assert_eq!(RESERVED_PIDS, 300, "RESERVED_PIDS == 300");

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
    test_assert_eq!(FUTEX_WAITER_POOL_SIZE, 8192, "FUTEX_WAITER_POOL_SIZE == 8192");
    test_assert_eq!(FUTEX_HASH_SIZE, 512, "FUTEX_HASH_SIZE == 512");
}
