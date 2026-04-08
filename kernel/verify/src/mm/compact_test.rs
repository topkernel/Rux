//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memory compaction control and termination invariant tests.
//!
//! Types copied from: kernel/src/mm/compact.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/mm/compact.rs
// ============================================================================

pub const MAX_SCAN_PAGES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompactResult {
    Success,
    Complete,
    Skipped,
}

pub struct CompactControl {
    pub migrate_pfn: usize,
    pub free_pfn: usize,
    pub order: usize,
    pub nr_migrated: usize,
    pub nr_scanned: usize,
}

impl CompactControl {
    pub fn new(start: usize, end: usize, order: usize) -> Self {
        Self {
            migrate_pfn: start,
            free_pfn: end,
            order,
            nr_migrated: 0,
            nr_scanned: 0,
        }
    }

    /// Check termination: scanners met or exceeded scan limit.
    /// Returns the result if terminated, None if should continue.
    pub fn check_termination(&self) -> Option<CompactResult> {
        if self.migrate_pfn >= self.free_pfn || self.nr_scanned >= MAX_SCAN_PAGES {
            Some(if self.nr_migrated > 0 {
                CompactResult::Complete
            } else {
                CompactResult::Skipped
            })
        } else {
            None
        }
    }

    /// Simulate advancing the migrate scanner (upward)
    pub fn advance_migrate(&mut self) {
        self.migrate_pfn += 1;
        self.nr_scanned += 1;
    }

    /// Simulate advancing the free scanner (downward)
    pub fn advance_free(&mut self, min_pfn: usize) {
        if self.free_pfn > min_pfn {
            self.free_pfn -= 1;
            self.nr_scanned += 1;
        }
    }

    /// Simulate a successful migration
    pub fn record_migration(&mut self) {
        self.nr_migrated += 1;
    }
}

/// Page migration filter predicate (pure logic).
/// Returns true if the page should be migrated.
pub fn is_migratable(
    is_free: bool,
    is_reserved: bool,
    is_anonymous: bool,
    is_mapped: bool,
    refcount: usize,
    is_dirty: bool,
) -> bool {
    !is_free && !is_reserved && is_anonymous && is_mapped && refcount == 1 && !is_dirty
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-CMP-1: Start >= end returns immediate termination
    #[test]
    fn test_start_ge_end(
        start in 1000usize..2000usize,
        delta in 0usize..1000usize,
    ) {
        let end = start - delta;
        let cc = CompactControl::new(start, end, 0);
        prop_assert!(cc.check_termination().is_some());
    }

    /// INV-CMP-2: Start < end does not terminate immediately
    #[test]
    fn test_start_lt_end(
        start in 0usize..1000usize,
        range in 1usize..1000usize,
    ) {
        let end = start + range;
        let cc = CompactControl::new(start, end, 0);
        prop_assert!(cc.check_termination().is_none());
    }

    /// INV-CMP-3: Scanners meeting terminates
    #[test]
    fn test_scanners_meet(
        start in 0usize..100usize,
        range in 2usize..200usize,
    ) {
        let end = start + range;
        let mut cc = CompactControl::new(start, end, 0);
        // Advance both until they meet
        while cc.check_termination().is_none() {
            cc.advance_migrate();
            cc.advance_free(start);
        }
        prop_assert!(cc.migrate_pfn >= cc.free_pfn || cc.nr_scanned >= MAX_SCAN_PAGES);
    }

    /// INV-CMP-4: MAX_SCAN_PAGES limits scanning
    #[test]
    fn test_max_scan_limit(
        start in 0usize..100usize,
        end in 100_000usize..200_000usize,
    ) {
        let mut cc = CompactControl::new(start, end, 0);
        // Simulate advancing migrate scanner only (won't meet free scanner)
        let mut iterations = 0usize;
        while cc.check_termination().is_none() {
            cc.advance_migrate();
            iterations += 1;
            prop_assert!(iterations <= MAX_SCAN_PAGES + 1);
        }
    }

    /// INV-CMP-5: Migrated pages yield Complete, not Skipped
    #[test]
    fn test_migrated_complete(
        start in 0usize..100usize,
        range in 10usize..200usize,
    ) {
        let end = start + range;
        let mut cc = CompactControl::new(start, end, 0);
        // Record a migration, then exhaust scans
        cc.record_migration();
        for _ in 0..MAX_SCAN_PAGES {
            cc.advance_migrate();
        }
        match cc.check_termination() {
            Some(CompactResult::Complete) => {},
            Some(_) => prop_assert!(false, "should be Complete"),
            None => prop_assert!(false, "should have terminated"),
        }
    }

    /// INV-CMP-6: No migrations yields Skipped
    #[test]
    fn test_no_migrate_skipped(
        start in 0usize..100usize,
        range in 2usize..200usize,
    ) {
        let end = start + range;
        let mut cc = CompactControl::new(start, end, 0);
        // Exhaust scans without migration
        for _ in 0..MAX_SCAN_PAGES {
            cc.advance_migrate();
        }
        match cc.check_termination() {
            Some(CompactResult::Skipped) => {},
            Some(_) => prop_assert!(false, "should be Skipped"),
            None => prop_assert!(false, "should have terminated"),
        }
    }

    /// INV-CMP-7: nr_scanned increments correctly
    #[test]
    fn test_scanned_count(steps in 1usize..100usize) {
        let mut cc = CompactControl::new(0, 10000, 0);
        for _ in 0..steps {
            cc.advance_migrate();
        }
        prop_assert_eq!(cc.nr_scanned, steps);
    }

    /// INV-CMP-8: Free scanner doesn't go below min_pfn
    #[test]
    fn test_free_scanner_floor(
        start in 0usize..100usize,
        min_pfn in 0usize..100usize,
        steps in 1usize..200usize,
    ) {
        let end = start + 10000;
        let mut cc = CompactControl::new(start, end, 0);
        for _ in 0..steps {
            cc.advance_free(min_pfn);
        }
        prop_assert!(cc.free_pfn >= min_pfn);
    }

    /// INV-CMP-9: Migrate scanner only goes up
    #[test]
    fn test_migrate_goes_up(
        start in 0usize..1000usize,
        end in 2000usize..5000usize,
        steps in 1usize..100usize,
    ) {
        let mut cc = CompactControl::new(start, end, 0);
        let initial = cc.migrate_pfn;
        for _ in 0..steps {
            cc.advance_migrate();
        }
        prop_assert!(cc.migrate_pfn > initial);
    }

    /// INV-CMP-10: Free scanner only goes down
    #[test]
    fn test_free_goes_down(
        start in 0usize..1000usize,
        end in 2000usize..5000usize,
        steps in 1usize..100usize,
    ) {
        let mut cc = CompactControl::new(start, end, 0);
        let initial = cc.free_pfn;
        for _ in 0..steps {
            cc.advance_free(0);
        }
        prop_assert!(cc.free_pfn < initial);
    }
}

proptest! {
    /// INV-CMP-11: Free page is not migratable
    #[test]
    fn test_filter_free(
        refcount in 1usize..10usize,
        mapped in 0u8..2u8,
        dirty in 0u8..2u8,
    ) {
        prop_assert!(!is_migratable(true, false, true, mapped != 0, refcount, dirty != 0));
    }

    /// INV-CMP-12: Reserved page is not migratable
    #[test]
    fn test_filter_reserved(
        refcount in 1usize..10usize,
        mapped in 0u8..2u8,
        dirty in 0u8..2u8,
    ) {
        prop_assert!(!is_migratable(false, true, true, mapped != 0, refcount, dirty != 0));
    }

    /// INV-CMP-13: Non-anonymous page is not migratable
    #[test]
    fn test_filter_non_anon(
        refcount in 1usize..10usize,
        mapped in 0u8..2u8,
        dirty in 0u8..2u8,
    ) {
        prop_assert!(!is_migratable(false, false, false, mapped != 0, refcount, dirty != 0));
    }

    /// INV-CMP-14: Dirty page is not migratable
    #[test]
    fn test_filter_dirty(
        refcount in 1usize..10usize,
        mapped in 0u8..2u8,
    ) {
        prop_assert!(!is_migratable(false, false, true, mapped != 0, refcount, true));
    }

    /// INV-CMP-15: refcount != 1 is not migratable
    #[test]
    fn test_filter_refcount(
        rc in 2usize..10usize,
        mapped in 0u8..2u8,
        dirty in 0u8..2u8,
    ) {
        prop_assert!(!is_migratable(false, false, true, mapped != 0, rc, dirty != 0));
    }

    /// INV-CMP-16: Ideal page is migratable
    #[test]
    fn test_filter_ideal(_v in 0u8..1u8) {
        prop_assert!(is_migratable(false, false, true, true, 1, false));
    }
}
