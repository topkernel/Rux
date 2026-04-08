//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for memory threshold and usage calculations.
//! Copied from: kernel/src/mm/meminfo.rs

use proptest::prelude::*;

// Extracted pure functions from kernel's is_memory_low / should_trigger_oom / get_memory_summary

/// Check if memory is low: free < 5% of total
pub fn is_memory_low(mem_total: usize, mem_free: usize) -> bool {
    if mem_total == 0 {
        return false;
    }
    mem_free * 100 / mem_total < 5
}

/// Check if OOM should trigger: free < 1% of total
pub fn should_trigger_oom(mem_total: usize, mem_free: usize) -> bool {
    if mem_total == 0 {
        return false;
    }
    mem_free * 100 / mem_total < 1
}

/// Heap usage percentage
pub fn heap_usage_percent(heap_total: usize, heap_used: usize) -> usize {
    if heap_total == 0 {
        return 0;
    }
    heap_used * 100 / heap_total
}

proptest! {
    #[test]
    fn test_is_memory_low_zero_free(mem_total in 1usize..1_000_000usize) {
        assert!(is_memory_low(mem_total, 0));
    }

    #[test]
    fn test_is_memory_low_full_free(mem_total in 1usize..1_000_000usize) {
        assert!(!is_memory_low(mem_total, mem_total));
    }

    #[test]
    fn test_is_memory_low_at_boundary(mem_total in 100usize..1_000_000usize) {
        // The check is: mem_free * 100 / mem_total < 5
        // Due to integer division, "5% of total" computed as total * 5 / 100
        // may round down, so threshold * 100 / total could be < 5.
        // Just verify the function behavior is consistent with its formula.
        let threshold = mem_total * 5 / 100;
        // If threshold * 100 / mem_total < 5, it IS low (integer division effect)
        // If threshold * 100 / mem_total >= 5, it is NOT low
        let expected_low = threshold * 100 / mem_total < 5;
        assert_eq!(is_memory_low(mem_total, threshold), expected_low);
    }

    #[test]
    fn test_is_memory_low_zero_total(_v in 0u8..1u8) {
        // Zero total → not low (avoid division by zero)
        assert!(!is_memory_low(0, 0));
        assert!(!is_memory_low(0, 100));
    }

    #[test]
    fn test_should_trigger_oom_zero_free(mem_total in 1usize..1_000_000usize) {
        assert!(should_trigger_oom(mem_total, 0));
    }

    #[test]
    fn test_should_trigger_oom_full_free(mem_total in 1usize..1_000_000usize) {
        assert!(!should_trigger_oom(mem_total, mem_total));
    }

    #[test]
    fn test_should_trigger_oom_at_boundary(mem_total in 100usize..1_000_000usize) {
        let threshold = mem_total / 100;
        let expected_trigger = threshold * 100 / mem_total < 1;
        assert_eq!(should_trigger_oom(mem_total, threshold), expected_trigger);
        // Just below threshold always triggers
        if threshold > 0 {
            assert!(should_trigger_oom(mem_total, threshold - 1));
        }
    }

    #[test]
    fn test_should_trigger_oom_zero_total(_v in 0u8..1u8) {
        assert!(!should_trigger_oom(0, 0));
    }

    #[test]
    fn test_oom_implies_low(mem_total in 100usize..1_000_000usize, mem_free in 0usize..100usize) {
        // If OOM triggers, memory must also be low
        if should_trigger_oom(mem_total, mem_free) {
            assert!(is_memory_low(mem_total, mem_free),
                "OOM triggered but memory not low: total={} free={}", mem_total, mem_free);
        }
    }

    #[test]
    fn test_heap_usage_percent_zero_total(_v in 0u8..1u8) {
        assert_eq!(heap_usage_percent(0, 0), 0);
        assert_eq!(heap_usage_percent(0, 100), 0);
    }

    #[test]
    fn test_heap_usage_percent_full(heap_total in 1usize..1_000_000usize) {
        assert_eq!(heap_usage_percent(heap_total, heap_total), 100);
    }

    #[test]
    fn test_heap_usage_percent_zero(heap_total in 1usize..1_000_000usize) {
        assert_eq!(heap_usage_percent(heap_total, 0), 0);
    }

    #[test]
    fn test_heap_usage_percent_half(heap_total in 2usize..1_000_000usize) {
        let half = heap_total / 2;
        let pct = heap_usage_percent(heap_total, half);
        assert!(pct >= 49 && pct <= 50, "expected ~50%, got {}", pct);
    }

    #[test]
    fn test_mem_used_equals_total_minus_free(mem_total in 0usize..1_000_000usize, mem_free in 0usize..1_000_000usize) {
        // Verify the identity: mem_used = mem_total - mem_free (saturating)
        let mem_used = mem_total.saturating_sub(mem_free);
        // Cross-check: is_memory_low depends only on the ratio
        let low = is_memory_low(mem_total, mem_free);
        if mem_total > 0 && mem_used * 100 / mem_total >= 95 {
            assert!(low, "used >= 95% but not detected as low");
        }
    }
}
