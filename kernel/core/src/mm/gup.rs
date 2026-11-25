//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Pin user pages for direct I/O — the reference kernel's
//! `pin_user_pages` + `bio_iov_iter_get_pages` shape (GUP-DIO lane,
//! 2026-11-18; reference-audit missing-optimization B2).
//!
//! The audit's two-sided attribution put a +34.5 µs/op copyout on the
//! 1 MiB direct read and ~56 µs staging on the write side: every O_DIRECT
//! transfer staged through a kernel buffer (alloc + full-data copy in,
//! DMA, full-data copy out). The reference instead pins the user pages
//! backing the request and builds the bio segments directly from the
//! pinned frames — zero copies. This module is that pin:
//!
//! - [`pin_user_pages`] walks the CURRENT task's page tables
//!   (`PageTableWalker::walk` under the mm's PTE lock, INV-LOCK-7),
//!   faulting each page in first through the mm fault handler (the same
//!   discipline the ptrace uaccess path uses), and takes a page
//!   refcount on every page that resolves cleanly (`Page::get_page`).
//!   The refcount pin is what makes the frame DMA-safe: reclaim only
//!   takes exclusively-owned pages (vmscan's `refcount() == 1` gate),
//!   and an unmap racing the pin is serialized by the same PTE lock the
//!   unmap path takes (munmap unlinks under INV-LOCK-7).
//! - For write-side pins the fault is driven with WRITE (COW-resolved
//!   through `handle_cow_fault`, exactly the kernel-uaccess trap path),
//!   so the mapped frame is a private, writable instance: a later fork
//!   write-protects the mapping and any sharer's store COWs away from
//!   our frame — the DMA reads a stable snapshot, the reference's
//!   FOLL_WRITE semantics.
//! - A page that cannot be faulted/pinned (unmapped range, OOM,
//!   protection) is recorded as a HOLE in the result; the caller's
//!   per-page fallback discipline serves the bytes through the
//!   existing staged paths for exactly those pages. Pinning never
//!   fails a request that the staged path could have served.
//!
//! The frames are exposed to the block layer as kernel direct-map
//! aliases (`phys_to_virt`): `virt_to_phys` in the virtio submit path
//! translates them back to the user frames, and the existing SG chain
//! machinery (bio's WriteBatch / the batched direct reader) DMAs
//! straight into/out of user memory. The pin is dropped (put_page) at
//! request completion — `PinnedUserPages::drop`.

#![forbid(unsafe_code)]

use crate::mm::page_desc::{pfn_to_page, pfn_to_page_mut};
use crate::mm::PAGE_SIZE;

/// Pin of the user pages backing one byte range `[va, va+len)`.
///
/// `pages[i]` covers the i-th page of the range (page 0 starts at
/// `va & !(PAGE_SIZE-1)`); `None` marks a page that could not be pinned
/// — bytes of the range in such pages MUST be served through the
/// caller's staged fallback, never DMA'd.
pub struct PinnedUserPages {
    /// User VA of the first byte of the range.
    va: usize,
    /// Byte length of the range (>= 1; may end mid-page).
    len: usize,
    /// Per covered page: physical frame base (page-aligned), or None
    /// for an unpinnable page.
    pages: alloc::vec::Vec<Option<u64>>,
    /// Number of pages actually pinned (for accounting/asserts).
    pinned: usize,
}

impl PinnedUserPages {
    /// User VA of the first byte.
    pub fn va(&self) -> usize {
        self.va
    }

    /// Byte length of the pinned request.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Byte offset of the range start inside its first page.
    fn head_off(&self) -> usize {
        self.va & (PAGE_SIZE - 1)
    }

    /// Page index (into `pages`) covering request offset `off`.
    fn page_of(&self, off: usize) -> usize {
        (self.head_off() + off) / PAGE_SIZE
    }

    /// Kernel direct-map alias of the piece of page `pi` that intersects
    /// `[off, off+n)`, and the piece's length. `off..off+n` must lie
    /// within one page.
    fn piece(&self, pi: usize, off: usize, n: usize) -> (*mut u8, usize) {
        let start = self.head_off() + off;
        let in_page = start - pi * PAGE_SIZE;
        let frame = self.pages[pi].unwrap();
        let kva = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr(frame)).0 as usize;
        (kva.wrapping_add(in_page) as *mut u8, n)
    }

    /// Iterate the pinned pieces covering request range `[off, off+n)`:
    /// yields `(kernel_alias_ptr, piece_len)` for every pinned piece,
    /// skipping unpinned pages. A piece never crosses a page boundary.
    /// `visit` returning false stops the walk. Returns the number of
    /// PINNED bytes visited (callers use it to detect holes without a
    /// second pass).
    ///
    /// # Safety
    /// Callers must not hold the returned pointers beyond the pin's
    /// lifetime (the `PinnedUserPages` borrow).
    pub fn for_each_piece<F: FnMut(*mut u8, usize) -> bool>(
        &self,
        off: usize,
        n: usize,
        mut visit: F,
    ) -> usize {
        debug_assert!(off + n <= self.len);
        let end = off + n;
        let mut done = 0usize;
        let mut o = off;
        while o < end {
            let pi = self.page_of(o);
            // Request offset where the current page ends (clipped to
            // the range end): pieces never cross page boundaries.
            let page_first_off = pi * PAGE_SIZE - self.head_off();
            let o_page_end = (page_first_off + PAGE_SIZE).min(end);
            let piece_len = o_page_end - o;
            if self.pages[pi].is_some() {
                let (p, l) = self.piece(pi, o, piece_len);
                if !visit(p, l) {
                    return done;
                }
                done += l;
            }
            o = o_page_end;
        }
        done
    }

    /// True when every byte of `[off, off+n)` lies in pinned pages —
    /// the gate for a zero-copy DMA span.
    pub fn range_pinned(&self, off: usize, n: usize) -> bool {
        let p0 = self.page_of(off);
        let p1 = self.page_of(off + n - 1);
        self.pages[p0..=p1].iter().all(|p| p.is_some())
    }

    /// Next pinned piece at request offset `*off` (before `end`):
    /// returns `(alias_ptr, len)` and advances `*off` past the piece.
    /// Unpinned pages are skipped. CONSECUTIVE PHYSICALLY-ADJACENT
    /// pinned pages are merged into ONE piece — the SG segment count
    /// then tracks physical contiguity (a fresh buddy-backed 1 MiB
    /// window is a handful of segments, not 256), which is what keeps
    /// the descriptor footprint at the staging path's shape. Returns
    /// None when no pinned piece remains before `end`.
    ///
    /// # Safety
    /// Same lifetime contract as `for_each_piece`.
    pub fn next_piece(&self, off: &mut usize, end: usize) -> Option<(*mut u8, usize)> {
        // Skip unpinned pages.
        let mut o = *off;
        loop {
            if o >= end {
                return None;
            }
            let pi = self.page_of(o);
            if self.pages[pi].is_some() {
                break;
            }
            o = self.page_end_off(pi, end);
        }
        // First (possibly partial) page piece.
        let pi0 = self.page_of(o);
        let first_end = self.page_end_off(pi0, end);
        let (ptr, l0) = self.piece(pi0, o, first_end - o);
        let mut total = l0;
        let mut cur_phys = self.pages[pi0].unwrap();
        // Extend through whole pages that are pinned and physically
        // adjacent (direct-map aliases of adjacent frames are
        // contiguous, so the merged pointer covers the whole run).
        let mut o2 = first_end;
        while o2 < end {
            let pi = self.page_of(o2);
            let Some(next_phys) = self.pages[pi] else { break };
            if next_phys != cur_phys + PAGE_SIZE as u64 {
                break;
            }
            let pe = self.page_end_off(pi, end);
            cur_phys = next_phys;
            total += pe - o2;
            o2 = pe;
        }
        *off = o + total;
        Some((ptr, total))
    }

    /// Request offset where page `pi`'s intersection with `[.., end)`
    /// ends (pieces never cross page boundaries before merging).
    fn page_end_off(&self, pi: usize, end: usize) -> usize {
        let page_first_off = pi * PAGE_SIZE - self.head_off();
        (page_first_off + PAGE_SIZE).min(end)
    }

    /// Kernel alias of the first pinned piece at request offset `off`
    /// (the completion-token pointer for SG chains; `off` must be
    /// pinned).
    pub fn first_ptr(&self, off: usize) -> usize {
        let pi = self.page_of(off);
        self.piece(pi, off, 1).0 as usize
    }

    /// Copy `src` into the user frames at request offset `off`
    /// (read-side per-block serve and hole fills; unpinned pages are
    /// SKIPPED — the caller's fallback owns their bytes). All bytes
    /// must lie in pinned pages (check `range_pinned` first).
    pub fn copy_into_pinned(&self, off: usize, src: &[u8]) {
        let mut s = 0usize;
        self.for_each_piece(off, src.len(), |dst, l| {
            // The pin keeps the frame alive; the alias is a valid
            // kernel address for l bytes (INV-MM-6 pinned-alias arm).
            super::r#unsafe::copy_kva(dst, &src[s..][..l]);
            s += l;
            true
        });
        debug_assert_eq!(s, src.len());
    }

    /// Copy `n` bytes of the user frames at request offset `off` into
    /// `dst` (write-side per-block gather). Unpinned bytes read as
    /// ZERO — callers gate spans with `range_pinned` and serve
    /// unpinned blocks through the staged path instead.
    pub fn copy_from_pinned(&self, off: usize, dst: &mut [u8]) {
        let mut d = 0usize;
        self.for_each_piece(off, dst.len(), |src, l| {
            // Same alias validity; src is l readable bytes (INV-MM-6).
            super::r#unsafe::copy_kva_from(&mut dst[d..][..l], src);
            d += l;
            true
        });
        for b in dst[d..].iter_mut() {
            *b = 0;
        }
    }

    /// Number of pages pinned (holes excluded).
    pub fn pinned_pages(&self) -> usize {
        self.pinned
    }

    /// Number of pages the range covers (pinned + holes).
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Drop every pin. Idempotent (the drop guard runs it once).
    pub fn unpin(&mut self) {
        for p in self.pages.iter_mut() {
            if let Some(frame) = p.take() {
                // pfn came from the page-table walk of a mapped page;
                // each pin took exactly one reference (INV-MM-2).
                if let Some(page) = super::r#unsafe::page_of((frame >> 12) as usize) {
                    page.put_page();
                }
            }
        }
        self.pinned = 0;
    }
}

impl Drop for PinnedUserPages {
    fn drop(&mut self) {
        self.unpin();
    }
}

/// Pin the user pages backing `[va, va+len)` of the CURRENT task.
///
/// `write` selects the fault discipline: a write pin drives WRITE
/// faults (COW-resolved) and requires a writable PTE, so the pinned
/// frame is the mapper's private instance (the reference's FOLL_WRITE);
/// a read pin only requires a present PTE. Pages that cannot be
/// faulted or pinned become holes in the result — the call fails
/// outright (None) only when the task or its mm cannot be resolved.
///
/// The caller must have access_ok'd the range (the syscall layer's
/// contract) and must not run this under the ext4 big lock: the
/// fault-in path may allocate and take mm locks (see the module doc).
pub fn pin_user_pages(va: usize, len: usize, write: bool) -> Option<PinnedUserPages> {
    use crate::arch::mm::mm_ops::PageTableWalker;

    debug_assert!(len > 0);
    let task = crate::sched::current()?;
    let mm = task.address_space_arc()?;

    let first_page = va & !(PAGE_SIZE - 1);
    let last_page = (va + len - 1) & !(PAGE_SIZE - 1);
    let npages = (last_page - first_page) / PAGE_SIZE + 1;
    let mut pages: alloc::vec::Vec<Option<u64>> = alloc::vec::Vec::with_capacity(npages);

    // ---- Pass 1 (fast path): ONE pte-lock section, walk every page ----
    // The DIO buffers of interest (fio's aligned 1 MiB windows) are
    // long-resident: the walk resolves them and the fault machinery
    // never runs. Pages that come back unwalkable here get Pass 2's
    // fault-in (below). One lock acquisition for the whole range —
    // the per-page lock/unlock pair of the first cut measured ~100 µs
    // per pinned MiB (256 pairs), the dominant GUP cost after the
    // copy savings.
    {
        use crate::arch::mm::PageTableEntry;
        let root = mm.root_ppn();
        let guard = mm.pte_lock().lock();
        for pi in 0..npages {
            let page_va = first_page + pi * PAGE_SIZE;
            // root is the pinned mm's live root; the walker
            // bounds-checks the root and every intermediate entry
            // (INV-SY-1 walk edge).
            let r = crate::sync::r#unsafe::pt_walk(root, page_va as u64);
            pages.push(match r {
                // Present but NOT writable = a COW-shared leaf (fork's
                // write-protected mapping). Pinning it directly would
                // alias the DMA with the fork child (reads into the
                // shared frame would change the child's buffer — the
                // reference COW-resolves before pinning, and so do we):
                // leave it to Pass 2, which drives the WRITE fault.
                // (A genuinely read-only VMA fails the fault there and
                // stays a hole — the staged fallback's semantics.)
                Some((ppn, bits)) if bits & PageTableEntry::W != 0 => {
                    // Descriptor of a machine frame (vmemmap-published);
                    // mapped frame (refcount >= 1) — one reference,
                    // dropped in unpin() (INV-MM-2).
                    match super::r#unsafe::page_of(ppn as usize) {
                        Some(page) => {
                            page.get_page();
                            Some(ppn << 12)
                        }
                        None => None,
                    }
                }
                _ => None,
            });
        }
        drop(guard);
    }

    // ---- Pass 2: fault-in + re-pin ONLY the holes ----
    // (Missing pages AND COW leaves — the reference's FOLL_WRITE
    // discipline applies to both DMA directions: a read DIO WRITES the
    // frames, so they must be the mapper's private instance too.)
    let _ = write;
    for pi in 0..npages {
        if pages[pi].is_some() {
            continue;
        }
        let page_va = first_page + pi * PAGE_SIZE;
        pages[pi] = pin_one_faulted(&mm, page_va);
    }

    let pinned = pages.iter().filter(|p| p.is_some()).count();
    Some(PinnedUserPages {
        va,
        len,
        pages,
        pinned,
    })
}

/// Fault a page in and pin it (the Pass-2 body for pages the walk
/// missed: demand-fault anon/file pages, COW entries). The fault is
/// driven as a WRITE fault — the pin is a DMA target (read DIO) or a
/// stable snapshot source (write DIO), and both demand the COW-resolved
/// private instance (FOLL_WRITE semantics); a genuinely read-only VMA
/// fails the fault and the page stays a hole (staged fallback).
fn pin_one_faulted(
    mm: &crate::mm::mm_struct::MmStruct,
    page_va: usize,
) -> Option<u64> {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::{handle_cow_fault, CowFaultResult, FaultFlags, MmFaultResult};

    let flags = FaultFlags::WRITE | FaultFlags::USER;
    for _ in 0..2 {
        match handle_mm_fault_checked(mm, page_va, flags) {
            MmFaultResult::Handled
            | MmFaultResult::Fixed
            | MmFaultResult::AlreadyMapped => break,
            MmFaultResult::CowPending => {
                // mm owns root_ppn; handle_cow_fault is a safe fn and
                // runs under the PTE section itself (INV-LOCK-7) — the
                // old wrapper was vestigial.
                let r = handle_cow_fault(
                    mm,
                    mm.root_ppn(),
                    crate::arch::mm::VirtAddr::new(page_va as u64),
                );
                match r {
                    CowFaultResult::Resolved | CowFaultResult::Retry => continue,
                    CowFaultResult::OutOfMemory => return None,
                }
            }
            // Segfault/PermissionDenied/OOM/BusError: leave the page a
            // hole — the staged fallback serves it (or reports the
            // same error it would have).
            _ => return None,
        }
    }

    // ---- Walk + pin under the PTE lock ----
    // The lock serializes against munmap's unlink and vmscan's
    // leaf_pte_replace; a present leaf here stays backed by its frame
    // until our get_page lands.
    //
    // Write-pin note: a WRITE fault that resolves to a read-only leaf
    // (a PROT_READ mapping) still pins the mapped frame — it is a
    // stable DMA SOURCE (a read-only mapping has no writable alias a
    // fork could COW away from). The reference's FOLL_WRITE would
    // fail here; we keep the staged path's more permissive semantics
    // instead of changing observable behavior.
    let root = mm.root_ppn();
    let guard = mm.pte_lock().lock();
    // root is the pinned mm's live root; the walker bounds
    // checks the root and every intermediate entry (INV-SY-1 walk
    // edge).
    let (ppn, _bits) = crate::sync::r#unsafe::pt_walk(root, page_va as u64)?;
    // Descriptor of a machine frame (vmemmap-published); the frame is
    // mapped (refcount >= 1) — the pin takes one reference, dropped in
    // unpin() (INV-MM-2).
    let page = super::r#unsafe::page_of(ppn as usize)?;
    page.get_page();
    drop(guard);
    Some(ppn << 12)
}

/// handle_mm_fault wrapper keeping the arch re-export surface uniform.
#[inline]
fn handle_mm_fault_checked(
    mm: &crate::mm::mm_struct::MmStruct,
    va: usize,
    flags: u32,
) -> crate::arch::mm::MmFaultResult {
    crate::api::mm::handle_mm_fault(mm, crate::arch::mm::VirtAddr::new(va as u64), flags)
}
