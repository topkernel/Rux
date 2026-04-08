//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for VMA non-overlap and sorted iteration.
//!
//! Types copied from: kernel/verify/src/mm/vma_test.rs

#![cfg(kani)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

pub const PAGE_SIZE: usize = 4096;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub usize);

impl VirtAddr {
    pub fn new(addr: usize) -> Self { Self(addr & !(PAGE_SIZE - 1)) }
    pub fn as_usize(&self) -> usize { self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaFlags(u32);
impl VmaFlags {
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 2;
    pub fn new() -> Self { Self(0) }
    pub fn bits(&self) -> u32 { self.0 }
}

#[derive(Debug, Clone, Copy)]
pub struct Vma {
    start: VirtAddr,
    end: VirtAddr,
    flags: VmaFlags,
}

impl Vma {
    pub fn new(start: VirtAddr, end: VirtAddr, flags: VmaFlags) -> Self {
        Self { start, end, flags }
    }
    pub fn start(&self) -> VirtAddr { self.start }
    pub fn end(&self) -> VirtAddr { self.end }
    pub fn overlaps(&self, other: &Vma) -> bool {
        self.start.as_usize() < other.end.as_usize()
            && other.start.as_usize() < self.end.as_usize()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaError { Overlap, NoSpace, NotFound, Invalid }

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
        if let Some((_, prev)) = self.vmas.range(..start).next_back() {
            if prev.end().as_usize() > start.as_usize() {
                return Err(VmaError::Overlap);
            }
        }
        if let Some((_, next)) = self.vmas.range(start..end).next() {
            return Err(VmaError::Overlap);
        }
        if end.as_usize() > self.max_end.as_usize() {
            self.max_end = end;
        }
        self.vmas.insert(start, vma);
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Vma> { self.vmas.values() }
    pub fn count(&self) -> usize { self.vmas.len() }
}

/// Helper: generate a page-aligned address for Kani with bounded range.
fn aligned_addr(base: usize) -> usize {
    (base / PAGE_SIZE) * PAGE_SIZE
}

/// INV-VMA-K1: Two non-overlapping adds both succeed.
#[kani::proof]
fn verify_vma_no_overlap() {
    let mut mgr = VmaManager::new();
    let gap: usize = kani::any();
    kani::assume(gap >= PAGE_SIZE && gap <= 128 * PAGE_SIZE);

    let vma1 = Vma::new(
        VirtAddr::new(4096),
        VirtAddr::new(4096 + PAGE_SIZE),
        VmaFlags::new(),
    );
    let vma2 = Vma::new(
        VirtAddr::new(4096 + PAGE_SIZE + gap),
        VirtAddr::new(4096 + 2 * PAGE_SIZE + gap),
        VmaFlags::new(),
    );
    assert!(mgr.add(vma1).is_ok());
    assert!(mgr.add(vma2).is_ok());
    assert_eq!(mgr.count(), 2);
}

/// INV-VMA-K2: Overlapping add is rejected.
#[kani::proof]
fn verify_vma_overlap_rejected() {
    let mut mgr = VmaManager::new();
    let offset: usize = kani::any();
    kani::assume(offset >= 1 && offset < PAGE_SIZE);

    let vma1 = Vma::new(
        VirtAddr::new(4096),
        VirtAddr::new(4096 + 2 * PAGE_SIZE),
        VmaFlags::new(),
    );
    assert!(mgr.add(vma1).is_ok());

    let vma2 = Vma::new(
        VirtAddr::new(4096 + PAGE_SIZE - offset),
        VirtAddr::new(4096 + 2 * PAGE_SIZE + PAGE_SIZE),
        VmaFlags::new(),
    );
    assert_eq!(mgr.add(vma2), Err(VmaError::Overlap));
}

/// INV-VMA-K3: Iteration yields VMAs sorted by start address.
#[kani::proof]
fn verify_vma_sorted() {
    let mut mgr = VmaManager::new();
    // Add 3 non-overlapping VMAs with symbolic gaps
    let gap1: usize = kani::any();
    let gap2: usize = kani::any();
    kani::assume(gap1 >= PAGE_SIZE && gap1 <= 64 * PAGE_SIZE);
    kani::assume(gap2 >= PAGE_SIZE && gap2 <= 64 * PAGE_SIZE);

    let start1 = 4096usize;
    let start2 = start1 + PAGE_SIZE + gap1;
    let start3 = start2 + PAGE_SIZE + gap2;

    let vma1 = Vma::new(VirtAddr::new(start1), VirtAddr::new(start1 + PAGE_SIZE), VmaFlags::new());
    let vma2 = Vma::new(VirtAddr::new(start2), VirtAddr::new(start2 + PAGE_SIZE), VmaFlags::new());
    let vma3 = Vma::new(VirtAddr::new(start3), VirtAddr::new(start3 + PAGE_SIZE), VmaFlags::new());

    assert!(mgr.add(vma1).is_ok());
    assert!(mgr.add(vma2).is_ok());
    assert!(mgr.add(vma3).is_ok());

    let starts: Vec<usize> = mgr.iter().map(|v| v.start().as_usize()).collect();
    for w in starts.windows(2) {
        assert!(w[0] < w[1], "VMAs not sorted: {} >= {}", w[0], w[1]);
    }
}
