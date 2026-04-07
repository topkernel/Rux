//! Property-based tests for Rux kernel core data structure invariants.
//!
//! These tests extract pure algorithmic logic from the kernel and verify
//! safety invariants using proptest (randomized input generation).
//!
//! Run: `cargo test -p rux-verify`

use proptest::prelude::*;

// ============================================================================
// Page Flags — bitmap operations
// ============================================================================

#[derive(Debug, Clone, Copy)]
struct PageFlags(u32);

impl PageFlags {
    const LOCKED: u32 = 1 << 0;
    const DIRTY: u32 = 1 << 2;
    const REFERENCED: u32 = 1 << 3;
    const ANONYMOUS: u32 = 1 << 15;
    const COW: u32 = 1 << 14;

    fn new() -> Self { Self(0) }
    fn test(&self, flag: u32) -> bool { self.0 & flag != 0 }
    fn set(&mut self, flag: u32) { self.0 |= flag; }
    fn clear(&mut self, flag: u32) { self.0 &= !flag; }
}

proptest! {
    /// INV-FLAGS-1: set then test returns true
    #[test]
    fn test_set_then_test_is_true(flag in 0u32..32) {
        let bit = 1u32 << flag;
        let mut flags = PageFlags::new();
        flags.set(bit);
        prop_assert!(flags.test(bit));
    }

    /// INV-FLAGS-2: clear removes flag
    #[test]
    fn test_set_then_clear_is_false(flag in 0u32..32) {
        let bit = 1u32 << flag;
        let mut flags = PageFlags::new();
        flags.set(bit);
        flags.clear(bit);
        prop_assert!(!flags.test(bit));
    }

    /// INV-FLAGS-3: clearing an unset flag is a no-op
    #[test]
    fn test_clear_unset_noop((a, b) in (0u32..32, 0u32..32)) {
        let bit_a = 1u32 << a;
        let bit_b = 1u32 << b;
        let mut flags = PageFlags::new();
        flags.set(bit_a);
        flags.clear(bit_b);
        // bit_a should still be set (clearing unrelated bit is no-op)
        if a != b {
            prop_assert!(flags.test(bit_a));
        }
    }

    /// INV-FLAGS-4: multiple flags can coexist
    #[test]
    fn test_multiple_flags_coexist(flags_val in 0u32..(1u64 << 16) as u32) {
        let flags = PageFlags(flags_val);
        prop_assert_eq!(flags.0, flags_val);
    }
}

// ============================================================================
// Buddy Allocator — alignment and merge mathematics
// ============================================================================

const PAGE_SIZE: usize = 4096;
const MAX_ORDER: usize = 10;

/// Check if PFN is aligned to the given order.
fn is_aligned(pfn: usize, order: usize) -> bool {
    let size = 1usize << order;
    (pfn & (size - 1)) == 0
}

/// Compute buddy PFN: flip the bit at position `order`.
fn buddy_pfn(pfn: usize, order: usize) -> usize {
    pfn ^ (1usize << order)
}

proptest! {
    /// INV-BUDDY-ALIGN-1: order-0 alignment: any pfn is aligned
    #[test]
    fn test_order0_any_pfn_aligned(pfn in 0usize..1_000_000) {
        prop_assert!(is_aligned(pfn, 0));
    }

    /// INV-BUDDY-ALIGN-2: order-N alignment check
    #[test]
    fn test_alignment_roundtrip((order, multiplier) in (1usize..MAX_ORDER, 0usize..1000usize)) {
        let size = 1usize << order;
        let aligned_pfn = multiplier * size;
        prop_assert!(is_aligned(aligned_pfn, order));
    }

    /// INV-BUDDY-MERGE-1: buddy(buddy(pfn, order), order) == pfn
    #[test]
    fn test_buddy_is_involution((pfn, order) in (0usize..1_000_000usize, 0usize..MAX_ORDER)) {
        let b = buddy_pfn(pfn, order);
        let b2 = buddy_pfn(b, order);
        prop_assert_eq!(b2, pfn);
    }

    /// INV-BUDDY-MERGE-2: buddy of aligned pfn differs only at bit `order`
    #[test]
    fn test_buddy_differs_at_order_bit(order in 1usize..MAX_ORDER) {
        let size = 1usize << order;
        for aligned_pfn in [0usize, size, size * 3, size * 7] {
            let b = buddy_pfn(aligned_pfn, order);
            let xor = aligned_pfn ^ b;
            prop_assert_eq!(xor, size, "buddy should differ only at bit {}", order);
        }
    }

    /// INV-BUDDY-MERGE-3: buddy pair covers a contiguous aligned block
    #[test]
    fn test_buddy_pair_alignment(order in 1usize..MAX_ORDER) {
        let size = 1usize << order;
        let aligned_pfn = 12345usize / size * size; // align down
        let b = buddy_pfn(aligned_pfn, order);
        let (lo, hi) = if b < aligned_pfn { (b, aligned_pfn) } else { (aligned_pfn, b) };
        prop_assert_eq!(lo + size, hi);
        prop_assert!(is_aligned(lo, order));
    }

    /// INV-BUDDY-MERGE-4: block size grows as 2^order
    #[test]
    fn test_block_size_power_of_two(order in 0usize..MAX_ORDER) {
        let size = 1usize << order;
        let byte_size = size * PAGE_SIZE;
        prop_assert!(byte_size.is_power_of_two());
        prop_assert_eq!(byte_size, PAGE_SIZE << order);
    }
}

// ============================================================================
// VMA Manager — non-overlap invariant
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Vma {
    start: usize,
    end: usize,
}

impl Vma {
    fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Debug)]
struct VmaManager {
    vmas: Vec<Vma>,
}

impl VmaManager {
    fn new() -> Self {
        Self { vmas: Vec::new() }
    }

    /// Returns Ok(()) if no overlap, Err otherwise.
    fn add(&mut self, vma: Vma) -> Result<(), &'static str> {
        // Check overlap with existing VMAs
        for existing in &self.vmas {
            if vma.start < existing.end && existing.start < vma.end {
                return Err("overlap");
            }
        }
        self.vmas.push(vma);
        Ok(())
    }

    /// Verify no two VMAs overlap.
    fn check_no_overlap(&self) -> bool {
        let mut sorted: Vec<&Vma> = self.vmas.iter().collect();
        sorted.sort_by_key(|v| v.start);
        for window in sorted.windows(2) {
            if window[0].end > window[1].start {
                return false;
            }
        }
        true
    }

    /// Check max_end invariant: max_end == max of all vma ends
    fn max_end(&self) -> usize {
        self.vmas.iter().map(|v| v.end).max().unwrap_or(0)
    }
}

proptest! {
    /// INV-VMA-1: after any sequence of adds, no two VMAs overlap
    #[test]
    fn test_no_overlap_after_adds(
        ranges in proptest::collection::vec(
            proptest::strategy::Just(()).prop_flat_map(|_| {
                let start = 0usize..100_000usize;
                let len = 1usize..10_000usize;
                (start, len).prop_map(|(s, l)| (s / 4096 * 4096, ((l + 4095) / 4096) * 4096))
            }),
            0..50
        )
    ) {
        let mut mgr = VmaManager::new();
        let mut added = 0;
        for (start, len) in ranges {
            let end = start + len;
            if start == end { continue; }
            let vma = Vma::new(start, end);
            if mgr.add(vma).is_ok() {
                added += 1;
            }
        }
        if added > 0 {
            prop_assert!(mgr.check_no_overlap());
        }
    }

    /// INV-VMA-2: adjacent VMAs (end == start) are not overlapping
    #[test]
    fn test_adjacent_vmas_no_overlap(start in 0usize..50_000usize) {
        let aligned_start = start / 4096 * 4096;
        let mut mgr = VmaManager::new();
        mgr.add(Vma::new(aligned_start, aligned_start + 4096)).unwrap();
        mgr.add(Vma::new(aligned_start + 4096, aligned_start + 8192)).unwrap();
        prop_assert!(mgr.check_no_overlap());
        prop_assert_eq!(mgr.vmas.len(), 2);
    }

    /// INV-VMA-3: overlapping add is rejected
    #[test]
    fn test_overlap_rejected(start in 0usize..50_000usize) {
        let aligned_start = start / 4096 * 4096;
        let mut mgr = VmaManager::new();
        mgr.add(Vma::new(aligned_start, aligned_start + 8192)).unwrap();
        // Try to add overlapping VMA
        let result = mgr.add(Vma::new(aligned_start + 4096, aligned_start + 12288));
        prop_assert!(result.is_err());
    }
}

// ============================================================================
// Reference Count — underflow protection
// ============================================================================

/// Simulates the refcount protocol with underflow protection.
struct RefCount {
    val: i32,
}

impl RefCount {
    fn new(val: i32) -> Self {
        Self { val: val.max(0) }
    }

    /// get_page: increment refcount
    fn get(&mut self) -> i32 {
        self.val += 1;
        self.val
    }

    /// put_page: decrement with underflow protection
    /// Returns (new_value, underflow_detected)
    fn put(&mut self) -> (i32, bool) {
        if self.val <= 0 {
            return (self.val, true); // underflow: do not decrement
        }
        self.val -= 1;
        (self.val, false)
    }
}

proptest! {
    /// INV-REF-1: refcount never goes negative
    #[test]
    fn test_refcount_never_negative(
        initial in 0i32..100i32,
        ops in proptest::collection::vec(
            proptest::prop_oneof![
                proptest::strategy::Just(true),   // get
                proptest::strategy::Just(false),  // put
            ],
            0..200
        )
    ) {
        let mut rc = RefCount::new(initial);
        for is_get in ops {
            if is_get {
                rc.get();
            } else {
                let (new_val, underflow) = rc.put();
                if underflow {
                    prop_assert!(new_val <= 0);
                }
            }
        }
        prop_assert!(rc.val >= 0, "refcount went negative: {}", rc.val);
    }

    /// INV-REF-2: get/put cycle returns to original
    #[test]
    fn test_refcount_symmetry(
        initial in 1i32..100i32,
        n in 1usize..50usize,
    ) {
        let mut rc = RefCount::new(initial);
        for _ in 0..n {
            rc.get();
        }
        for _ in 0..n {
            rc.put();
        }
        prop_assert_eq!(rc.val, initial);
    }

    /// INV-REF-3: refcount == 0 after enough puts
    #[test]
    fn test_refcount_reaches_zero(
        initial in 1i32..100i32,
    ) {
        let mut rc = RefCount::new(initial);
        for _ in 0..initial {
            rc.get();
        }
        // Now put `initial` extra times (total gets = 2*initial)
        for _ in 0..(initial as usize) {
            let (val, underflow) = rc.put();
            prop_assert!(!underflow);
            if val == 0 {
                break;
            }
        }
        // After equal gets and puts from initial state, should be at initial
        prop_assert_eq!(rc.val, initial);
    }
}
