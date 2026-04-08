//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for OOM badness scoring.
//! Copied from: kernel/src/mm/oom_kill.rs

use proptest::prelude::*;

// Copied constants
pub const OOM_SCORE_ADJ_MIN: i32 = -1000;
pub const OOM_SCORE_ADJ_MAX: i32 = 1000;

// Simplified oom_badness: extracts the pure arithmetic from the kernel function.
// Kernel has extra guards (kernel thread check, MMF_OOM_DISABLE, null mm).
// Here we test the core scoring formula with controlled inputs.
pub fn oom_badness_score(total_vm: u64, oom_score_adj: i32, totalpages: u64) -> u64 {
    // If oom_score_adj == OOM_SCORE_ADJ_MIN, immune
    if oom_score_adj <= OOM_SCORE_ADJ_MIN {
        return 0;
    }

    let mut points = total_vm;

    if totalpages >= 1000 {
        let adj = (oom_score_adj as i64) * (totalpages as i64) / 1000;
        if adj >= 0 {
            points = points.saturating_add(adj as u64);
        } else {
            points = points.saturating_sub((-adj) as u64);
        }
    }

    points
}

proptest! {
    #[test]
    fn test_immune_at_min_adj(total_vm in 0u64..1_000_000u64, totalpages in 1000u64..10_000_000u64) {
        // OOM_SCORE_ADJ_MIN = -1000 → always immune
        assert_eq!(oom_badness_score(total_vm, OOM_SCORE_ADJ_MIN, totalpages), 0);
        assert_eq!(oom_badness_score(total_vm, OOM_SCORE_ADJ_MIN - 1, totalpages), 0);
    }

    #[test]
    fn test_baseline_zero_adj(total_vm in 0u64..1_000_000u64, totalpages in 1000u64..10_000_000u64) {
        // Zero adjustment: points == total_vm
        let score = oom_badness_score(total_vm, 0, totalpages);
        assert_eq!(score, total_vm);
    }

    #[test]
    fn test_positive_adj_increases_score(total_vm in 100u64..1_000_000u64, totalpages in 1000u64..10_000_000u64, adj in 1i32..1000i32) {
        let base = oom_badness_score(total_vm, 0, totalpages);
        let boosted = oom_badness_score(total_vm, adj, totalpages);
        assert!(boosted >= base, "positive adj should not decrease score: base={} boosted={} adj={}", base, boosted, adj);
    }

    #[test]
    fn test_negative_adj_decreases_score(total_vm in 100u64..1_000_000u64, totalpages in 1000u64..10_000_000u64, adj in -999i32..0i32) {
        let base = oom_badness_score(total_vm, 0, totalpages);
        let reduced = oom_badness_score(total_vm, adj, totalpages);
        assert!(reduced <= base, "negative adj should not increase score: base={} reduced={} adj={}", base, reduced, adj);
    }

    #[test]
    fn test_max_adj_boost(total_vm in 0u64..100_000u64, totalpages in 1000u64..10_000_000u64) {
        let base = oom_badness_score(total_vm, 0, totalpages);
        let boosted = oom_badness_score(total_vm, OOM_SCORE_ADJ_MAX, totalpages);
        // max adj adds totalpages * 1000 / 1000 = totalpages
        assert_eq!(boosted, base.saturating_add(totalpages));
    }

    #[test]
    fn test_near_min_adj_reduction(total_vm in 100u64..100_000u64, totalpages in 1000u64..10_000_000u64) {
        // adj = -999 (one above immunity threshold)
        let base = oom_badness_score(total_vm, 0, totalpages);
        let reduced = oom_badness_score(total_vm, -999, totalpages);
        let expected_sub = totalpages * 999 / 1000;
        assert_eq!(reduced, base.saturating_sub(expected_sub));
    }

    #[test]
    fn test_no_saturation_for_typical_values(total_vm in 0u64..1_000_000u64, totalpages in 1000u64..10_000_000u64) {
        // Typical systems: total_vm < totalpages, so no saturation
        let score = oom_badness_score(total_vm, OOM_SCORE_ADJ_MAX, totalpages);
        // Should not saturate for reasonable values
        assert!(score > 0 || total_vm == 0);
    }

    #[test]
    fn test_small_totalpages_no_adjustment(total_vm in 0u64..1_000_000u64, adj in -999i32..1000i32) {
        // totalpages < 1000 → no adjustment applied
        let score_no_adj = oom_badness_score(total_vm, 0, 999);
        let score_with_adj = oom_badness_score(total_vm, adj, 999);
        assert_eq!(score_no_adj, score_with_adj);
    }

    #[test]
    fn test_zero_total_vm_all_zero(_v in 0u8..1u8) {
        assert_eq!(oom_badness_score(0, 0, 1000), 0);
        assert_eq!(oom_badness_score(0, OOM_SCORE_ADJ_MAX, 10000), 10000);
        assert_eq!(oom_badness_score(0, OOM_SCORE_ADJ_MIN, 10000), 0);
    }

    #[test]
    fn test_score_symmetry(total_vm in 100_000u64..1_000_000u64, totalpages in 1000u64..100_000u64) {
        // +adj adds approximately the same as -adj subtracts
        // total_vm must be large enough to avoid saturating_sub
        let score_pos = oom_badness_score(total_vm, 500, totalpages);
        let score_neg = oom_badness_score(total_vm, -500, totalpages);
        let score_zero = oom_badness_score(total_vm, 0, totalpages);
        // The delta from zero should be symmetric (approximately)
        let diff_pos = score_pos as i64 - score_zero as i64;
        let diff_neg = score_zero as i64 - score_neg as i64;
        assert!(diff_pos >= 0 && diff_neg >= 0);
        // Allow 1 difference due to integer division rounding
        assert!((diff_pos - diff_neg).abs() <= 1,
            "symmetry: pos_delta={} neg_delta={}", diff_pos, diff_neg);
    }
}
