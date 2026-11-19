//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for the page-cache contiguous-run walk
//! (`PageCache::get_range_into`). Copied from:
//! kernel/src/fs/page_cache.rs
//!
//! The caller (ext4_file_read_cached_dst) consumes the run POSITIONALLY:
//! run[i] is served as page `start + i`, the returned index is ignored.
//! The walk must therefore never step over a missing page — a BTreeMap
//! range() iterates only existing keys, so an explicit contiguity check is
//! required (the AC-2 CKSUM-MISMATCH family: valid file data served at the
//! wrong offset after eviction punched a hole inside a cached window).

use std::collections::BTreeMap;
use proptest::prelude::*;

/// Copied walk, PRE-fix (bare range iteration, no contiguity check).
/// Kept to demonstrate the violation this test guards against.
fn run_len_prefix(pages: &BTreeMap<u64, bool>, start: u64, count: u64) -> Vec<u64> {
    let mut out = Vec::new();
    for (&idx, &invalidated) in pages.range(start..start + count) {
        if invalidated {
            break;
        }
        out.push(idx);
    }
    out
}

/// Copied walk, POST-fix (expected-index contiguity check) — must match
/// kernel/src/fs/page_cache.rs get_range_into exactly in structure.
fn run_len_fixed(pages: &BTreeMap<u64, bool>, start: u64, count: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut expected = start;
    for (&idx, &invalidated) in pages.range(start..start + count) {
        if idx != expected {
            break;
        }
        if invalidated {
            break;
        }
        out.push(idx);
        expected += 1;
    }
    out
}

/// Run of `count` pages starting at `start`, some present, some missing.
fn arb_pages(max_page: u64) -> BoxedStrategy<(BTreeMap<u64, bool>, u64, u64)> {
    (
        prop::collection::btree_set(0u64..max_page, 0..(max_page as usize).min(24)),
        0u64..max_page,
        1u64..=8u64,
    )
        .prop_map(|(present, start, count)| {
            let mut pages = BTreeMap::new();
            for &p in &present {
                pages.insert(p, false);
            }
            (pages, start, count)
        })
        .boxed()
}

proptest! {
    /// CONTRACT: out[i].0 == start + i for every returned entry — the caller
    /// serves run[i] as page start+i. The fixed walk must never violate it.
    #[test]
    fn run_is_contiguous_from_start((ref pages, start, count) in arb_pages(16)) {
        let run = run_len_fixed(pages, start, count);
        for (i, &idx) in run.iter().enumerate() {
            prop_assert_eq!(idx, start + i as u64);
        }
    }

    /// The fixed walk never returns MORE pages than the pre-fix walk
    /// (it only stops earlier at gaps).
    #[test]
    fn fixed_is_prefix_of_prefix_walk((ref pages, start, count) in arb_pages(16)) {
        let pre = run_len_prefix(pages, start, count);
        let post = run_len_fixed(pages, start, count);
        prop_assert!(post.len() <= pre.len());
        for (i, &idx) in post.iter().enumerate() {
            prop_assert_eq!(pre.get(i), Some(&idx));
        }
    }

    /// Every entry the fixed walk returns is cached and non-invalidated,
    /// and the page after the run end is missing, invalidated, or the
    /// window end (otherwise the walk stopped too early).
    #[test]
    fn run_stops_only_at_barrier((ref pages, start, count) in arb_pages(16)) {
        let run = run_len_fixed(pages, start, count);
        let n = run.len() as u64;
        if start + n < start + count {
            let next = start + n;
            prop_assert!(matches!(pages.get(&next), None | Some(true)));
        }
    }
}

/// Regression cases from the corruption family (deterministic pinpoints).
#[test]
fn gap_regressions() {
    let mut pages = BTreeMap::new();
    // Window [0,8): page 0 MISSING (evicted), 1..7 cached.
    for p in 1..8u64 {
        pages.insert(p, false);
    }
    // Pre-fix bug: served page 1's content as page 0's.
    assert_eq!(run_len_prefix(&pages, 0, 8), vec![1, 2, 3, 4, 5, 6, 7]);
    // Fixed: run is empty, caller refills from page 0.
    assert_eq!(run_len_fixed(&pages, 0, 8), Vec::<u64>::new());

    // Middle hole (evicted page 3 / fill skipped a hole).
    pages.insert(0, false);
    pages.remove(&3);
    let pre = run_len_prefix(&pages, 0, 8);
    assert_eq!(pre, vec![0, 1, 2, 4, 5, 6, 7]); // 4 shifted into 3's slot
    let post = run_len_fixed(&pages, 0, 8);
    assert_eq!(post, vec![0, 1, 2]);

    // Invalidated first page still stops the run.
    pages.insert(0, true);
    assert_eq!(run_len_fixed(&pages, 0, 8), Vec::<u64>::new());

    // Fully contiguous window unaffected.
    let mut full = BTreeMap::new();
    for p in 0..8u64 {
        full.insert(p, false);
    }
    assert_eq!(run_len_fixed(&full, 0, 8), vec![0, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(run_len_fixed(&full, 2, 3), vec![2, 3, 4]);
}
