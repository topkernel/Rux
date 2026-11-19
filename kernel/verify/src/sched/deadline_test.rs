//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Deadline scheduler bandwidth and runtime invariant tests.
//!
//! Types copied from: kernel/src/sched/deadline.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/sched/deadline.rs
// ============================================================================

pub const DL_DEFAULT_PERIOD_NS: u64 = 1_000_000_000;
pub const DL_DEFAULT_RUNTIME_NS: u64 = 100_000_000;
pub const DL_BW_UNIT: u64 = 1 << 20;
pub const DL_BW_MAX: u64 = DL_BW_UNIT;

#[derive(Debug, Clone)]
pub struct SchedDlEntity {
    pub deadline: u64,
    pub runtime: i64,
    pub dl_period: u64,
    pub dl_runtime: u64,
    pub dl_throttled: bool,
}

impl SchedDlEntity {
    pub fn new() -> Self {
        Self {
            deadline: 0,
            runtime: DL_DEFAULT_RUNTIME_NS as i64,
            dl_period: DL_DEFAULT_PERIOD_NS,
            dl_runtime: DL_DEFAULT_RUNTIME_NS,
            dl_throttled: false,
        }
    }

    /// Bandwidth = runtime * DL_BW_UNIT / period
    pub fn get_bw(&self) -> u64 {
        if self.dl_period == 0 {
            return 0;
        }
        (self.dl_runtime * DL_BW_UNIT) / self.dl_period
    }

    /// Advance deadline to now + period
    pub fn update_deadline(&mut self, now: u64) {
        self.deadline = now + self.dl_period;
    }

    /// Reset runtime to dl_runtime, clear throttled
    pub fn replenish_runtime(&mut self) {
        self.runtime = self.dl_runtime as i64;
        self.dl_throttled = false;
    }

    /// Consume runtime, return true if still has time
    pub fn consume_runtime(&mut self, delta: u64) -> bool {
        let remaining = self.runtime - delta as i64;
        if remaining <= 0 {
            self.runtime = 0;
            self.dl_throttled = true;
            false
        } else {
            self.runtime = remaining;
            true
        }
    }
}

impl Default for SchedDlEntity {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-DL-1: Default entity is not throttled
    #[test]
    fn test_default_not_throttled(_v in 0u8..1u8) {
        let dl = SchedDlEntity::new();
        prop_assert!(!dl.dl_throttled);
        prop_assert!(dl.runtime > 0);
    }

    /// INV-DL-2: get_bw is 0 when period is 0
    #[test]
    fn test_bw_zero_period(runtime in 0u64..1_000_000_000u64) {
        let mut dl = SchedDlEntity::new();
        dl.dl_period = 0;
        dl.dl_runtime = runtime;
        prop_assert_eq!(dl.get_bw(), 0);
    }

    /// INV-DL-3: get_bw is DL_BW_UNIT when runtime == period
    #[test]
    fn test_bw_100_percent(period in 1u64..1_000_000_000u64) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = period;
        dl.dl_period = period;
        prop_assert_eq!(dl.get_bw(), DL_BW_UNIT);
    }

    /// INV-DL-4: get_bw never exceeds DL_BW_MAX when runtime <= period
    #[test]
    fn test_bw_capped(
        runtime in 1u64..10_000_000u64,
        period in 1u64..10_000_000u64,
    ) {
        // When runtime <= period, bw <= DL_BW_MAX
        let (rt, pd) = if runtime <= period { (runtime, period) } else { (period, runtime) };
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = rt;
        dl.dl_period = pd;
        prop_assert!(dl.get_bw() <= DL_BW_MAX);
    }

    /// INV-DL-5: get_bw is 0 when runtime is 0
    #[test]
    fn test_bw_zero_runtime(period in 1u64..1_000_000_000u64) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = 0;
        dl.dl_period = period;
        prop_assert_eq!(dl.get_bw(), 0);
    }

    /// INV-DL-6: consume_runtime reduces runtime
    #[test]
    fn test_consume_reduces(
        runtime in 100u64..1_000_000u64,
        delta in 1u64..100u64,
    ) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = runtime;
        dl.runtime = runtime as i64;
        let before = dl.runtime;
        dl.consume_runtime(delta);
        prop_assert!(dl.runtime < before);
    }

    /// INV-DL-7: consume_runtime throttles when runtime exhausted
    #[test]
    fn test_consume_throttle(
        runtime in 1u64..100u64,
    ) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = runtime;
        dl.runtime = runtime as i64;
        // Consume more than available
        let result = dl.consume_runtime(runtime + 1);
        prop_assert!(!result);
        prop_assert!(dl.dl_throttled);
        prop_assert_eq!(dl.runtime, 0);
    }

    /// INV-DL-8: replenish restores runtime and clears throttle
    #[test]
    fn test_replenish(runtime in 1u64..1_000_000u64) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = runtime;
        dl.dl_period = runtime * 10;
        dl.runtime = runtime as i64;
        // Exhaust runtime
        dl.consume_runtime(runtime + 1);
        prop_assert!(dl.dl_throttled);
        // Replenish
        dl.replenish_runtime();
        prop_assert!(!dl.dl_throttled);
        prop_assert_eq!(dl.runtime, runtime as i64);
    }

    /// INV-DL-9: update_deadline sets deadline = now + period
    #[test]
    fn test_update_deadline(
        now in 0u64..10_000_000_000u64,
        period in 1u64..10_000_000_000u64,
    ) {
        let mut dl = SchedDlEntity::new();
        dl.dl_period = period;
        dl.update_deadline(now);
        prop_assert_eq!(dl.deadline, now + period);
    }

    /// INV-DL-10: Deadline advances monotonically
    #[test]
    fn test_deadline_monotone(
        now in 0u64..10_000_000u64,
        period in 1u64..1_000_000u64,
    ) {
        let mut dl = SchedDlEntity::new();
        dl.dl_period = period;
        dl.update_deadline(now);
        let d1 = dl.deadline;
        dl.update_deadline(d1);
        let d2 = dl.deadline;
        prop_assert!(d2 > d1);
    }

    /// INV-DL-11: get_bw is monotone in runtime for fixed period
    #[test]
    fn test_bw_monotone_runtime(
        r1 in 0u64..1_000_000u64,
        r2 in 0u64..1_000_000u64,
        period in 1u64..1_000_000u64,
    ) {
        let (small, large) = if r1 <= r2 { (r1, r2) } else { (r2, r1) };
        let mut dl1 = SchedDlEntity::new();
        dl1.dl_runtime = small;
        dl1.dl_period = period;
        let mut dl2 = SchedDlEntity::new();
        dl2.dl_runtime = large;
        dl2.dl_period = period;
        prop_assert!(dl1.get_bw() <= dl2.get_bw());
    }

    /// INV-DL-12: get_bw is antitone in period for fixed runtime
    #[test]
    fn test_bw_antitone_period(
        p1 in 1u64..1_000_000u64,
        p2 in 1u64..1_000_000u64,
        runtime in 1u64..1_000_000u64,
    ) {
        let (small, large) = if p1 <= p2 { (p1, p2) } else { (p2, p1) };
        let mut dl1 = SchedDlEntity::new();
        dl1.dl_runtime = runtime;
        dl1.dl_period = small;
        let mut dl2 = SchedDlEntity::new();
        dl2.dl_runtime = runtime;
        dl2.dl_period = large;
        prop_assert!(dl1.get_bw() >= dl2.get_bw());
    }

    /// INV-DL-13: Consume zero is no-op
    #[test]
    fn test_consume_zero(runtime in 1u64..1_000_000u64) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = runtime;
        dl.runtime = runtime as i64;
        let result = dl.consume_runtime(0);
        prop_assert!(result);
        prop_assert!(!dl.dl_throttled);
        prop_assert_eq!(dl.runtime, runtime as i64);
    }

    /// INV-DL-14: Default values match constants
    #[test]
    fn test_default_values(_v in 0u8..1u8) {
        let dl = SchedDlEntity::new();
        prop_assert_eq!(dl.dl_period, DL_DEFAULT_PERIOD_NS);
        prop_assert_eq!(dl.dl_runtime, DL_DEFAULT_RUNTIME_NS);
        prop_assert_eq!(dl.runtime, DL_DEFAULT_RUNTIME_NS as i64);
    }

    /// INV-DL-15: Runtime never goes negative
    #[test]
    fn test_runtime_nonnegative(
        runtime in 1u64..1000u64,
        delta in 1u64..2000u64,
    ) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = runtime;
        dl.runtime = runtime as i64;
        dl.consume_runtime(delta);
        prop_assert!(dl.runtime >= 0);
    }

    /// INV-DL-16: Repeated consume doesn't panic when throttled
    #[test]
    fn test_repeated_consume(runtime in 1u64..100u64) {
        let mut dl = SchedDlEntity::new();
        dl.dl_runtime = runtime;
        dl.runtime = runtime as i64;
        dl.consume_runtime(runtime + 1); // throttle
        dl.consume_runtime(1); // should not panic
        prop_assert!(dl.dl_throttled);
        prop_assert_eq!(dl.runtime, 0);
    }
}
