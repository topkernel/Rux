//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VMA Manager non-overlap and correctness invariant tests.
//!
//! Types copied from: kernel/src/mm/vma.rs, kernel/src/mm/page.rs

use proptest::prelude::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

// ============================================================================
// Copied types from kernel/src/mm/page.rs
// ============================================================================

pub const PAGE_SIZE: usize = 4096;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub usize);

impl VirtAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !(PAGE_SIZE - 1))
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & (PAGE_SIZE - 1) == 0
    }
}

// ============================================================================
// Copied types from kernel/src/mm/vma.rs
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaFlags(u32);

impl VmaFlags {
    pub const READ: u32 = 0x00000001;
    pub const WRITE: u32 = 0x00000002;
    pub const EXEC: u32 = 0x00000004;

    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaType {
    Anonymous,
    FileBacked,
    Device,
    SharedMemory,
}

#[derive(Clone, Copy)]
pub struct Vma {
    start: VirtAddr,
    end: VirtAddr,
    flags: VmaFlags,
    offset: usize,
    vma_type: VmaType,
    file_fd: i32,
    file_size: u64,
}

impl Vma {
    pub fn new(start: VirtAddr, end: VirtAddr, flags: VmaFlags) -> Self {
        assert!(start.as_usize() < end.as_usize(), "Invalid VMA range");
        assert!(start.as_usize() % PAGE_SIZE == 0, "VMA start not page aligned");
        assert!(end.as_usize() % PAGE_SIZE == 0, "VMA end not page aligned");

        Self {
            start,
            end,
            flags,
            offset: 0,
            vma_type: VmaType::Anonymous,
            file_fd: -1,
            file_size: 0,
        }
    }

    #[inline]
    pub fn start(&self) -> VirtAddr {
        self.start
    }

    #[inline]
    pub fn end(&self) -> VirtAddr {
        self.end
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.end.as_usize() - self.start.as_usize()
    }

    #[inline]
    pub fn page_count(&self) -> usize {
        self.size() / PAGE_SIZE
    }

    #[inline]
    pub fn flags(&self) -> VmaFlags {
        self.flags
    }

    #[inline]
    pub fn contains(&self, addr: VirtAddr) -> bool {
        addr.as_usize() >= self.start.as_usize() && addr.as_usize() < self.end.as_usize()
    }

    pub fn overlaps(&self, other: &Vma) -> bool {
        self.start.as_usize() < other.end.as_usize()
            && other.start.as_usize() < self.end.as_usize()
    }

    pub fn split(&self, addr: VirtAddr) -> Option<(Vma, Vma)> {
        if !self.contains(addr) {
            return None;
        }

        let aligned_addr = VirtAddr::new(addr.as_usize() & !(PAGE_SIZE - 1));
        if aligned_addr.as_usize() <= self.start.as_usize()
            || aligned_addr.as_usize() >= self.end.as_usize()
        {
            return None;
        }

        let first = Vma {
            start: self.start,
            end: aligned_addr,
            flags: self.flags,
            offset: self.offset,
            vma_type: self.vma_type,
            file_fd: self.file_fd,
            file_size: self.file_size,
        };

        let second = Vma {
            start: aligned_addr,
            end: self.end,
            flags: self.flags,
            offset: self.offset + (aligned_addr.as_usize() - self.start.as_usize()),
            vma_type: self.vma_type,
            file_fd: self.file_fd,
            file_size: self.file_size,
        };

        Some((first, second))
    }

    pub fn can_merge(&self, other: &Vma) -> bool {
        self.end.as_usize() == other.start.as_usize()
            && self.flags.bits() == other.flags.bits()
            && self.vma_type == other.vma_type
    }
}

impl std::fmt::Debug for Vma {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vma")
            .field("range", &format_args!("0x{:x}-0x{:x}", self.start.as_usize(), self.end.as_usize()))
            .field("size", &self.size())
            .field("flags", &self.flags)
            .field("type", &self.vma_type)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaError {
    Overlap,
    NoSpace,
    NotFound,
    Invalid,
}

pub struct VmaManager {
    vmas: BTreeMap<VirtAddr, Vma>,
    max_end: VirtAddr,
    count: AtomicU32,
}

impl VmaManager {
    pub fn new() -> Self {
        Self {
            vmas: BTreeMap::new(),
            max_end: VirtAddr::new(0),
            count: AtomicU32::new(0),
        }
    }

    pub fn add(&mut self, vma: Vma) -> Result<(), VmaError> {
        let start = vma.start();
        let end = vma.end();

        if let Some((_, prev_vma)) = self.vmas.range(..start).next_back() {
            if prev_vma.end().as_usize() > start.as_usize() {
                return Err(VmaError::Overlap);
            }
        }

        if let Some((_, next_vma)) = self.vmas.range(start..end).next() {
            return Err(VmaError::Overlap);
        }

        if end.as_usize() > self.max_end.as_usize() {
            self.max_end = end;
        }

        self.vmas.insert(start, vma);
        self.count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn find(&self, addr: VirtAddr) -> Option<&Vma> {
        if addr.as_usize() >= self.max_end.as_usize() {
            return None;
        }

        if let Some((_, vma)) = self.vmas.range(..=addr).next_back() {
            if vma.contains(addr) {
                return Some(vma);
            }
        }

        None
    }

    pub fn remove(&mut self, start: VirtAddr) -> Result<(), VmaError> {
        if let Some(removed) = self.vmas.remove(&start) {
            if removed.end() == self.max_end {
                self.max_end = self.vmas.values()
                    .map(|v| v.end())
                    .max()
                    .unwrap_or(VirtAddr::new(0));
            }
            self.count.fetch_sub(1, Ordering::Release);
            Ok(())
        } else {
            Err(VmaError::NotFound)
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Vma> {
        self.vmas.values()
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.vmas.len()
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-VMA-1: after any sequence of adds, no two VMAs overlap
    #[test]
    fn test_no_overlap_after_adds(
        ranges in proptest::collection::vec(
            proptest::strategy::Just(()).prop_flat_map(|_| {
                let start = 0usize..100_000usize;
                let len = 1usize..10_000usize;
                (start, len).prop_map(|(s, l)| {
                    let aligned_start = s / 4096 * 4096;
                    let aligned_len = ((l + 4095) / 4096) * 4096;
                    (aligned_start, aligned_len)
                })
            }),
            0..50
        ),
    ) {
        let mut mgr = VmaManager::new();
        let mut added = 0;
        for (start, len) in ranges {
            let end = start + len;
            if start == end || len == 0 { continue; }
            let vma = Vma::new(VirtAddr::new(start), VirtAddr::new(end), VmaFlags::new());
            if mgr.add(vma).is_ok() {
                added += 1;
            }
        }
        if added > 0 {
            let mut sorted: Vec<_> = mgr.iter().collect();
            sorted.sort_by_key(|v| v.start().as_usize());
            for window in sorted.windows(2) {
                prop_assert!(window[0].end().as_usize() <= window[1].start().as_usize(),
                    "overlap: 0x{:x}-0x{:x} vs 0x{:x}-0x{:x}",
                    window[0].start().as_usize(), window[0].end().as_usize(),
                    window[1].start().as_usize(), window[1].end().as_usize());
            }
        }
    }

    /// INV-VMA-2: adjacent VMAs (end == start) are not overlapping
    #[test]
    fn test_adjacent_vmas_no_overlap(start in 0usize..50_000usize) {
        let aligned_start = start / 4096 * 4096;
        let mut mgr = VmaManager::new();
        mgr.add(Vma::new(
            VirtAddr::new(aligned_start),
            VirtAddr::new(aligned_start + 4096),
            VmaFlags::new(),
        )).unwrap();
        mgr.add(Vma::new(
            VirtAddr::new(aligned_start + 4096),
            VirtAddr::new(aligned_start + 8192),
            VmaFlags::new(),
        )).unwrap();
        prop_assert_eq!(mgr.count(), 2);
    }

    /// INV-VMA-3: overlapping add is rejected
    #[test]
    fn test_overlap_rejected(start in 0usize..50_000usize) {
        let aligned_start = start / 4096 * 4096;
        let mut mgr = VmaManager::new();
        mgr.add(Vma::new(
            VirtAddr::new(aligned_start),
            VirtAddr::new(aligned_start + 8192),
            VmaFlags::new(),
        )).unwrap();
        let result = mgr.add(Vma::new(
            VirtAddr::new(aligned_start + 4096),
            VirtAddr::new(aligned_start + 12288),
            VmaFlags::new(),
        ));
        prop_assert!(result.is_err());
    }

    /// INV-VMA-4: find returns correct VMA for contained address
    #[test]
    fn test_find_contains(start in 4096usize..100_000usize) {
        let aligned_start = start / 4096 * 4096;
        let end = aligned_start + 4096;
        let vma = Vma::new(VirtAddr::new(aligned_start), VirtAddr::new(end), VmaFlags::from_bits(VmaFlags::READ));
        let mut mgr = VmaManager::new();
        mgr.add(vma).unwrap();

        let found = mgr.find(VirtAddr::new(aligned_start));
        prop_assert!(found.is_some());
        prop_assert_eq!(found.unwrap().start().as_usize(), aligned_start);

        let found_outside = mgr.find(VirtAddr::new(aligned_start + 4096));
        prop_assert!(found_outside.is_none());
    }

    /// INV-VMA-5: remove works correctly
    #[test]
    fn test_remove(start in 4096usize..100_000usize) {
        let aligned_start = start / 4096 * 4096;
        let mut mgr = VmaManager::new();
        mgr.add(Vma::new(
            VirtAddr::new(aligned_start),
            VirtAddr::new(aligned_start + 4096),
            VmaFlags::new(),
        )).unwrap();
        prop_assert_eq!(mgr.count(), 1);

        mgr.remove(VirtAddr::new(aligned_start)).unwrap();
        prop_assert_eq!(mgr.count(), 0);

        let result = mgr.remove(VirtAddr::new(aligned_start));
        prop_assert_eq!(result, Err(VmaError::NotFound));
    }

    /// INV-VMA-6: split divides VMA at address
    #[test]
    fn test_split(start in 4096usize..100_000usize) {
        let aligned_start = start / 4096 * 4096;
        let vma = Vma::new(
            VirtAddr::new(aligned_start),
            VirtAddr::new(aligned_start + 12288),
            VmaFlags::new(),
        );
        let split_addr = VirtAddr::new(aligned_start + 4096);
        let (first, second) = vma.split(split_addr).unwrap();

        prop_assert_eq!(first.start().as_usize(), aligned_start);
        prop_assert_eq!(first.end().as_usize(), aligned_start + 4096);
        prop_assert_eq!(second.start().as_usize(), aligned_start + 4096);
        prop_assert_eq!(second.end().as_usize(), aligned_start + 12288);
    }

    /// INV-VMA-7: contains checks address range correctly
    #[test]
    fn test_contains(start in 4096usize..100_000usize) {
        let aligned_start = start / 4096 * 4096;
        let vma = Vma::new(
            VirtAddr::new(aligned_start),
            VirtAddr::new(aligned_start + 4096),
            VmaFlags::new(),
        );
        prop_assert!(vma.contains(VirtAddr::new(aligned_start)));
        prop_assert!(!vma.contains(VirtAddr::new(aligned_start + 4096)));
        prop_assert!(!vma.contains(VirtAddr::new(aligned_start - 1)));
    }

    /// INV-VMA-8: overlaps detects overlapping VMAs
    #[test]
    fn test_overlaps(start in 4096usize..100_000usize) {
        let aligned_start = start / 4096 * 4096;
        let vma1 = Vma::new(
            VirtAddr::new(aligned_start),
            VirtAddr::new(aligned_start + 8192),
            VmaFlags::new(),
        );
        let vma2 = Vma::new(
            VirtAddr::new(aligned_start + 4096),
            VirtAddr::new(aligned_start + 12288),
            VmaFlags::new(),
        );
        prop_assert!(vma1.overlaps(&vma2));

        let vma3 = Vma::new(
            VirtAddr::new(aligned_start + 8192),
            VirtAddr::new(aligned_start + 12288),
            VmaFlags::new(),
        );
        prop_assert!(!vma1.overlaps(&vma3));
    }

    /// INV-VMA-9: can_merge and merge work correctly
    #[test]
    fn test_can_merge(start in 4096usize..100_000usize) {
        let aligned_start = start / 4096 * 4096;
        let flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE);
        let vma1 = Vma::new(
            VirtAddr::new(aligned_start),
            VirtAddr::new(aligned_start + 4096),
            flags,
        );
        let vma2 = Vma::new(
            VirtAddr::new(aligned_start + 4096),
            VirtAddr::new(aligned_start + 8192),
            flags,
        );
        prop_assert!(vma1.can_merge(&vma2));

        let vma3 = Vma::new(
            VirtAddr::new(aligned_start + 4096),
            VirtAddr::new(aligned_start + 8192),
            VmaFlags::from_bits(VmaFlags::READ),
        );
        prop_assert!(!vma1.can_merge(&vma3));
    }

    /// INV-VMA-10: iteration order is sorted by start address after arbitrary adds
    #[test]
    fn test_vma_sorted_after_adds(
        ranges in proptest::collection::vec(
            proptest::strategy::Just(()).prop_flat_map(|_| {
                let base = 0usize..100_000usize;
                let pages = 1usize..20usize;
                (base, pages).prop_map(|(s, p)| {
                    let aligned = s / 4096 * 4096;
                    (aligned, p * 4096)
                })
            }),
            0..50
        ),
    ) {
        let mut mgr = VmaManager::new();
        for (start, len) in &ranges {
            let vma = Vma::new(VirtAddr::new(*start), VirtAddr::new(start + len), VmaFlags::new());
            let _ = mgr.add(vma);
        }
        if mgr.count() < 2 {
            return Ok(());
        }
        let starts: Vec<usize> = mgr.iter().map(|v| v.start().as_usize()).collect();
        for w in starts.windows(2) {
            prop_assert!(w[0] < w[1],
                "VMAs not sorted: 0x{:x} >= 0x{:x}", w[0], w[1]);
        }
    }

    /// INV-VMA-11: no overlap after add+remove+add sequence
    #[test]
    fn test_no_overlap_after_add_remove(
        ranges in proptest::collection::vec(
            proptest::strategy::Just(()).prop_flat_map(|_| {
                let base = 0usize..200_000usize;
                let pages = 1usize..10usize;
                (base, pages).prop_map(|(s, p)| {
                    let aligned = s / 4096 * 4096;
                    (aligned, p * 4096)
                })
            }),
            1..30
        ),
    ) {
        let mut mgr = VmaManager::new();
        let mut added_starts: Vec<usize> = Vec::new();
        for (start, len) in &ranges {
            let vma = Vma::new(VirtAddr::new(*start), VirtAddr::new(start + len), VmaFlags::new());
            if mgr.add(vma).is_ok() {
                added_starts.push(*start);
            }
        }
        // Remove every other VMA
        let mut to_remove: Vec<usize> = added_starts.iter().step_by(2).copied().collect();
        for start in &to_remove {
            let _ = mgr.remove(VirtAddr::new(*start));
        }
        // Add some VMAs back (they may overlap with remaining ones — that's ok, just test non-overlap)
        for (start, len) in ranges.iter().take(5) {
            let vma = Vma::new(VirtAddr::new(*start), VirtAddr::new(start + len), VmaFlags::new());
            let _ = mgr.add(vma);
        }
        // Verify no two VMAs overlap
        let sorted: Vec<_> = mgr.iter().collect();
        for w in sorted.windows(2) {
            prop_assert!(w[0].end().as_usize() <= w[1].start().as_usize(),
                "overlap after ops: 0x{:x}-0x{:x} vs 0x{:x}-0x{:x}",
                w[0].start().as_usize(), w[0].end().as_usize(),
                w[1].start().as_usize(), w[1].end().as_usize());
        }
    }
}
