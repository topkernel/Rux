//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Memory Management Operations
//!
//! This module contains:
//! - MmStruct extension methods for address space management
//! - User space mapping operations
//! - Copy-on-Write (COW) support
//! - mmap/munmap implementations
//!
//! # Safety Invariants — COW Page Table Protocol
//!
//! - **INV-COW-1**: When the COW bit is set in a PTE, the PTE `W` (write) bit
//!   must be clear. The page is shared read-only.
//!
//! - **INV-COW-2**: When the COW bit is set, the page's `_refcount >= 2`
//!   (shared by at least two mappings, e.g. parent + child after fork).
//!
//! - **INV-COW-3**: On a COW fault, if `_refcount == 1` (all other sharers
//!   have released), restore the `W` bit directly without copying.
//!
//! - **INV-COW-4**: On a COW fault, if `_refcount > 1`, allocate a new page,
//!   copy contents, decrement the old page's refcount, and map the new page
//!   with `W=1, COW=0`.
//!
//! - **INV-COW-5**: After `fork`, every writable PTE in the child must be
//!   downgraded to read-only (`W=0`) with `COW=1` set, and the page's
//!   refcount incremented.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;
use core::sync::atomic::{fence, Ordering};

extern crate alloc;
use alloc::vec::Vec;

use super::memory_layout::*;
use super::unsafe_edges::PtTable;
use super::mmu_init::*;
use super::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::vma::{Vma, VmaFlags, VmaOrigin, VmaType};
use crate::mm::{MmStruct, alloc_pages, GfpFlags};

// Re-export AddressSpace for backward compatibility
pub use crate::mm::AddressSpace;

// ==================== MmStruct Extension Methods ====================

impl MmStruct {




    // ==================== VMA Operations ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages.
    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();

        let start = vma.start();
        let end = vma.end();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // Update virtual memory statistics
        let size = end.as_usize().saturating_sub(start.as_usize());
        let pages = (size / PAGE_SIZE_USIZE) as u64;
        self.add_total_vm(pages);
        self.update_highest_vm_end(end.as_usize());

        Ok(())
    }


    /// Unmap VMA (requires write lock)
    pub fn unmap_vma(&self, start: PageVirtAddr) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();

        let _vma = vma_mgr.find(start).ok_or(MapError::NotMapped)?;
        let _ = vma_mgr.remove(start);
        Ok(())
    }


    /// mmap system call implementation
    pub fn mmap(
        &self,
        addr: PageVirtAddr,
        size: usize,
        flags: VmaFlags,
        vma_type: VmaType,
        perm: Perm,
        map_flags: u32,
    ) -> Result<PageVirtAddr, MapError> {
        use super::memory_layout::map;

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        // MAP_FIXED_NOREPLACE implies exact placement like MAP_FIXED (the
        // EEXIST overlap check happens in sys_mmap before we get here).
        let is_fixed = map_flags
            & (map::MAP_FIXED | map::MAP_FIXED_NOREPLACE)
            != 0;

        let end_addr = addr.as_usize() + aligned_size;
        // Linux places MAP_FIXED anywhere in the user range, unmapping
        // whatever is covered. The historical blanket refusal
        // (addr < MMAP_START && end > BRK_DEFAULT) rejected EVERY fixed
        // mapping between 512MB and MMAP_START — mid-low placements
        // Linux allows (0x50000000-class: JVM compressed oops, wine,
        // LTP shapes) all returned EINVAL (session-14/15 probe, x86_64
        // twin MF1-3). Refuse only the LIVE brk heap span
        // [align_down(start_brk), align(brk)): mapping over malloc's
        // own storage stays EINVAL (deliberate divergence — Linux would
        // unmap the heap; we never silently free brk-managed pages).
        // Everything above the live heap is safe: the fixed-path surgery
        // splits/zaps/replaces whatever VMA+PTE state exists, brk growth
        // into foreign VMAs is refused by sys_brk's VmaManager overlap
        // check, and the mm.brk mirror is synced by sys_brk so this
        // span tracks the real break.
        let brk_cur = self.brk().as_usize();
        let heap_lo = self.start_brk() & !(PAGE_SIZE_USIZE - 1);
        let heap_hi = if brk_cur > self.start_brk() {
            (brk_cur + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1)
        } else {
            heap_lo
        };
        let has_brk_conflict = addr.as_usize() < heap_hi && end_addr > heap_lo;

        // See the non-fixed branch below: placement + VMA insert share one
        // write-lock section (atomic pick-and-insert).
        let mut vma_installed = false;
        let start = if is_fixed {
            let start = addr;
            if start.as_usize() % PAGE_SIZE_USIZE != 0 {
                return Err(MapError::Invalid);
            }
            if start.as_usize() < user_addr::USER_START {
                return Err(MapError::Invalid);
            }
            // Upper bound: a fixed mapping at or above USER_END would write
            // leaf PTEs into the kernel-shared L1/L0 tables (user page
            // tables copy the kernel PGD entries) — reject outright.
            if start
                .as_usize()
                .checked_add(aligned_size)
                .map_or(true, |e| e > user_addr::USER_END)
            {
                return Err(MapError::Invalid);
            }
            if has_brk_conflict {
                return Err(MapError::Invalid);
            }
            start
        } else {
            // ATOMIC find + insert: pick the address AND install the VMA
            // under ONE vma_write guard (x86_64 twin of the TOCTOU that
            // let two threads of one mm receive the same free address and
            // silently replace each other's VMA — cross-task heap
            // corruption; root-caused on x86_64 via heapstress, see
            // arch/x86_64/mm/mm_ops.rs mmap).
            //
            // Hint policy (session-17 parity with the x86_64 twin):
            // Linux honors a free hint ANYWHERE in the user range
            // (arch_get_unmapped_area): the hint is rounded DOWN and used
            // when its gap is free, otherwise the kernel-chosen search
            // runs. The historical blanket discard of hints below
            // MMAP_START is gone. Like Linux the hint is IGNORED (never
            // an error) when it is below MIN_MAP_ADDR (mmap_min_addr
            // analog), lands on the LIVE brk heap span (ours can be
            // mirror-only before the first growth), or ends past
            // USER_END (the syscall layer pre-checked the raw length;
            // rounding the length UP can still push the last page over).
            // Residual divergence, documented: no stack-guard-gap
            // reservation below GROWSDOWN VMAs (Linux vm_start_gap
            // reserves 1MB) — this VMA has no guard-gap model. The
            // round-down also closes a kernel panic vector: an unaligned
            // hint used to reach Vma::new and trip its page-alignment
            // assert.
            let mut vma_mgr = self.vma_write();
            let chosen = if addr.as_usize() != 0 {
                let hint = addr.as_usize() & !(PAGE_SIZE_USIZE - 1);
                let hint_end = hint.checked_add(aligned_size);
                let hint_heap_conflict =
                    hint < heap_hi && hint_end.map_or(true, |e| e > heap_lo);
                let hint_ok = hint >= user_addr::MIN_MAP_ADDR
                    && hint_end.map_or(false, |e| e <= user_addr::USER_END)
                    && !hint_heap_conflict;
                let mut use_hint = false;
                if hint_ok {
                    let test_vma = Vma::new(
                        PageVirtAddr::new(hint),
                        PageVirtAddr::new(hint + aligned_size),
                        flags,
                    );
                    let has_vma_conflict =
                        vma_mgr.iter().any(|v| v.overlaps(&test_vma));
                    use_hint = !has_vma_conflict;
                }
                if use_hint {
                    PageVirtAddr::new(hint)
                } else {
                    self.find_free_area_locked(&vma_mgr, aligned_size)?
                }
            } else {
                self.find_free_area_locked(&vma_mgr, aligned_size)?
            };
            let vend = PageVirtAddr::new(chosen.as_usize() + aligned_size);
            let mut vma = Vma::new(chosen, vend, flags);
            vma.set_type(vma_type);
            vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
            vma_mgr
                .add(vma)
                .map_err(|_| MapError::Invalid)?;
            drop(vma_mgr);
            self.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
            self.update_highest_vm_end(chosen.as_usize() + aligned_size);
            vma_installed = true;
            chosen
        };

        if is_fixed {
            // Linux semantics: MAP_FIXED replaces only the covered range.
            // Overlapping VMAs are SPLIT — their non-overlapping head/tail
            // must survive with type/flags/fd/offset preserved. Removing
            // whole overlapping VMAs destroyed the rest of the span when a
            // dynamic linker mapped a DSO's whole range and then overlaid
            // the individual segments with MAP_FIXED (glibc
            // _dl_map_object_from_fd): every page outside the final
            // segment lost its VMA and faulted with SIGSEGV (Ubuntu
            // dash/libc load).
            //
            // Whole-range teardown via the CHUNKED munmap loop (the same
            // bounded-hold discipline as sys_munmap — see munmap): the
            // old shape held vma_write across the ENTIRE fixed range's
            // surgery AND PTE-zap walk, the same whole-range VMA-lock
            // convoy shape munmap had. A PROT_NONE reserve carve over
            // empty space walked every page of the range under the write
            // lock even with zero overlapping VMAs; the chunked loop
            // exits immediately when nothing overlaps. Split head/tail
            // preservation and pin (vm_file) discipline are munmap's own.
            //
            // ATOMICITY (mm-audit, OH stack-VMA-vanishing): the old
            // single guard made surgery+zap+install one unit; the chunked
            // shape keeps every intermediate state CONSISTENT instead of
            // atomic: at every guard drop the range is either untouched
            // (VMA + PTEs live) or fully cleared (no VMA, no PTEs). The
            // install re-checks under its own guard; a same-mm racer that
            // lands in the cleared gap between teardown and install is
            // cleared by one bounded retry (a legal linearization for
            // concurrent mmap/MAP_FIXED on the same range). The b89
            // piece-drop hole cannot recur: pieces are never held across
            // a guard drop by the chunked loop.
            let mut attempts = 0;
            loop {
                self.munmap(start, aligned_size)?;
                let end = PageVirtAddr::new(start.as_usize() + aligned_size);
                let mut vma_mgr = self.vma_write();
                let test_vma = Vma::new(start, end, flags);
                if vma_mgr.iter().any(|v| v.overlaps(&test_vma)) {
                    // a same-mm racer mapped into the gap after the last
                    // teardown pass; drop the guard and clear it too
                    drop(vma_mgr);
                    attempts += 1;
                    if attempts >= 8 {
                        return Err(MapError::OutOfMemory);
                    }
                    continue;
                }
                let mut vma = Vma::new(start, end, flags);
                vma.set_type(vma_type);
                vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
                vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;
                drop(vma_mgr);
                self.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
                self.update_highest_vm_end(end.as_usize());
                vma_installed = true;
                break;
            }
        }

        if !vma_installed {
            // Unreachable today (every non-fixed path installs inline);
            // kept as a safety net for future placement paths.
            let end = PageVirtAddr::new(start.as_usize() + aligned_size);
            let mut vma = Vma::new(start, end, flags);
            vma.set_type(vma_type);
            vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
            self.map_vma(vma, perm)?;
        }
        Ok(start)
    }

    /// Find free virtual address area
    pub fn find_free_area(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let vma_mgr = self.vma_read();
        self.find_free_area_locked(&vma_mgr, size)
    }

    /// Scan body of find_free_area under a lock the CALLER holds (read
    /// for the wrapper, write for mmap's atomic pick-and-insert).
    pub(crate) fn find_free_area_locked(
        &self,
        vma_mgr: &crate::mm::vma::VmaManager,
        size: usize,
    ) -> Result<PageVirtAddr, MapError> {
        use user_addr::{MMAP_START, MMAP_END, USER_END};

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        let mut search_start = MMAP_START;
        let search_end = MMAP_END.min(USER_END - aligned_size);

        for vma in vma_mgr.iter() {
            let vma_start = vma.start().as_usize();

            if vma_start > search_start {
                let gap_size = vma_start - search_start;
                if gap_size >= aligned_size {
                    return Ok(PageVirtAddr::new(search_start));
                }
            }

            if vma.end().as_usize() > search_start {
                search_start = (vma.end().as_usize() + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
            }

            if search_start > search_end {
                break;
            }
        }

        if search_start <= search_end && (search_end - search_start) >= aligned_size {
            return Ok(PageVirtAddr::new(search_start));
        }

        Err(MapError::OutOfMemory)
    }

    /// munmap system call implementation — supports partial VMA removal:
    /// split VMAs at the unmapped boundaries, remove the overlapping parts
    /// (review ARCH-M3 / SYSA-H1: previously a sub-VMA munmap returned
    /// EINVAL and a cross-VMA range only removed the first VMA).
    pub fn munmap(&self, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // s15 catch (ltp-a r22, madvise02 sweep): the page-round add
        // `(size + PAGE_SIZE - 1)` OVERFLOWS for user lengths within one
        // page of usize::MAX — munmap(0, 0xfffffffffffff000) panicked the
        // DEBUG kernel right at this line (release wrapped, then the R7-1
        // bound caught the garbage by accident). Linux returns EINVAL via
        // its len > TASK_SIZE - addr check; the same semantics here: the
        // wrap class is unreachable user ranges, reject as Invalid. The
        // s12 fix covered seven round-ups in syscall/memory.rs; munmap
        // rounds here, one layer deeper.
        let aligned_size = match size.checked_add(PAGE_SIZE_USIZE - 1) {
            Some(rounded) => rounded & !(PAGE_SIZE_USIZE - 1),
            None => return Err(MapError::Invalid),
        };

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        // R7-1: user-range bound — a kernel-range munmap would walk the
        // SHARED kernel PGD entries, put_page/rmap kernel page descriptors
        // and zero shared kernel PTEs (NEW-C1 class; covers sys_munmap and
        // madvise(MADV_REMOVE), which both funnel through here).
        {
            use super::memory_layout::user_addr;
            let end_checked = match addr.as_usize().checked_add(aligned_size) {
                Some(e) => e,
                None => return Err(MapError::Invalid),
            };
            if addr.as_usize() < user_addr::USER_START || end_checked > user_addr::USER_END {
                return Err(MapError::Invalid);
            }
        }

        let addr_us = addr.as_usize();
        let end_addr = addr_us + aligned_size;

        // VMA-lock convoy bound (twin of the x86_64 fix; the OH
        // service-graph crawl): the old shape collected the surgery for the
        // WHOLE range and ran the entire PTE-zap walk under ONE
        // vma_write() hold — a multi-MB unmap parked every same-mm VMA
        // user (sys_brk above all: VMA-surgery only, so it waits on
        // exactly this lock) for the full zap PLUS whatever scheduler
        // starvation the holder suffered while holding (the 100s-class
        // brk convoy). The reference kernel bounds its equivalent hold by
        // detaching per-VMA and keeping its zap loops resched-friendly.
        // Here the surgery is CHUNKED: each iteration removes at most
        // MUNMAP_CHUNK_BYTES of ONE VMA — that piece's surgery AND its
        // PTE zap still share ONE guard (mm-audit rule below) — then the
        // guard is dropped and cond_resched() yields the CPU; the tree
        // walk is RE-DERIVED from scratch each iteration because the
        // tree can change under us between holds.
        //
        // mm-audit (unchanged invariant): a racing mmap may only ever
        // land in a FULLY-CONSISTENT hole. At every guard drop, every
        // page in the munmap range is either (VMA present AND PTEs
        // live) — the unprocessed piece — or (VMA gone AND PTEs
        // cleared) — a processed chunk. Releasing the guard with live
        // PTEs under a VMA-free hole let a racing mmap's demand-fault
        // installs interleave with the zap walk (silently zeroed or
        // half-torn fresh mappings); the per-chunk surgery+zap pairing
        // keeps the two from ever parting. unmap_pages takes no VMA lock
        // (PTE lock only, nested one-way), so the hold is safe.
        //
        // CURSOR (89th session, Face-A closure — twin of the x86_64 fix):
        // the search starts at the CURSOR (the end of the last processed
        // piece), never back at addr_us. Re-deriving from addr_us let a
        // fresh same-mm mmap that legally landed in an already-processed
        // hole be found by the NEXT iteration's search and deleted as an
        // "unprocessed piece" of THIS munmap — the fresh VMA vanished
        // with its pages freed under it.
        //
        // Pin-table discipline (vm_file semantics): a split FILE VMA's
        // tail starts at a new address, so the pin keyed at the original
        // start no longer resolves for it — re-pin the tail at its own
        // start and drop the original key (vm_file follows the VMA, not
        // the address).
        const MUNMAP_CHUNK_BYTES: usize = 256 * PAGE_SIZE_USIZE;

        let mut cur = addr_us;
        loop {
            if cur >= end_addr {
                break;
            }
            let mut vma_mgr = self.vma_write();

            // First VMA overlapping [cur, end_addr): the one CONTAINING
            // cur, else the first starting inside the remaining range.
            // Re-derived every iteration — but always FORWARD from the
            // cursor, never from addr_us (Face A).
            let hit = vma_mgr
                .last_at_or_before(PageVirtAddr::new(cur))
                .filter(|v| v.end().as_usize() > cur)
                .or_else(|| vma_mgr.find_vma_after(PageVirtAddr::new(cur)))
                .filter(|v| v.start().as_usize() < end_addr);
            let Some(vma) = hit else {
                break;
            };

            let vkey = vma.start();
            let vs = vma.start().as_usize();
            let ve = vma.end().as_usize();
            let vflags = vma.flags();
            let vtype = vma.vma_type();
            let vorigin = vma.origin();
            let vfd = vma.file_fd();
            let vfsz = vma.file_size();
            let voff = vma.offset();

            let seg_start = vs.max(cur);
            let seg_end = ve.min(end_addr);
            let chunk_end = seg_start + MUNMAP_CHUNK_BYTES.min(seg_end - seg_start);

            // The pages of this piece leave the mapping: release their
            // virtual-size charge. Preserved pieces stay charged.
            self.release_unmapped_pages(vs, ve, seg_start, chunk_end);

            let pinned = self.get_vma_file(vs);
            let _ = vma_mgr.remove(vkey);
            self.unpin_vma_file(vs);
            if vs < seg_start {
                let mut head = Vma::new(
                    crate::mm::page::VirtAddr::new(vs),
                    crate::mm::page::VirtAddr::new(seg_start),
                    vflags,
                );
                head.set_type(vtype);
                // Split-preserved piece: keep the original origin.
                head.set_origin(vorigin);
                head.set_file_fd(vfd);
                head.set_file_size(vfsz);
                head.set_offset(voff);
                let _ = vma_mgr.add(head);
                if let Some(f) = pinned.clone() {
                    self.pin_vma_file(vs, f);
                }
            }
            if chunk_end < ve {
                let mut tail = Vma::new(
                    crate::mm::page::VirtAddr::new(chunk_end),
                    crate::mm::page::VirtAddr::new(ve),
                    vflags,
                );
                tail.set_type(vtype);
                // Split-preserved piece: keep the original origin.
                tail.set_origin(vorigin);
                tail.set_file_fd(vfd);
                tail.set_file_size(vfsz);
                tail.set_offset(voff + (chunk_end - vs));
                let _ = vma_mgr.add(tail);
                if let Some(f) = pinned.clone() {
                    self.pin_vma_file(chunk_end, f);
                }
            }

            self.unmap_pages(PageVirtAddr::new(seg_start), chunk_end - seg_start)?;

            // Drop the guard BEFORE yielding: the write lock must never
            // be held across schedule().
            drop(vma_mgr);
            crate::sched::cond_resched();
            // Face-A cursor: advance past the piece just processed so a
            // fresh mmap landing in its (now fully consistent) hole is
            // never revisited by this munmap.
            cur = chunk_end;
        }

        Ok(())
    }

    /// Zap the mapped pages in [start, start+size) WITHOUT touching the
    /// VMAs — madvise(MADV_DONTNEED) semantics. The next fault on the range
    /// re-materializes a zero page (anonymous VMAs) or re-reads the file
    /// page (file-backed VMAs). Reuses unmap_pages so the walk/rmap/
    /// refcount/clear_pte/sfence teardown happens under PTE_MODIFY_LOCK.
    pub fn zap_page_range(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if start.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }
        if start.as_usize().checked_add(aligned_size).is_none() {
            return Err(MapError::Invalid);
        }
        self.unmap_pages(start, aligned_size)
    }

    /// Legacy munmap body kept for reference — the new implementation above
    /// replaces it. Previously only whole-VMA removal was supported.
    #[allow(dead_code)]
    pub(crate) fn munmap_legacy(&self, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        let end_addr = addr.as_usize() + aligned_size;

        {
            let vma_mgr = self.vma_read();

            let vma_info = vma_mgr.find(addr).map(|vma| {
                (vma.start(), vma.end())
            });
            drop(vma_mgr);

            if let Some((vma_start, vma_end)) = vma_info {
                let vma_start_usize = vma_start.as_usize();
                let vma_end_usize = vma_end.as_usize();

                if addr.as_usize() <= vma_start_usize && end_addr >= vma_end_usize {
                    let mut vma_mgr = self.vma_write();
                    vma_mgr.remove(vma_start)?;
                } else if addr.as_usize() > vma_start_usize && end_addr < vma_end_usize {
                    return Err(MapError::Invalid);
                } else {
                    return Err(MapError::Invalid);
                }
            }
        }

        self.unmap_pages(addr, aligned_size)?;

        Ok(())
    }


    /// Teardown of ONE present user leaf at `va`, for callers that already
    /// hold PTE_MODIFY_LOCK (mremap's page-move copy). Mirrors the per-page
    /// body of unmap_pages: kernel-only leaves (no U bit) and device
    /// frames are left untouched; a normal user leaf gets rmap removal,
    /// the adjudicated put_page release (last ref frees) and its PTE
    /// cleared. Returns true if a leaf was torn down.
    ///
    /// Used by copy_old_to_new_pages when the mremap destination holds a
    /// page mapped by a racing same-mm fault between the destination VMA's
    /// MAP_FIXED install and the PTE copy — Linux's mremap moves PTEs
    /// atomically under mmap_lock and the move REPLACES whatever sits in
    /// the destination; skipping the page (the old MREMAP-DST-LIVE guard)
    /// permanently lost the source data.
    pub fn unmap_one_user_leaf_locked(&self, va: usize) -> bool {
        let Some((ppn_val, pte_bits)) = PageTableWalker::walk(self.pgd, va as u64) else {
            return false;
        };
        if pte_bits & PageTableEntry::U == 0 {
            return false; // kernel translation overlapped in the walk
        }
        // Device frames (virtio-gpu framebuffer) are not RAM pages, and
        // neither is any other boot-reserved (kernel-heap) frame — the
        // drm GEM dumb-buffer backing. Clear the PTE only.
        {
            let phys = (ppn_val as u64) << 12;
            let fb = crate::drivers::gpu::get_framebuffer_info()
                .map_or(false, |i| phys >= i.addr && phys < i.addr + i.size as u64);
            if fb || crate::mm::page_alloc::is_foreign_user_frame(phys as usize) {
                self.clear_pte(va as u64);
                return true;
            }
        }
        use crate::mm::page_desc::page_desc;
        if let Some(page) = page_desc(ppn_val as usize) {
            page.rmap_alt_remove(
                self as *const _ as usize,
                va / PAGE_SIZE_USIZE,
            );
            // SIGILL-stamp adjudicated release (same as unmap_pages).
            crate::arch::mm::mmu_init::release_user_leaf(
                page,
                ppn_val as usize,
                va,
            );
            // PCACHE mapping: drop the pin after the descriptor put (same
            // ordering as unmap_pages; x86_64 twin).
            if pte_bits & cow_flags::PCACHE != 0 {
                release_cache_mapping_pin(ppn_val as usize);
            }
        }
        self.clear_pte(va as u64);
        self.sub_rss(1);
        true
    }

    /// Clear page table entry at specified virtual address
    pub(crate) fn clear_pte(&self, virt: u64) {
        let vpn2 = ((virt >> 30) & 0x1FF) as usize;
        let vpn1 = ((virt >> 21) & 0x1FF) as usize;
        let vpn0 = ((virt >> 12) & 0x1FF) as usize;

        let Some(root_table) = PtTable::from_phys(self.pgd << PAGE_SHIFT) else {
            return;
        };
        let pte2 = root_table.get(vpn2);
        if !pte2.is_valid() {
            return;
        }

        let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
            return;
        };
        let pte1 = table1.get(vpn1);
        // A still-megapaged L1 leaf means no user page was ever mapped at
        // this VA (user maps over a megapage demote it first) — nothing to
        // clear, and treating the leaf's frame as an L0 table would write
        // into MMIO memory.
        if !pte1.is_valid() || pte1.is_leaf() {
            return;
        }

        let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
            return;
        };

        // E8-MM: if this VA sits inside a kernel low-half device window
        // (PLIC/ECAM/PCI-MMIO/UART/...), the PTE being cleared may cover
        // a device page the user mapped over (megapage demotion made that
        // possible — Linux MAP_FIXED replace semantics). RESTORE the
        // device translation instead of writing 0: the kernel accesses
        // these registers from traps on the CURRENT satp (PLIC claim in
        // IRQ entry), and a hole here is an S-mode page fault —
        // KERNPANIC. Ordinary user VAs (the None case) clear to 0 as
        // before.
        let new_bits = super::mmu_init::kernel_device_window_pte(virt).unwrap_or(0);
        table0.set(vpn0, PageTableEntry::from_bits(new_bits));
    }


    /// brk system call implementation (legacy interface)
    pub fn do_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        self.set_brk(new_brk)
    }

    /// Allocate stack space
    pub fn allocate_stack(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let stack_size = if size == 0 {
            user_addr::STACK_MAX_SIZE
        } else {
            size
        };
        let aligned_size = (stack_size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        let stack_top = PageVirtAddr::new(user_addr::STACK_TOP & !(PAGE_SIZE_USIZE - 1));
        let stack_start = PageVirtAddr::new(stack_top.as_usize() - aligned_size);

        let mut flags = VmaFlags::new();
        flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN);
        let mut vma = Vma::new(stack_start, stack_top, flags);
        vma.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC_STACK));
        self.map_vma(vma, Perm::ReadWrite)?;

        self.setup_stack(stack_top.as_usize(), stack_size);

        Ok(stack_top)
    }

}

/// Convert permission to page table flags
pub(crate) fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
    let mut flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    match perm {
        Perm::None => {
            flags |= PageTableEntry::R;
        }
        Perm::Read => {
            flags |= PageTableEntry::R;
        }
        Perm::ReadWrite => {
            flags |= PageTableEntry::R | PageTableEntry::W;
        }
        Perm::ReadWriteExec => {
            flags |= PageTableEntry::R | PageTableEntry::W | PageTableEntry::X;
        }
        Perm::ReadExec => {
            flags |= PageTableEntry::R | PageTableEntry::X;
        }
        Perm::Exec => {
            flags |= PageTableEntry::X;
        }
    }
    if space_type == PageTableType::User {
        flags |= PageTableEntry::U;
    }
    flags
}

// ==================== User Address Space Management ====================

pub(crate) struct PageTableWalker;

impl PageTableWalker {
    /// Walk page table to find physical page number for virtual address
    /// Returns (ppn, full_pte_value) for debugging
    /// Handles both 4KB pages and 2MB huge pages
    pub(crate) fn walk(user_root_ppn: u64, virt: u64) -> Option<(u64, u64)> {
        let virt_addr = VirtAddr::new(virt);

        let vpn2 = virt_addr.vpn(2) as usize;
        let vpn1 = virt_addr.vpn(1) as usize;
        let vpn0 = virt_addr.vpn(0) as usize;

        // Root guard (x86_64 twin): a caller whose mm was freed under it
        // (unpinned procfs borrow) feeds a garbage root ppn — bail with a
        // bounded forensic line instead of dying in phys_to_virt OVERFLOW.
        {
            let root_pa = user_root_ppn << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published()
                && root_pa != 0
                && !crate::mm::phys_valid(root_pa as usize)
            {
                static ROOT_STALE: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if ROOT_STALE.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "PT-WALK-STALE: root={:#x} va={:#x} ROOT phys {:#x} off-machine, pid={}",
                        user_root_ppn, virt, root_pa,
                        crate::sched::get_current_pid()
                    );
                }
                return None;
            }
        }
        let root_table = PtTable::from_phys(user_root_ppn << PAGE_SHIFT)?;

        let pte2 = root_table.get(vpn2);
        if !pte2.is_valid() {
            return None;
        }

        // Check for 1GB huge page at PGD level
        if pte2.is_leaf() {
            let offset = virt & 0x3FFFFFFF; // Lower 30 bits for 1GB page
            let phys_base = pte2.ppn() << PAGE_SHIFT;
            let ppn = (phys_base + offset) >> PAGE_SHIFT;
            return Some((ppn, pte2.bits()));
        }

        // Stale-intermediate guard (x86_64 twin: exitwalk probe — a walker
        // reaching an intermediate entry whose target is not machine
        // memory is walking a freed/recycled frame; bail with a bounded
        // forensic line instead of feeding garbage to get_page_table_virt).
        {
            let pa = pte2.ppn() << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published() && pa != 0 && !crate::mm::phys_valid(pa as usize) {
                static WALK_STALE: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if WALK_STALE.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "PT-WALK-STALE: root={:#x} va={:#x} lvl=2 pte={:#x} -> phys {:#x} off-machine, pid={}",
                        user_root_ppn, virt, pte2.bits(), pa,
                        crate::sched::get_current_pid()
                    );
                }
                return None;
            }
        }

        let ppn1 = pte2.ppn();
        let table1 = PtTable::from_phys(ppn1 << PAGE_SHIFT)?;
        let pte1 = table1.get(vpn1);
        if !pte1.is_valid() {
            return None;
        }

        // Check for 2MB huge page at PMD level
        if pte1.is_leaf() {
            // For 2MB huge page at PMD level, use the correct PPN extraction
            let ppn_2mb = pte1.ppn_for_2mb_page();
            // Physical address = {PPN_2MB, offset[20:0]}
            // where PPN_2MB is the 35-bit physical page number for 2MB pages
            let offset_2mb = virt & 0x1FFFFF; // Lower 21 bits
            let phys_addr = (ppn_2mb << 21) | offset_2mb;
            // Convert to 4KB PPN for the API
            let ppn_4kb = phys_addr >> PAGE_SHIFT;

            return Some((ppn_4kb, pte1.bits()));
        }

        // Stale-intermediate guard (see the lvl=2 note above).
        {
            let pa = pte1.ppn() << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published() && pa != 0 && !crate::mm::phys_valid(pa as usize) {
                static WALK_STALE1: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if WALK_STALE1.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "PT-WALK-STALE: root={:#x} va={:#x} lvl=1 pte={:#x} -> phys {:#x} off-machine, pid={}",
                        user_root_ppn, virt, pte1.bits(), pa,
                        crate::sched::get_current_pid()
                    );
                }
                return None;
            }
        }

        let ppn0 = pte1.ppn();
        let table0 = PtTable::from_phys(ppn0 << PAGE_SHIFT)?;
        let pte0 = table0.get(vpn0);
        if !pte0.is_valid() {
            return None;
        }

        Some((pte0.ppn(), pte0.bits()))
    }
}

/// Allocate one page from the unified zone allocator
pub fn alloc_user_phys_page() -> Option<u64> {
    let phys = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys != 0 {
        Some(phys as u64)
    } else {
        None
    }
}
/// Create user address space
///
/// Collected into `arch::arch_floor` (INV-ARCH-MM-OPS-14): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn create_user_address_space() -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::mm_ops::create_user_address_space()
}

/// Copy kernel mappings to user page table
///
/// NOTE (observed 2026-11-19): this inherits the device MMIO PGDs
/// VPN2[0..2] into every user root, so user VAs in 0x0..0x7FFF_FFFF
/// resolve to DEVICE 2MB leaves — a MAP_FIXED anon mapping placed there
/// silently aliases MMIO registers (PTE ppn=0x40000 = the virtio window)
/// instead of faulting in RAM. Anything mapping user memory must place
/// it at VPN2[3] or above.
///
/// This function copies PGD entries (pointers to L1 tables), NOT the L1 tables themselves.
/// All processes share the same kernel L1/L0 page tables.
///
/// We copy:
/// 1. MMIO PGD entries (VPN2[0..2]) for device access from kernel mode
///    when running with a user page table (e.g., during syscall handling)
/// 2. All kernel space PGD entries (VPN2 >= KERNEL_PGD_START = 256)
pub(crate) fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    let kernel_phys = kernel_root_ppn * PAGE_SIZE;
    let user_phys = user_root_ppn * PAGE_SIZE;

    let kernel_table = PtTable::from_phys(kernel_phys);
    let Some(user_table) = PtTable::from_phys(user_phys) else { return };

    user_table.zero_entries();

    // Copy MMIO PGD entries (VPN2[0] and VPN2[1])
    // These map device memory (PLIC, CLINT, UART, VirtIO, PCI MMIO) at their
    // physical addresses. Needed because kernel may access devices during syscalls.
    //
    // IMPORTANT: Do NOT directly copy the PGD entry, because the kernel's L1 table
    // for VPN2[0..1] may contain non-leaf entries pointing to user-space L0 tables
    // (from a previous process's mappings). Instead, create a new L1 table and
    // copy only entries that are safe (MMIO), skipping entries that belong to user space.
    let Some(kernel_table) = kernel_table else { return };
    for i in 0..2usize {
        let pte = kernel_table.get(i);
        if !pte.is_valid() {
            continue;
        }

        if pte.is_leaf() {
            // L2 leaf (1GB huge page) — safe to copy directly
            user_table.set(i, pte);
        } else {
            // Non-leaf: create new L1 table, copy only safe entries
            let kernel_l1_phys = pte.ppn() << PAGE_SHIFT;
            let Some(kernel_l1) = PtTable::from_phys(kernel_l1_phys) else { continue };

            if let Some(new_l1_phys) = super::mmu_init::alloc_page_table() {
                let Some(new_l1_virt) = PtTable::from_phys(new_l1_phys) else { continue };
                new_l1_virt.zero_entries();

                for j in 0..512 {
                    let l1_entry = kernel_l1.get(j);
                    if !l1_entry.is_valid() {
                        continue;
                    }

                    if l1_entry.is_leaf() {
                        // Leaf entry (huge page MMIO) — safe to copy
                        new_l1_virt.set(j, l1_entry);
                    } else {
                        // Non-leaf L1 entry: points to kernel L0 table.
                        // Must NOT share the kernel L0 table pointer directly because
                        // free_user_page_tables() would free it on process exit.
                        // Instead, create a new L0 table and copy only leaf entries.
                        let l0_phys = l1_entry.ppn() << PAGE_SHIFT;
                        let Some(l0_table) = PtTable::from_phys(l0_phys) else { continue };

                        if let Some(new_l0_phys) = super::mmu_init::alloc_page_table() {
                            let Some(new_l0_virt) = PtTable::from_phys(new_l0_phys) else { continue };
                            new_l0_virt.zero_entries();

                            for k in 0..512 {
                                let l0_entry = l0_table.get(k);
                                if l0_entry.is_valid() && l0_entry.is_leaf() {
                                    // Copy leaf entries (kernel MMIO pages, U=0)
                                    // Skip user entries (U=1) — they belong to another process
                                    if !l0_entry.is_user() {
                                        new_l0_virt.set(k, l0_entry);
                                    }
                                }
                            }

                            let new_l0_ppn = new_l0_phys >> PAGE_SHIFT;
                            new_l1_virt.set(j, PageTableEntry::new_table(new_l0_ppn));
                        }
                    }
                }

                let new_l1_ppn = new_l1_phys >> PAGE_SHIFT;
                user_table.set(i, PageTableEntry::new_table(new_l1_ppn));
            }
        }
    }

    // Copy kernel-space PGD entries (VPN2 >= KERNEL_PGD_START = 256)
    for i in KERNEL_PGD_START..PTRS_PER_PGD as usize {
        let pte = kernel_table.get(i);
        if pte.is_valid() {
            user_table.set(i, pte);
        }
    }

    fence(Ordering::SeqCst);
}

/// Map user page
///
/// Returns false when the mapping could not be installed (page-table
/// allocation failure — failible map_page, fake-OOM family).
pub fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    map_page(user_root_ppn, user_virt, phys, flags)
}

/// Map user region
///
/// Leaf-PTE exclusion epoch (INV-LOCK-7; type `GlobalPteLock`, see
/// mm/mm_struct.rs and docs/architecture/lock-ordering.md).
///
/// Split mode on all arches since the 2026-10-31 perf review: PTE sites
/// with a reachable `MmStruct` take that mm's `PteLock` via
/// `pte_lock_for(mm)` (global epoch SHARED + per-mm spin — writers on
/// different mms run concurrently). The DIRECT `PTE_MODIFY_LOCK`
/// (exclusive) users below are the whole-domain minority:
/// `map_user_region`, kernel-root/unpublished-tree maps — sites that
/// cannot reach the owning mm and must exclude against everyone.

/// map_page caller-context marker (advisory, interface parity with
/// x86_64's MM_CTX): arch-neutral sites (sysv_shm, mremap, the mmap
/// paths) store a small call-site id before the unchecked map helpers
/// run. On x86_64 the PTE-OVERWRITE-LIVE tripwire reads it to name the
/// site; on riscv64 no reader exists yet — the stores stay cheap and
/// inert.
pub static MM_CTX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub static PTE_MODIFY_LOCK: crate::mm::mm_struct::GlobalPteLock =
    crate::mm::mm_struct::GlobalPteLock::new();

/// Per-mm leaf-PTE section hook (INV-LOCK-7), GLOBAL mode on riscv64:
/// every mm maps to the same global epoch section, so arch-neutral
/// callers compile unchanged against the x86_64 shape.
#[inline]
pub fn pte_lock_for(
    mm: &crate::mm::mm_struct::MmStruct,
) -> &crate::mm::mm_struct::PteLock {
    // Split mode on all arches (perf review 2026-10-31): this mm's own
    // leaf-PTE writers serialize on the per-mm spin; writers of other
    // mms run concurrently; the global-mode users (map_user_region,
    // foreign IPC installs) still hold the epoch EXCLUSIVE and exclude
    // against everyone. See docs/architecture/lock-ordering.md.
    mm.pte_lock()
}

pub(crate) static PTE_GLOBAL_HANDLE: crate::mm::mm_struct::PteLock =
    crate::mm::mm_struct::PteLock::new();

// ==================== Leaf primitives for shared mm code (FD-1) ====================
//
// mm/rmap.rs (try_to_unmap_inner) and mm/compact.rs (remap_page) used to
// carry Sv39 manual descents of their own — root indexed with bits 38:30,
// Sv39 PTE encoding — which silently no-oped on the 4-level arches (wrong
// slot, is_valid() false, unmap/migration aborted, swap-out reclaimed
// nothing). The walk shape and the PTE encoding are arch facts and live
// here; the shared mm code calls only these three primitives.

/// Permission/flag bits of a 4K leaf PTE — everything except the PPN
/// field: V|R|W|X|U|G|A|D plus the COW/PCACHE software bits. This is the
/// exact bit set [`leaf_pte_install`] puts back.
#[inline]
pub fn pte_flag_bits(bits: u64) -> u64 {
    bits & 0x3FF
}

/// Default flag bits for an anonymous user page — compaction's fallback
/// when the pre-unmap walk could not recover the live PTE. Present, RW,
/// user, accessed, dirty; non-exec (the historical 0xD7 shape).
pub const PTE_FLAGS_ANON_DEFAULT: u64 = PageTableEntry::V | PageTableEntry::R
    | PageTableEntry::W | PageTableEntry::U | PageTableEntry::A
    | PageTableEntry::D;

/// Replace the 4K leaf PTE at `vaddr` with raw `new_bits` (0 = unmap; a
/// swap or migration entry), under this mm's PTE lock. The leaf must
/// still be present and map `expect_pfn` — the R7-C3 re-validation: the
/// caller's lock-free walk may have raced a concurrent leaf writer (COW
/// break / swap-in / munmap). Returns true when the leaf was replaced;
/// the caller owns the TLB flush.
///
/// A huge-leaf entry at an upper level has no 4K slot to replace —
/// returns false (anonymous reclaim targets 4K mappings only).
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub fn leaf_pte_replace(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    expect_pfn: u64,
    new_bits: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
    let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
    let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = root_table.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }
    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);
    if !pte1.is_valid() || pte1.is_leaf() {
        return false;
    }
    let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte0 = table0.get(vpn0);
    if !pte0.is_valid() || pte0.ppn() != expect_pfn {
        return false;
    }
    table0.set(vpn0, PageTableEntry::from_bits(new_bits));
    true
}

/// Rebuild a PRESENT 4K leaf mapping `pfn` at `vaddr` from saved flag
/// bits ([`pte_flag_bits`] shape), under this mm's PTE lock. The leaf
/// slot may currently hold a non-present marker (the compaction
/// migration window) — only the intermediate levels must exist. The
/// valid bit is OR-ed in unconditionally (the saved bits of a live leaf
/// carry it; the fallback constant does too — belt for the marker case).
/// Returns true when installed; the caller owns the TLB flush.
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub fn leaf_pte_install(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    pfn: u64,
    flags: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
    let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
    let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = root_table.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }
    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);
    if !pte1.is_valid() || pte1.is_leaf() {
        return false;
    }
    let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let new_bits = (pfn << 10) | (flags & 0x3FF) | PageTableEntry::V;
    table0.set(vpn0, PageTableEntry::from_bits(new_bits));
    true
}

pub fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) {
    // Serialize against fork's table walk (PTE lock coverage).
    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
    let virt_end_checked = virt_start.checked_add(size);
    if virt_end_checked.is_none() {
        panic!("map_user_region: virt_start + size overflow");
    }
    let virt_end_val = virt_end_checked.unwrap();

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);
    let virt_end = VirtAddr::new(virt_end_val);

    let mut virt = virt_start_addr.floor();
    let end = virt_end.ceil();

    while virt.bits() < end.bits() {
        let virt_bits = virt.bits();
        let virt_start_bits = virt_start_addr.bits();
        let offset = virt_bits - virt_start_bits;
        let phys = PhysAddr::new(phys_start_addr.bits() + offset);
        map_page(user_root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
}
/// Allocate and map user memory
///
/// Collected into `arch::arch_floor` (INV-ARCH-MM-OPS-13): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn alloc_and_map_user_memory( user_root_ppn: u64, virt_addr: u64, size: u64, flags: u64, ) -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::mm_ops::alloc_and_map_user_memory(user_root_ppn, virt_addr, size, flags)
}
/// Allocate and map to kernel table
///
/// Collected into `arch::arch_floor` (INV-ARCH-MM-OPS-12): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn alloc_and_map_to_kernel_table( virt_addr: u64, size: u64, flags: u64, ) -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::mm_ops::alloc_and_map_to_kernel_table(virt_addr, size, flags)
}
/// Allocate and map to user table
///
/// Collected into `arch::arch_floor` (INV-ARCH-MM-OPS-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn alloc_and_map_to_user_table( user_ppn: u64, virt_addr: u64, size: u64, flags: u64, ) -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::mm_ops::alloc_and_map_to_user_table(user_ppn, virt_addr, size, flags)
}

// ==================== Copy-on-Write Support ====================

/// Copy-on-Write flags
pub mod cow_flags {
    /// COW flag - page is marked as copy-on-write
    pub const COW: u64 = 1 << 8;  // Use bit 8 (after A and D)

    /// This PTE maps a PAGE-CACHE frame directly (mm filemap_fault, riscv64
    /// port of the x86_64 session-23 fix) and OWNS one page-cache pin
    /// (`CachedPage.ref_count`) plus one page-descriptor reference for its
    /// mapping lifetime. Sv39 RSW bit 1, beside the COW bit (RSW bit 0).
    /// Carried through mprotect rewrites and PROT_NONE markers like COW;
    /// dropped (with the matching `page_cache::put_by_key`) at unmap, mm
    /// teardown, and the COW break; duplicated (with `pin_by_key` +
    /// get_page) by the fork walk. Session 25: MAP_SHARED cache mappings
    /// carry it TOO — the riscv64 alias's PERMANENT pin is retired; every
    /// alias owns its pin, so unmap bounds alias memory by live mappings
    /// instead of forever.
    pub const PCACHE: u64 = 1 << 9; // RSW bit 1

    /// Session 25 (MAP_SHARED writeback): SHARED cache leaves (a MAP_SHARED
    /// VMA mapping its page-cache frame WRITABLE — stores go straight into
    /// the frame, mapper coherence by construction, and a W install marks
    /// the page dirty for msync/fsync writeback) are identified by
    /// PREDICATE, not by a dedicated PTE bit: Sv39 offers only the two RSW
    /// bits (COW, PCACHE) — bit 10 and up are the PPN field, and an earlier
    /// draft of this change defined `PCACHE_SHARED = 1 << 10`, which added
    /// 4 KiB to every shared mapping's PHYSICAL ADDRESS (PTE bits are
    /// `(ppn << 10) | flags`): each mapper wrote a DIFFERENT, wrong frame
    /// one page away from the cache page. The x86_64 twin keeps a real bit
    /// (AVL bit 11) because that arch has three software bits.
    ///
    /// The predicate: PRIVATE cache leaves ALWAYS carry COW — writable
    /// installs get it for the writenotify divert, and session 26 extends
    /// it to read-only installs so the property "PCACHE set and COW clear"
    /// holds EXACTLY for SHARED cache leaves (a W-upgrade on a RO-faulted
    /// private leaf can then still be told apart from a shared one without
    /// consulting the VMA). SHARED installs never set COW. Consumers:
    /// `pte_is_shared_cache(bits)` below — the mprotect/marker re-present
    /// sites grant W IN PLACE for shared leaves (a COW copy would fork a
    /// private frame, splitting the mapping from the cache and from every
    /// other mapper) and force the COW divert for private ones.
    pub fn pte_is_shared_cache(bits: u64) -> bool {
        bits & PCACHE != 0 && bits & COW == 0
    }
}

/// Drop the page-cache pin owned by ONE mapping of a cache-backed frame
/// (the PTE carried `cow_flags::PCACHE`; x86_64 twin in the x86 mm_ops).
/// The descriptor's mapping/index fields carry the page-cache eviction
/// keys (set by cache insert); this must run AFTER the descriptor's
/// `put_page_site` so the frame's refcount is already back at the cache's
/// base reference before the pin drop — which may release and free an
/// invalidated entry — decides the frame's fate. No-op when the cache
/// entry is already gone (floored put discipline, R14-16).
pub(crate) fn release_cache_mapping_pin(ppn: usize) {
    use crate::mm::page_desc::page_desc;
    let Some(page) = page_desc(ppn) else { return };
    // Descriptor of a live frame that was mapped until this unmap;
    // mapping/index are only cleared by the cache's own release path,
    // which cannot run while a pin (held until this call) exists.
    let key = page.mapping() as u64;
    let index = page.index() as u64;
    if key != 0 {
        crate::fs::page_cache::get_page_cache().put_by_key(key, index);
    }
}
/// Copy page table with COW marking
///
/// Collected into `arch::arch_floor` (INV-ARCH-MM-OPS-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn copy_page_table_cow( parent: &crate::mm::mm_struct::MmStruct, cow_exempt: &[(u64, u64)], ) -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::mm_ops::copy_page_table_cow(parent, cow_exempt)
}

/// Outcome of a COW fault resolution attempt.
///
/// `handle_cow_fault` used to return `Option<()>`, collapsing two very
/// different situations into `None`:
///
///  - the re-walk under `PTE_MODIFY_LOCK` no longer sees a COW PTE — a
///    sibling thread sharing the mm (CLONE_VM) broke the COW between
///    `handle_mm_fault`'s LOCK-FREE `is_cow_page()` check and the locked
///    re-walk, or the mapping was concurrently unmapped/replaced;
///  - allocating the private copy genuinely failed.
///
/// `do_page_fault` mapped every `None` to `OutOfMemory`, so the race loser
/// was SIGKILLed by a fabricated OOM while the allocator sat nearly full
/// (the "fake OOM layer 2" family: GNOME gsettings/gmain/dconf-worker
/// deaths, OOM-FORENSIC dumps with ~450k free pages, zero corruption
/// counters). The tri-state result keeps the two cases apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowFaultResult {
    /// COW resolved: PTE is writable again (exclusive fast path, or a
    /// private copy was installed and mapped).
    Resolved,
    /// The PTE changed under us between the lock-free COW check and the
    /// locked re-walk. Not an error: re-execute the faulting instruction.
    /// If the mapping is now writable the store simply succeeds; if it was
    /// unmapped or swapped the NEXT fault takes the proper path
    /// (Segfault / swap-in). This mirrors Linux's do_wp_page() re-check
    /// under the PTL, which returns VM_FAULT_RETRY/0 — never a kill —
    /// when the pte changed.
    Retry,
    /// Allocating the private copy failed — a genuine out-of-memory.
    OutOfMemory,
}
/// Handle copy-on-write page fault (GLOBAL-mode PTE section on riscv64,
///
/// Collected into `arch::arch_floor` (INV-ARCH-MM-OPS-9): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn handle_cow_fault( mm: &crate::mm::mm_struct::MmStruct, root_ppn: u64, fault_addr: VirtAddr, ) -> CowFaultResult {
    crate::arch::arch_floor::riscv64::mm::mm_ops::handle_cow_fault(mm, root_ppn, fault_addr)
}

/// Check if page is a COW page
pub fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    let virt_addr = addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = root_table.get(vpn2);

    if !pte2.is_valid() {
        return false;
    }

    // Superpage leaf (PERF: boot-time MMIO windows map as 2MB L1 leaves;
    // the framebuffer maps as a 1GB L2 leaf): a leaf is never a COW PTE
    // (COW only exists on 4KB user leaves).
    if pte2.is_leaf() {
        return false;
    }

    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);

    if !pte1.is_valid() {
        return false;
    }
    if pte1.is_leaf() {
        return false;
    }

    let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte0 = table0.get(vpn0);

    if !pte0.is_valid() {
        return false;
    }

    (pte0.bits() & cow_flags::COW) != 0
}

/// Check if page has required permissions
pub fn check_pte_permissions(root_ppn: u64, addr: VirtAddr) -> Option<(bool, bool, bool, bool)> {
    let virt_addr = addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
    let pte2 = root_table.get(vpn2);

    if !pte2.is_valid() {
        return None;
    }

    // Superpage leaf (PERF: MMIO windows as 2MB L1 leaves, framebuffer as
    // 1GB L2 leaf): report the LEAF's permissions — following its PPN as
    // a table pointer would read device/framebuffer memory as PTEs.
    if pte2.is_leaf() {
        let bits = pte2.bits();
        return Some((
            (bits & PageTableEntry::R) != 0,
            (bits & PageTableEntry::W) != 0,
            (bits & PageTableEntry::X) != 0,
            (bits & PageTableEntry::U) != 0,
        ));
    }

    let table1 = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)?;
    let pte1 = table1.get(vpn1);

    if !pte1.is_valid() {
        return None;
    }
    if pte1.is_leaf() {
        let bits = pte1.bits();
        return Some((
            (bits & PageTableEntry::R) != 0,
            (bits & PageTableEntry::W) != 0,
            (bits & PageTableEntry::X) != 0,
            (bits & PageTableEntry::U) != 0,
        ));
    }

    let table0 = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT)?;
    let pte0 = table0.get(vpn0);

    if !pte0.is_valid() {
        return None;
    }

    let bits = pte0.bits();
    let has_read = (bits & PageTableEntry::R) != 0;
    let has_write = (bits & PageTableEntry::W) != 0;
    let has_exec = (bits & PageTableEntry::X) != 0;
    let is_user = (bits & PageTableEntry::U) != 0;

    Some((has_read, has_write, has_exec, is_user))
}
