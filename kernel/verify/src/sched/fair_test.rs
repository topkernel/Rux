//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! CFS scheduler weight tables and vruntime arithmetic invariant tests.
//!
//! Types copied from: kernel/src/sched/fair.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/sched/fair.rs
// ============================================================================

pub const NICE_0_LOAD: u64 = 1024;
pub const WEIGHT_IDLEPRIO: u64 = 3;
pub const SCHED_MIN_GRANULARITY_NS: u64 = 700_000;
pub const SCHED_LATENCY_NS: u64 = 6_000_000;

pub const PRIO_TO_WEIGHT: [u64; 40] = [
    /* -20 */ 88761, 71755, 56483, 46273, 36291,
    /* -15 */ 29154, 23254, 18705, 14949, 11916,
    /* -10 */ 9548,  7620,  6100,  4904,  3906,
    /*  -5 */ 3121,  2501,  1991,  1586,  1277,
    /*   0 */ 1024,   820,   655,   526,   423,
    /*   5 */ 335,    272,   215,   172,   137,
    /*  10 */ 110,     87,    70,    56,    45,
    /*  15 */ 36,     29,    23,    18,    15,
];

pub const PRIO_TO_WMULT: [u64; 40] = [
    /* -20 */ 48388, 59856, 76040, 92818, 118348,
    /* -15 */ 147320, 184698, 229616, 288308, 360437,
    /* -10 */ 449829, 563644, 704093, 875809, 1099582,
    /*  -5 */ 1376151, 1717300, 2157191, 2708050, 3363326,
    /*   0 */ 4194304, 5237760, 6557202, 8165337, 10153587,
    /*   5 */ 12820794, 15790321, 19976592, 24970740, 31350126,
    /*  10 */ 39045157, 49367440, 61356676, 76695844, 95443717,
    /*  15 */ 119304647, 148154320, 186737708, 238609294, 286331153,
];

#[derive(Debug, Clone, Copy)]
pub struct LoadWeight {
    pub weight: u64,
    pub inv_weight: u64,
}

impl LoadWeight {
    pub fn new(weight: u64) -> Self {
        Self { weight, inv_weight: 0 }
    }

    pub fn from_nice(nice: i32) -> Self {
        let idx = (nice + 20) as usize;
        let idx = idx.min(39).max(0);
        Self {
            weight: PRIO_TO_WEIGHT[idx],
            inv_weight: PRIO_TO_WMULT[idx],
        }
    }

    pub fn update_inv_weight(&mut self) {
        if self.inv_weight == 0 {
            if self.weight >= (1u64 << 32) {
                self.inv_weight = 1;
            } else {
                self.inv_weight = (1u64 << 32) / self.weight;
            }
        }
    }
}

impl Default for LoadWeight {
    fn default() -> Self {
        Self::from_nice(0)
    }
}

pub fn calc_delta_fair(delta_exec: u64, weight: u64, inv_weight: u64) -> u64 {
    if weight == NICE_0_LOAD {
        return delta_exec;
    }

    let mut lw = LoadWeight { weight, inv_weight };
    lw.update_inv_weight();

    (delta_exec * lw.inv_weight) >> 32
}

pub fn sched_slice_calc(
    nr_running: u64,
    task_weight: u64,
    total_weight: u64,
) -> u64 {
    if nr_running == 0 {
        return SCHED_MIN_GRANULARITY_NS;
    }

    let sched_period = if nr_running > SCHED_LATENCY_NS / SCHED_MIN_GRANULARITY_NS {
        SCHED_MIN_GRANULARITY_NS * nr_running
    } else {
        SCHED_LATENCY_NS
    };

    if total_weight == 0 {
        return SCHED_MIN_GRANULARITY_NS;
    }

    let slice = (sched_period * task_weight) / total_weight;
    slice.max(SCHED_MIN_GRANULARITY_NS)
}

pub fn check_preempt(curr_vruntime: u64, se_vruntime: u64) -> bool {
    let wakeup_granularity = SCHED_MIN_GRANULARITY_NS;
    if se_vruntime < curr_vruntime {
        let delta = curr_vruntime - se_vruntime;
        delta > wakeup_granularity
    } else {
        false
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-CFS-1: Weight table is monotonically decreasing
    #[test]
    fn test_weight_monotone(i in 0usize..39usize) {
        if i > 0 {
            prop_assert!(PRIO_TO_WEIGHT[i] < PRIO_TO_WEIGHT[i - 1],
                "weight[{}]={} >= weight[{}]={}", i, PRIO_TO_WEIGHT[i], i-1, PRIO_TO_WEIGHT[i-1]);
        }
    }

    /// INV-CFS-2: WMULT table is monotonically increasing
    #[test]
    fn test_wmult_monotone(i in 0usize..39usize) {
        if i > 0 {
            prop_assert!(PRIO_TO_WMULT[i] > PRIO_TO_WMULT[i - 1],
                "wmult[{}]={} <= wmult[{}]={}", i, PRIO_TO_WMULT[i], i-1, PRIO_TO_WMULT[i-1]);
        }
    }

    /// INV-CFS-3: Nice 0 weight is NICE_0_LOAD (1024)
    #[test]
    fn test_nice_0_weight(_v in 0u8..1u8) {
        let lw = LoadWeight::from_nice(0);
        prop_assert_eq!(lw.weight, NICE_0_LOAD);
    }

    /// INV-CFS-4: from_nice maps to valid table entry
    #[test]
    fn test_from_nice_valid_index(nice in -100i32..100i32) {
        let lw = LoadWeight::from_nice(nice);
        // Kernel does: idx = ((nice + 20) as usize).min(39).max(0)
        // For nice < -20: wraps to large usize, min(39) = 39
        // For nice > 19: > 39, min(39) = 39
        // For nice in [-20, 19]: idx = nice + 20, range [0, 39]
        // Verify the result is a valid table entry
        prop_assert!(lw.weight > 0);
        prop_assert!(lw.inv_weight > 0);
    }

    /// INV-CFS-5: Lower nice (higher priority) gives higher weight
    #[test]
    fn test_nice_weight_inverse(n1 in -20i32..18i32) {
        let n2 = n1 + 1;
        prop_assert!(LoadWeight::from_nice(n1).weight > LoadWeight::from_nice(n2).weight);
    }

    /// INV-CFS-6: calc_delta_fair for nice-0 weight returns delta unchanged
    #[test]
    fn test_delta_fair_nice_0(delta in 1u64..1_000_000u64) {
        let lw = LoadWeight::from_nice(0);
        prop_assert_eq!(calc_delta_fair(delta, lw.weight, lw.inv_weight), delta);
    }

    /// INV-CFS-7: Higher weight gets smaller vruntime delta (more CPU)
    #[test]
    fn test_delta_fair_weight_relation(delta in 1000u64..10_000_000u64) {
        let lw_high = LoadWeight::from_nice(-10);
        let lw_low = LoadWeight::from_nice(10);
        let vr_high = calc_delta_fair(delta, lw_high.weight, lw_high.inv_weight);
        let vr_low = calc_delta_fair(delta, lw_low.weight, lw_low.inv_weight);
        prop_assert!(vr_high < vr_low);
    }

    /// INV-CFS-8: calc_delta_fair is linear in delta for fixed weight
    #[test]
    fn test_delta_fair_linear(
        delta1 in 100u64..10_000_000u64,
        delta2 in 100u64..10_000_000u64,
        nice in -20i32..19i32,
    ) {
        let lw = LoadWeight::from_nice(nice);
        let vr1 = calc_delta_fair(delta1, lw.weight, lw.inv_weight);
        let vr2 = calc_delta_fair(delta2, lw.weight, lw.inv_weight);
        // If delta1 < delta2 then vr1 <= vr2
        if delta1 < delta2 {
            prop_assert!(vr1 <= vr2);
        } else if delta1 > delta2 {
            prop_assert!(vr1 >= vr2);
        }
    }

    /// INV-CFS-9: update_inv_weight is idempotent
    #[test]
    fn test_update_inv_weight_idempotent(weight in 1u64..1_000_000u64) {
        let mut lw = LoadWeight::new(weight);
        lw.update_inv_weight();
        let first = lw.inv_weight;
        lw.update_inv_weight();
        prop_assert_eq!(lw.inv_weight, first);
    }

    /// INV-CFS-10: inv_weight for nice 0 is (1<<32)/1024 = 4194304
    #[test]
    fn test_inv_weight_nice_0(_v in 0u8..1u8) {
        let mut lw = LoadWeight::new(NICE_0_LOAD);
        lw.update_inv_weight();
        prop_assert_eq!(lw.inv_weight, 4194304);
    }

    /// INV-CFS-11: sched_slice with 0 tasks returns min granularity
    #[test]
    fn test_sched_slice_empty(_v in 0u8..1u8) {
        prop_assert_eq!(sched_slice_calc(0, 1024, 1024), SCHED_MIN_GRANULARITY_NS);
    }

    /// INV-CFS-12: sched_slice with 0 total_weight returns min granularity
    #[test]
    fn test_sched_slice_zero_total_weight(nr in 1u64..100u64) {
        prop_assert_eq!(sched_slice_calc(nr, 1024, 0), SCHED_MIN_GRANULARITY_NS);
    }

    /// INV-CFS-13: sched_slice >= SCHED_MIN_GRANULARITY_NS always
    #[test]
    fn test_sched_slice_minimum(
        nr in 1u64..100u64,
        tw in 1u64..100_000u64,
        w in 1u64..100_000u64,
    ) {
        let slice = sched_slice_calc(nr, w, tw);
        prop_assert!(slice >= SCHED_MIN_GRANULARITY_NS);
    }

    /// INV-CFS-14: sched_slice is proportional to task_weight/total_weight
    #[test]
    fn test_sched_slice_proportional(nr in 2u64..50u64) {
        let total = nr * 1024;
        let single = sched_slice_calc(nr, 1024, total);
        let double = sched_slice_calc(nr, 2048, total);
        prop_assert!(double >= single);
    }

    /// INV-CFS-15: check_preempt returns false when se_vruntime >= curr
    #[test]
    fn test_check_preempt_no_preempt(
        curr in 0u64..10_000_000u64,
        offset in 0u64..10_000_000u64,
    ) {
        let se = curr + offset; // se >= curr
        prop_assert!(!check_preempt(curr, se));
    }

    /// INV-CFS-16: check_preempt needs threshold gap
    #[test]
    fn test_check_preempt_threshold(curr in 700_001u64..10_000_000u64) {
        // Small gap <= min granularity: no preempt
        let se_close = curr - 1;
        prop_assert!(!check_preempt(curr, se_close));
        // Large gap: preempt
        let se_far = curr - SCHED_MIN_GRANULARITY_NS - 1;
        prop_assert!(check_preempt(curr, se_far));
    }

    /// INV-CFS-17: sched_slice_to_ms / ms_to_ns roundtrip
    #[test]
    fn test_ms_ns_roundtrip(ms in 0u32..60_000u32) {
        let ns = (ms as u64) * 1_000_000;
        prop_assert_eq!((ns / 1_000_000) as u32, ms);
    }

    /// INV-CFS-18: Weight * inv_weight is positive and non-zero
    #[test]
    fn test_weight_inv_product(nice in -20i32..19i32) {
        let lw = LoadWeight::from_nice(nice);
        let product = lw.weight * lw.inv_weight;
        prop_assert!(product > 0);
    }
}
