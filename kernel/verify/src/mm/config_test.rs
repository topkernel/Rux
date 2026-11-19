//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for kernel configuration constant relationships.
//! Copied from: kernel/src/config.rs (auto-generated from Kernel.toml)

use proptest::prelude::*;

// Copied constants from config.rs
pub const KERNEL_HEAP_SIZE: usize = 33554432;   // 32MB
pub const PHYS_MEMORY_SIZE: usize = 2147483648;  // 2GB
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;
pub const USER_STACK_SIZE: usize = 8388608;      // 8MB
pub const USER_STACK_MAX_SIZE: usize = 8388608;  // 8MB
pub const USER_HEAP_MAX_SIZE: usize = 134217728; // 128MB
pub const KERNEL_STACK_SIZE: usize = 32768;      // 32KB
pub const MAX_PAGE_TABLES: usize = 1024;
pub const BUDDY_MAX_ORDER: usize = 20;
pub const SLAB_NUM_CACHES: usize = 10;
pub const PCP_HIGH: usize = 64;
pub const PCP_LOW: usize = 16;
pub const PCP_BATCH: usize = 16;
pub const PIPE_BUFFER_SIZE: usize = 16384;       // 16KB
pub const ICACHE_SIZE: usize = 256;
pub const DCACHE_SIZE: usize = 256;
pub const MAX_SYMLINKS: usize = 40;
pub const EXT4_MAX_SYMLINK_DEPTH: usize = 8;
pub const PID_MAX_LIMIT: usize = 4194304;        // 4M
pub const PID_MAX_DEFAULT: usize = 32768;        // 32K
pub const RESERVED_PIDS: usize = 300;
pub const MAX_CMDLINE_LEN: usize = 2048;
pub const FD_SETSIZE: usize = 1024;
pub const MAX_CPUS: usize = 4;
pub const MAX_TASKS: usize = 256;
pub const ETH_MTU: usize = 1500;
pub const IP_DEFAULT_TTL: u8 = 64;
pub const TCP_SOCKET_TABLE_SIZE: usize = 64;
pub const FUTEX_WAITER_POOL_SIZE: usize = 256;
pub const FUTEX_HASH_SIZE: usize = 64;
pub const TCP_RTO_MIN_US: u64 = 200000;
pub const TCP_RTO_MAX_US: u64 = 120000000;
pub const TCP_RTO_DEFAULT_US: u64 = 1000000;
pub const DEFAULT_TIME_SLICE_MS: u32 = 100;
pub const KERNEL_HZ: u32 = 100;
pub const TIMER_CLOCK_FREQ_HZ: u64 = 10000000;
pub const CFS_MIN_GRANULARITY_NS: u64 = 700000;
pub const CFS_LATENCY_NS: u64 = 6000000;
pub const PLIC_MAX_INTERRUPTS: usize = 128;
pub const PRINTK_RING_BUFFER_SIZE: usize = 1048576; // 1MB

proptest! {
    #[test]
    fn test_page_size_is_power_of_two(_v in 0u8..1u8) {
        assert!(PAGE_SIZE > 0 && (PAGE_SIZE & (PAGE_SIZE - 1)) == 0);
        assert_eq!(PAGE_SIZE, 1 << PAGE_SHIFT);
    }

    #[test]
    fn test_pcp_watermark_ordering(_v in 0u8..1u8) {
        assert!(PCP_LOW <= PCP_HIGH, "PCP_LOW must be <= PCP_HIGH");
        assert!(PCP_BATCH <= PCP_HIGH, "PCP_BATCH must be <= PCP_HIGH");
    }

    #[test]
    fn test_pcp_batch_divides_watermark(_v in 0u8..1u8) {
        // PCP_BATCH should divide (PCP_HIGH - PCP_LOW) evenly
        let range = PCP_HIGH - PCP_LOW;
        assert_eq!(range % PCP_BATCH, 0, "PCP batch should evenly divide watermark range");
    }

    #[test]
    fn test_heap_within_phys_memory(_v in 0u8..1u8) {
        assert!(KERNEL_HEAP_SIZE < PHYS_MEMORY_SIZE, "Kernel heap must fit in physical memory");
        assert!(USER_STACK_MAX_SIZE < PHYS_MEMORY_SIZE);
        assert!(USER_HEAP_MAX_SIZE < PHYS_MEMORY_SIZE);
    }

    #[test]
    fn test_pid_hierarchy(_v in 0u8..1u8) {
        assert!(RESERVED_PIDS < PID_MAX_DEFAULT, "Reserved PIDs must fit in default range");
        assert!(PID_MAX_DEFAULT <= PID_MAX_LIMIT, "Default PID max must be <= limit");
    }

    #[test]
    fn test_pid_max_is_power_of_two(_v in 0u8..1u8) {
        assert!(PID_MAX_LIMIT > 0 && (PID_MAX_LIMIT & (PID_MAX_LIMIT - 1)) == 0,
                "PID_MAX_LIMIT should be a power of two");
    }

    #[test]
    fn test_symlink_depth_nesting(_v in 0u8..1u8) {
        assert!(EXT4_MAX_SYMLINK_DEPTH <= MAX_SYMLINKS,
                "ext4 max symlink depth must not exceed global max");
    }

    #[test]
    fn test_tcp_rto_ordering(_v in 0u8..1u8) {
        assert!(TCP_RTO_MIN_US <= TCP_RTO_DEFAULT_US, "RTO min <= default");
        assert!(TCP_RTO_DEFAULT_US <= TCP_RTO_MAX_US, "RTO default <= max");
    }

    #[test]
    fn test_cfs_latency_vs_granularity(_v in 0u8..1u8) {
        assert!(CFS_MIN_GRANULARITY_NS <= CFS_LATENCY_NS,
                "CFS min granularity must be <= scheduling latency");
    }

    #[test]
    fn test_kernel_hz_positive(_v in 0u8..1u8) {
        assert!(KERNEL_HZ > 0);
        assert_eq!(1000 / KERNEL_HZ * KERNEL_HZ, 1000,
                   "KERNEL_HZ should divide 1000 evenly");
    }

    #[test]
    fn test_stack_sizes_page_aligned(_v in 0u8..1u8) {
        assert_eq!(USER_STACK_SIZE % PAGE_SIZE, 0, "User stack must be page-aligned");
        assert_eq!(KERNEL_STACK_SIZE % PAGE_SIZE, 0, "Kernel stack must be page-aligned");
    }

    #[test]
    fn test_cache_sizes_power_of_two(_v in 0u8..1u8) {
        for (name, size) in [
            ("ICACHE_SIZE", ICACHE_SIZE),
            ("DCACHE_SIZE", DCACHE_SIZE),
            ("FUTEX_HASH_SIZE", FUTEX_HASH_SIZE),
            ("TCP_SOCKET_TABLE_SIZE", TCP_SOCKET_TABLE_SIZE),
            ("MAX_TASKS", MAX_TASKS),
        ] {
            assert!(size > 0 && (size & (size - 1)) == 0,
                    "{} must be a power of two", name);
        }
    }

    #[test]
    fn test_pipe_buffer_power_of_two(_v in 0u8..1u8) {
        assert!(PIPE_BUFFER_SIZE > 0 && (PIPE_BUFFER_SIZE & (PIPE_BUFFER_SIZE - 1)) == 0,
                "PIPE_BUFFER_SIZE must be a power of two");
    }

    #[test]
    fn test_fd_setsize_positive(_v in 0u8..1u8) {
        assert!(FD_SETSIZE > 0);
        assert!(FD_SETSIZE >= MAX_TASKS, "FD_SETSIZE should accommodate max tasks");
    }

    #[test]
    fn test_printk_ring_buffer_size(_v in 0u8..1u8) {
        assert!(PRINTK_RING_BUFFER_SIZE > 0);
        assert_eq!(PRINTK_RING_BUFFER_SIZE % PAGE_SIZE, 0,
                   "Printk ring buffer should be page-aligned");
    }

    #[test]
    fn test_max_cpus_reasonable(_v in 0u8..1u8) {
        assert!(MAX_CPUS > 0 && MAX_CPUS <= 1024);
        assert!(PLIC_MAX_INTERRUPTS > 0);
    }
}
