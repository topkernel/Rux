//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 mm POLICY — the carved half of arch/x86_64/mm/{mm_ops,page_fault}.rs
//! (M1a, the design doc's mechanism/policy split). This file is NOT part of
//! the framework crate: it is #[path]-included by the `mm` service crate
//! and compiled under crate-level #![forbid(unsafe_code)] — every memory
//! operation below rides the framework's published safe APIs (map_page,
//! PageTableWalker, pte_lock_for, the mm/fs unsafe_edges, page_slice_mut).
//! The MECHANISM twins (walks, PTE atomics, fill/copy frame primitives)
//! stay in the arch files. Moved verbatim except: self->mm free-fn shape,
//! the raw zero/inode-deref/read_at sites onto the audited edges, and
//! vestigial unsafe wrappers dropped around already-safe calls.

use alloc::vec::Vec;
use crate::arch::mm::memory_layout::*;
use crate::arch::mm::unsafe_edges::PtTable;
use crate::arch::mm::mm_ops::{
    alloc_user_phys_page, check_pte_permissions, copy_page_table_cow, is_cow_page,
    pte_lock_for, PageTableWalker, perm_to_flags,
};
use crate::mm::page::VirtAddr as PageVirtAddr;
use crate::arch::mm::mmu_init::{
    get_page_table_virt, map_page, map_page_noflush, ROOT_PAGE_TABLE,
};
use crate::arch::mm::page_fault::{
    fill_file_page, filemap_pin_cache_page, filemap_bookkeep_mapped_cache_page,
    release_fill_page, read_pte_raw, write_pte_raw, l1_table_for,
};
use crate::arch::mm::pagetable::*;
use crate::mm::page::PAGE_SIZE as PAGE_SIZE_USIZE;
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;
use crate::mm::mm_hooks::{FaultFlags, MmFaultResult};
use crate::mm::mm_struct::MmStruct;
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::vma::{Vma, VmaFlags, VmaOrigin};
use crate::mm::{alloc_pages, GfpFlags};

// The fault-routing entry + helpers (moved from page_fault.rs).
// ==================== Fault-Around ====================

/// Fault-around window in pages for read/exec demand faults on file
/// VMAs — Linux's fault_around_bytes default shape (64 KiB / 4 KiB = 16).
/// One fault fills and maps up to this many pages starting at the faulting
/// page (session 14, the mm-side half of the 16x lib-load win; the fs-side
/// readahead detector sees the sequential read_at stream and prefetches
/// the window through the batch block layer).
///
/// Around pages are held to SINGLE-page semantics, only amortized: each
/// page is an exclusive private copy filled from the pinned vm_file at
/// its own file offset, mapped with the VMA's flags, past-EOF pages stay
/// unmapped (a later touch re-faults into the SIGBUS gate), and write
/// faults opt out (Linux do_fault_around parity).
const FAULT_AROUND_PAGES: usize = 16;

/// SHARED read fault-around window (perf-gap-hunt 2026-11-04): pages
/// prefetched per SHARED read fault, clamped by VMA end and EOF, and
/// bounded to the CONTIGUOUS RESIDENT run (holes stop the batch — a cold
/// or sparse mapping still faults page-by-page through the populate arm).
/// 512 pages = 2 MiB per fault; sequential mappers re-fault once per 2 MiB,
/// which is where the lat_pagefault amortization sits after the slot-batch
/// walker removed the per-page walk cost. The window is BIDIRECTIONAL
/// (2026-11-05): the same cap applies below the faulting page, so a random
/// touch order consumes the whole unmapped run containing the fault in one
/// trap instead of leaving the lower half to trap per page.
const SHARED_FAULT_AROUND_PAGES: usize = 512;
/// Pin-run chunk for the SHARED fault-around (bounds the stack array).
const SHARED_PIN_CHUNK: usize = 64;

/// sc181: forward fault-around window (pages) for anonymous WRITE faults
/// on large VMAs — the hilogd-ring family (see the anon batch in
/// handle_mm_fault).
const ANON_FAULTAROUND_PAGES: usize = 32;

/// sc181: minimum VMA size that gets the anonymous fault-around (small
/// stacks/heaps/brk keep exact one-page semantics).
const ANON_FAULTAROUND_MIN_VMA: usize = 1024 * 1024;

/// Try to expand stack when page fault occurs below current stack bottom
///
/// On-demand stack expansion.
fn try_expand_stack(
    mm: &MmStruct,
    fault_addr: VirtAddr,
    flags: u32,
    root_ppn: u64,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;
    use crate::mm::page::PAGE_SIZE as MM_PAGE_SIZE;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    let stack_limit = mm.stack_limit();
    let fault_addr_val = fault_addr.as_usize();

    // Check if fault address is within stack expansion range
crate::arch::mm::page_fault::FILL_REFUSAL_SITE.store(1, core::sync::atomic::Ordering::Relaxed);
    if fault_addr_val < stack_limit {
        return MmFaultResult::Segfault;
    }

    // Try to find the stack VMA (with GROWSDOWN flag)
    let vma_mgr_read = mm.vma_read();
    let (vma_start, stack_vma) = match vma_mgr_read.find_stack_vma(page_virt_addr) {
        Some((start, vma)) => (start, vma),
        None => {
            crate::arch::mm::page_fault::FILL_REFUSAL_SITE.store(2, core::sync::atomic::Ordering::Relaxed);
            return MmFaultResult::Segfault;
        }
    };

    // Calculate new start address (page-aligned)
    let new_start = PageVirtAddr::new(fault_addr_val & !(MM_PAGE_SIZE - 1));

    // New start must be below current VMA start
crate::arch::mm::page_fault::FILL_REFUSAL_SITE.store(3, core::sync::atomic::Ordering::Relaxed);
    if new_start.as_usize() >= vma_start.as_usize() {
        return MmFaultResult::Segfault;
    }

    // Check if expansion would exceed stack limit
crate::arch::mm::page_fault::FILL_REFUSAL_SITE.store(4, core::sync::atomic::Ordering::Relaxed);
    if new_start.as_usize() < stack_limit {
        return MmFaultResult::Segfault;
    }

    // Get VMA attributes before dropping the lock
    let vma_flags = stack_vma.flags();
    let vma_type = stack_vma.vma_type();

    // Verify permissions
    let is_write = flags & FaultFlags::WRITE != 0;
    let is_exec = flags & FaultFlags::EXEC != 0;
    let is_read = flags & FaultFlags::READ != 0;

    if is_write && !vma_flags.is_writable() {
        return MmFaultResult::PermissionDenied;
    }
    if is_exec && !vma_flags.is_executable() {
        return MmFaultResult::PermissionDenied;
    }
    if is_read && !vma_flags.is_readable() {
        return MmFaultResult::PermissionDenied;
    }

    // Release read lock before acquiring write lock
    drop(vma_mgr_read);

    // Expand the stack VMA downward
    {
        let mut vma_mgr_write = mm.vma_write();
crate::arch::mm::page_fault::FILL_REFUSAL_SITE.store(5, core::sync::atomic::Ordering::Relaxed);
        if vma_mgr_write
            .expand_downwards(vma_start, new_start)
            .is_err()
        {
            return MmFaultResult::Segfault;
        }
    }

    // Allocate new page
    let phys_addr = match alloc_user_phys_page() {
        Some(addr) => PhysAddr::new(addr),
        None => return MmFaultResult::OutOfMemory,
    };

    // Convert physical address to virtual address for kernel access
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;

    // Initialize page content based on type: fresh exclusive page
    // (INV-XE-ZP); each raw op carries its own block.
    match vma_type {
        VmaType::Anonymous => {
            // SAFETY: fresh exclusive page (INV-XE-ZP).
            crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
        }
        VmaType::FileBacked | VmaType::SharedMemory => {
            // SAFETY: fresh exclusive page (INV-XE-ZP).
            crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
        }
        VmaType::Device => {
            // Device mapping: don't zero
        }
    }

    // Build page table entry flags (x86: NX set unless the VMA is
    // executable; write adds RW; user pages carry US).
    let mut pte_flags =
        PageTableEntry::P | PageTableEntry::A | PageTableEntry::D | PageTableEntry::NX;
    pte_flags |= PageTableEntry::U; // User page

    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags &= !PageTableEntry::NX;
    }

    // Map page under the PTE-modify lock and keep it held across the rmap
    // setup: a concurrent fork's copy_page_table_cow walk landing between
    // the map and the rmap/refcount update takes a reference the rmap
    // never sees (round 6 MED — demand-fault PTE lock).
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
    // Re-check under the lock. The already_mapped walk at entry was
    // lock-free; two threads sharing this mm (CLONE_VM) can both see "not
    // mapped", both allocate+zero, and the second map_page would orphan
    // the first page (refcount 1, mapcount 0, unreclaimable) and silently
    // discard stores landed in it. E8-MM: same device-window-hole rule as
    // the entry walk (see pte_counts_as_mapped) — a restored kernel PTE
    // must not be mistaken for the winner's mapping here, or this path
    // "Handles" forever against it while the user instruction keeps
    // refaulting.
    if  PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) 
        .map_or(false, |(_ppn, bits)| {
            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
        })
    {
        drop(_pte_guard);
        // Lost the race: free our exclusively-owned fresh page and let the
        // caller retry — the next entry sees the mapping present.
        crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
        // R8-4: Handled, not AlreadyMapped — the trap handler maps
        // AlreadyMapped to SIGSEGV for user faults, killing the race loser
        // instead of re-executing against the winner's mapping.
        return MmFaultResult::Handled;
    }
    // Map page
    // SAFETY: root_ppn is a valid page table root, fault_addr is page-aligned,
    // phys_addr is a freshly allocated physical page, and pte_flags are well-formed.
    {
        if !map_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // Page-table allocation failure (fake-OOM family): free the
            // data page and deliver OOM — the trap layer SIGKILLs the
            // user task (Linux pagefault-OOM semantics; the old path
            // panicked the kernel here).
            crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
            return MmFaultResult::OutOfMemory;
        }

        // Address-specific TLB flush
        crate::arch::cpu::invlpg(fault_addr.bits());
    }

    // Set up reverse mapping for stack page (VMA lock already dropped,
    // so set fields directly instead of calling page_add_anon_rmap)
    {
        use crate::mm::page_desc::{page_desc, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        // Fresh exclusive page (INV-FRAME-1); SwapBacked + LRU_INACTIVE_ANON
        // membership make it visible to the reclaim engine — vmscan only
        // swaps out pages on LRU_INACTIVE_ANON with SwapBacked set.
        if let Some(page) = page_desc(page_pfn) {
            page.set_flag(PageFlag::Anonymous);
            page.set_flag(PageFlag::SwapBacked);
            page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
            page.inc_mapcount();
            crate::mm::rmap::page_record_mapping(
                page,
                mm as *const _ as usize,
                fault_addr.bits() as usize,
            );
            crate::mm::lru::page_add_anon_lru(page);
        }
    }
    drop(_pte_guard);

    // RSS accounting: stack page became resident.
    mm.add_rss(1);

    MmFaultResult::Handled
}

/// E8-MM port: does a leaf PTE (raw bits) at `va` count as "mapped" for a
/// fault on this address space? A valid KERNEL (U=0) PTE inside a
/// low-half identity device window is the RESTORE fill clear_pte leaves
/// behind after a user mapping over the window was unmapped (leaf
/// demotion made those mmaps possible — MAP_FIXED replace semantics). A
/// user fault there is a demand fault on the user mapping, not a
/// protection violation: treat the device PTE as a hole and let the VMA
/// layer decide (a stray access with no covering VMA still SIGSEGVs, just
/// via the correct path).
#[inline]
fn pte_counts_as_mapped(bits: u64, va: u64) -> bool {
    !(bits & PageTableEntry::U == 0
        && crate::arch::mm::mmu_init::kernel_device_window_pte(va).is_some())
}

pub fn handle_mm_fault(
    mm: &MmStruct,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    let root_ppn = mm.root_ppn();

    // Check if page is already mapped
    // SAFETY: root_ppn is the address space's valid root page table PPN.
    // PageTableWalker::walk only reads page table entries.
    // R-WRITESTORM-2: a PRESENT USER leaf counts as mapped. A PROT_NONE
    // marker (P=0, U=1, PPN preserved) does NOT — counting it sent the
    // fault into the already-mapped permission check, which reads P=0 as
    // "no leaf" (check_pte_permissions -> None) and killed the task with
    // PermissionDenied without ever consulting the covering VMA (the oh
    // service-kill family). Markers now fall through to the VMA layer,
    // which re-presents the leaf when the VMA grants the access.
    // One-walk entry read (gb-opt lane, 2026-11-15): the entry used a
    // PageTableWalker::walk() for the mapped check plus a read_pte_raw()
    // for the swap/migration block — two full leaf walks per fault where
    // one suffices. read_pte_raw already returns every shape the entry
    // needs: Some(raw) for P=1 leaves (P&U decides mapped), Some(raw) for
    // P=0 non-zero leaves (swap/migration entries), Some(bits) for
    // upper-level leaves (mapped), None for absent chains / zero leaves.
    // The same raw also serves the PROT_NONE-marker re-present arm below,
    // which used to re-walk. Two of the five per-fault leaf walks gone.
    let entry_raw = read_pte_raw(root_ppn, fault_addr);
    let already_mapped = entry_raw.map_or(false, |bits| {
        (bits & PageTableEntry::P) != 0 && (bits & PageTableEntry::U) != 0
    });

    // If not mapped, check for a swap entry in the PTE (P=0 but non-zero bits)
    if !already_mapped {
        if let Some(entry) = entry_raw {
            // Migration entry: compaction is relocating the page that
            // lived here. Wait for the migration window to close instead
            // of installing a zero page that the remap would then
            // overwrite (silent user-write loss). Bounded spin.
            if crate::mm::swap::is_migration_entry(entry) {
                let mut spins: u64 = 0;
                while crate::mm::compact::migration_in_progress() {
                    spins += 1;
                    if spins > 16_000_000 {
                        break; // wedged migration — fall through to refill
                    }
                    core::hint::spin_loop();
                }
                // Marker gone (remap installed a valid PTE, or munmap won)?
                // Retry the instruction.
                let still_marked = read_pte_raw(root_ppn, fault_addr)
                    .map(|e| crate::mm::swap::is_migration_entry(e))
                    .unwrap_or(false);
                if !still_marked {
                    return MmFaultResult::Handled;
                }
                // Stale marker: fall through to normal demand handling below.
            }
            if crate::mm::swap::is_swap_entry(entry) {
                crate::pr_debug!("pagefault: swap-in at {:#x}", fault_addr.bits());
                return handle_swap_fault(mm, fault_addr, flags, entry, root_ppn);
            }
        }
    }

    // Calculate access type flags
    let is_write = flags & FaultFlags::WRITE != 0;
    let is_read = flags & FaultFlags::READ != 0;
    let is_exec = flags & FaultFlags::EXEC != 0;
    let is_user = flags & FaultFlags::USER != 0;

    // If page is already mapped, first check if it's COW
    if already_mapped {
        // Check COW
        // SAFETY: root_ppn is a valid page table root. is_cow_page only reads the PTE.
        if is_write &&  is_cow_page(root_ppn, fault_addr)  {
            // wildwrite-hunt note: arming CR0.WP makes supervisor uaccess
            // writes take this path too (previously they silently wrote
            // through the COW protection). OH boots then COW-fault the
            // same buffer address once per loop iteration — an unbounded
            // pr_debug flood that alone starves the boot (the b128
            // lesson). Rate-limit repeats of the same page to one line
            // per 25 jiffies while keeping the first occurrence with the
            // access mode, so the anomaly stays visible without killing
            // the machine.
            static LAST_COW_PAGE: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(u64::MAX);
            static LAST_COW_JIFFY: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let page = fault_addr.bits() & !0xFFF;
            let now = crate::timer::get_jiffies();
            let last_page = LAST_COW_PAGE.load(core::sync::atomic::Ordering::Relaxed);
            let last_j = LAST_COW_JIFFY.load(core::sync::atomic::Ordering::Relaxed);
            let repeat = last_page == page && now.saturating_sub(last_j) < 25;
            if !repeat {
                LAST_COW_PAGE.store(page, core::sync::atomic::Ordering::Relaxed);
                LAST_COW_JIFFY.store(now, core::sync::atomic::Ordering::Relaxed);
                crate::pr_debug!(
                    "pagefault: cow at {:#x} ({}-mode)",
                    fault_addr.bits(),
                    if is_user { "user" } else { "KERNEL" }
                );
            }
            return MmFaultResult::CowPending;
        }

        // Check if page permissions meet access requirements
        // SAFETY: root_ppn is a valid page table root. check_pte_permissions only reads PTEs.
        if let Some((has_read, has_write, has_exec, pte_is_user)) =
             check_pte_permissions(root_ppn, fault_addr) 
        {
            // Verify permissions
            let perm_ok = (!is_write || has_write)
                && (!is_read || has_read)
                && (!is_exec || has_exec)
                && (!is_user || pte_is_user);

            if perm_ok {
                // Permissions correct, flush TLB for this page
                // SAFETY: invlpg invalidates the TLB entry for this
                // address so the retried access uses the current PTE.
                {
                    crate::arch::cpu::invlpg(fault_addr.bits());
                }
                return MmFaultResult::Handled;
            }
        }

        return MmFaultResult::PermissionDenied;
    }

    // Find VMA. DEFECT-C FIX (LTP mmap1 false-kill class): Rux drops the
    // VMA read guard before the fill (the R73 EXT4_BIG_LOCK pinch rule),
    // so a sibling thread's munmap can complete between the faulting
    // instruction and this lookup — the find sees absence for a mapping
    // that WAS live at fault time, and the old code killed the task
    // (SIGSEGV via try_expand_stack's site-1). Linux holds mmap_read
    // from before lookup through fill, so the unmap cannot interleave.
    // Our shape cannot hold the guard across the file read; instead, on
    // a miss we RETRY the find once under a fresh guard before refusing:
    // if the mapping is back (the remap churn of the racing thread) or
    // the first find lost a snapshot race, the fill proceeds. Only a
    // mapping that is absent on BOTH lookups refuses.
    let mut vma_mgr = mm.vma_read();
    let mut vma = vma_mgr.find(page_virt_addr);
    if vma.is_none() {
        drop(vma_mgr);
        vma_mgr = mm.vma_read();
        vma = vma_mgr.find(page_virt_addr);
        if vma.is_none() {
            drop(vma_mgr);
            // Defect-C budget print (12/boot): the refusal inputs after
            // the retry — both finds missed, the expansion fallback
            // decides (stack growth or SIGSEGV).
            static MISS_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if MISS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 12 {
                crate::pr_err!(
                    "UFILL-MISS2 pid={} addr={:#x} — absent on retry",
                    crate::process::current_pid(),
                    fault_addr.as_usize()
                );
            }
            return try_expand_stack(mm, fault_addr, flags, root_ppn);
        }
    }
    let vma = vma.unwrap();

    // Get VMA attributes
    let vma_flags = vma.flags();
    let vma_type = vma.vma_type();
    let vma_file_fd = vma.file_fd();
    let vma_file_size = vma.file_size();
    let vma_offset = vma.offset();
    // File-fault lookup dedup (launcher stime burn, session 13): the
    // EOF/SIGBUS check and the fill arm below used to EACH re-find this
    // VMA (full vma tree walk) and re-resolve the pinned vm_file, plus a
    // sched::current() round trip — 3 finds and up to 4 pin-table
    // lookups per fault, x25600 faults for a 100 MB lib load. Capture
    // everything the file paths need ONCE here, under the guard we
    // already hold; the guard is dropped before any read_at (the R73
    // preemption/EXT4_BIG_LOCK pinch rule is about holding it across
    // I/O, not about cloning an Arc under it).
    let vma_start = vma.start().as_usize();
    // Fault-around (session 14): the batch below also needs the VMA's
    // end to bound its window; captured under the same entry guard
    // snapshot.
    let vma_end = vma.end().as_usize();
    let vm_file = if vma_type == VmaType::FileBacked && vma_file_fd >= 0 {
        mm.get_vma_file(vma_start)
    } else {
        None
    };
    let file_offset = vma_offset + (page_virt_addr.as_usize() - vma_start);
    // Effective size: CURRENT inode size (a file grown or truncated
    // since mmap must serve/shrink accordingly); devfs regular files
    // additionally consult the live entry (b74 ftruncate snapshot). The
    // mmap-time snapshot stays as the no-file fallback. One computation
    // serves both the SIGBUS gate below and the fill arm.
    let effective_size = vm_file
        .as_ref()
        .and_then(|f| {
            if let Some(live) = crate::fs::leaf_glue::devfs::reg_current_size(f.as_ref()) {
                return Some(live);
            }
            // SAFETY: inode cell written at open time; read-only.
            let inode_opt = crate::fs::unsafe_edges::file_inode(f);
            inode_opt.map(|i| i.get_size())
        })
        .unwrap_or(vma_file_size) as usize;

    // Verify permissions
    if is_write && !vma_flags.is_writable() {
        return MmFaultResult::PermissionDenied;
    }
    if is_exec && !vma_flags.is_executable() {
        return MmFaultResult::PermissionDenied;
    }
    if is_read && !vma_flags.is_readable() {
        return MmFaultResult::PermissionDenied;
    }

    // R-WRITESTORM-2 (OH Permission-denied family): a P=0, U=1, non-swap
    // leaf is a PROT_NONE marker left by mprotect(PROT_NONE) — the PPN is
    // deliberately preserved so a re-protect restores the same page. The
    // entry walk below no longer counts markers as "already mapped", so
    // the fault reaches this VMA layer. If the covering VMA now GRANTS
    // the access (the region was re-protected from PROT_NONE, or a fork
    // raced an mprotect upgrade and left the child's VMA/PTE disagreeing
    // — fork copies PTEs and snapshots VMAs in two separately-locked
    // phases), the marker must be RE-PRESENTED with the VMA's
    // permissions instead of (a) short-circuiting to SIGSEGV (the oh
    // b113-b121 service kills: `pagefault: Permission denied at
    // rip=ld.so+0x89ac6`, covering VMA flags=0x10 right after GWP-Asan
    // mprotect'ed the slot readable) or (b) zero-filling over the
    // quarantined page (silent data loss). Linux parity: a fault under a
    // VMA that grants the access must succeed regardless of the region's
    // mprotect history; the stale marker is advisory PTE state, not a
    // protection decision.
    {
        // The marker read reuses the entry walk's raw (see the entry
        // comment): same address, same leaf. page_base stays for the
        // re-present write below.
        let page_base = VirtAddr::new(fault_addr.bits() & !(PAGE_SIZE as u64 - 1));
        if let Some(raw) = entry_raw {
            if is_protnone_marker(raw) {
                let mut flags = PageTableEntry::P
                    | PageTableEntry::U
                    | PageTableEntry::A
                    | PageTableEntry::D
                    | PageTableEntry::NX;
                if vma_flags.is_writable() {
                    flags |= PageTableEntry::W;
                }
                if vma_flags.is_executable() {
                    flags &= !PageTableEntry::NX;
                }
                let cow_bit = raw & crate::arch::mm::mm_ops::cow_flags::COW;
                let pcache_bit = raw
                    & (crate::arch::mm::mm_ops::cow_flags::PCACHE
                        | crate::arch::mm::mm_ops::cow_flags::PCACHE_SHARED
                        | crate::arch::mm::mm_ops::cow_flags::PCACHE_RO);
                if cow_bit != 0 {
                    flags |= cow_bit;
                    flags &= !PageTableEntry::W;
                }
                if pcache_bit != 0 {
                    flags |= pcache_bit;
                    if flags & PageTableEntry::W != 0
                        && pcache_bit
                            & (crate::arch::mm::mm_ops::cow_flags::PCACHE_SHARED
                                | crate::arch::mm::mm_ops::cow_flags::PCACHE_RO)
                            == 0
                    {
                        // Private cache-backed leaf: never W in place —
                        // divert the first write through COW (see
                        // mprotect_pte_range). SHARED cache leaves keep W:
                        // their stores must land in the cache frame
                        // (session 25).
                        flags |= crate::arch::mm::mm_ops::cow_flags::COW;
                        flags &= !PageTableEntry::W;
                    }
                }
                let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
                // Re-read under the lock: a racing mprotect / COW fault /
                // swap-in may have replaced the leaf since the lock-free
                // read above; only rewrite a still-marker leaf.
                if let Some(now) = read_pte_raw(root_ppn, page_base) {
                    if is_protnone_marker(now) {
                        let new_bits = (now & PageTableEntry::PHYS_MASK_PUBLIC) | flags;
                        write_pte_raw(root_ppn, page_base, new_bits);
                        // SAFETY: invlpg on a user VA from kernel context.
                        {
                            crate::arch::cpu::invlpg(page_base.bits());
                        }
                        return MmFaultResult::Handled;
                    }
                    // Leaf changed under us: fall through; the demand
                    // paths below (or the retry) act on the new state.
                }
            }
        }
    }

    if crate::trace::EVENT_STEPTRACE.armed() {
        crate::pr_debug!(
            "pagefault: map new page at {:#x}, type={:?}",
            fault_addr.bits(),
            vma_type
        );
    }

    // Release read lock
    drop(vma_mgr);

    // File-backed fault BEYOND the file's end is SIGBUS, not a zero page
    // (Linux filemap_fault: no page at/after EOF can be served for a
    // mapping — LTP mmap13 truncates a mapped file and expects the next
    // touch of the cut page to raise SIGBUS). The effective size is the
    // CURRENT inode size (a truncate after mmap shrinks it), falling
    // back to the snapshot taken at mmap time.
    if vma_type == VmaType::FileBacked && vma_file_fd >= 0 {
        // Uses the entry-time captures (vma_start / vm_file /
        // file_offset / effective_size) — see the dedup comment above.
        {
            // Page-granular EOF gate (fsx-linux FXW family, filemap_fault
            // parity): a fault on the file's LAST PARTIAL page serves (the
            // tail past i_size reads as zeros downstream — the fill arm
            // clamps at effective_size); only a page FULLY past i_size
            // raises SIGBUS, which keeps mmap13's post-truncate cut-page
            // SIGBUS intact (the cut page is fully past). A WRITE fault on
            // a SHARED writable TMPFS mapping never SIGBUSes here even
            // fully past EOF: the reference lets a mapper's store land in
            // the alias frame beyond i_size (the size itself moves only on
            // write(2)/ftruncate — the store is invisible to fd reads,
            // same as the reference). Other files keep the strict form —
            // the ext4 writeback machinery is not built for past-EOF
            // dirty tails.
            let file_pages = effective_size.div_ceil(PAGE_SIZE_USIZE);
            let fully_past = file_offset >= file_pages * PAGE_SIZE_USIZE;
            let tmpfs_shared_write = is_write
                && vma_flags.is_writable()
                && vma_flags.contains(crate::mm::vma::VmaFlags::SHARED)
                && vm_file
                    .as_ref()
                    .is_some_and(|f| crate::fs::leaf_glue::tmpfs::is_tmpfs_file(f.as_ref()));
            if fully_past && !tmpfs_shared_write {
                crate::pr_err!(
                    "pagefault: SIGBUS forensics: addr {:#x} vma_start {:#x} vma_off {:#x} file_off {:#x} eff_size {:#x} mmap_snap_size {:#x} fd {} pid {} path {}",
                    page_virt_addr.as_usize(),
                    vma_start,
                    vma_offset,
                    file_offset,
                    effective_size,
                    vma_file_size as usize,
                    vma_file_fd,
                    crate::process::current_pid(),
                    vm_file
                        .as_ref()
                        .map(|f| f.path())
                        .unwrap_or_default(),
                );
                return MmFaultResult::BusError;
            }

            // Coherent SHARED devfs mapping (OH param workspaces, see
            // fs/devfs SharedFilePages): instead of a per-process
            // private page filled from read_at, map the FILE'S shared
            // physical page so plain memcpy stores from any mapper
            // are instantly visible to every other mapper — Linux
            // tmpfs MAP_SHARED semantics, which the OH parameter
            // service's write protocol (memcpy, no msync) requires.
            if vma_flags.contains(crate::mm::vma::VmaFlags::SHARED) {
                if let Some(shared_phys) = vm_file.as_ref().and_then(|f| {
                    // The ops-identity gate is folded into the row
                    // (None for non-registry files).
                    crate::fs::leaf_glue::devfs::shared_fault_file_page(
                        f.as_ref(),
                        (file_offset / (PAGE_SIZE as usize)) * (PAGE_SIZE as usize),
                    )
                }) {
                    let sp_phys = PhysAddr::new(shared_phys);
                    let mut sp_flags = PageTableEntry::P
                        | PageTableEntry::A
                        | PageTableEntry::D
                        | PageTableEntry::NX
                        | PageTableEntry::U;
                    if vma_flags.is_writable() {
                        sp_flags |= PageTableEntry::W;
                    }
                    if vma_flags.is_executable() {
                        sp_flags &= !PageTableEntry::NX;
                    }
                    let _sp_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
                    if  PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) 
                        .map_or(false, |(_ppn, bits)| {
                            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
                        })
                    {
                        return MmFaultResult::Handled;
                    }
                    // SAFETY: root_ppn valid; fault_addr page-aligned;
                    // sp_phys is the shared file page; flags built above.
                    {
                        if !map_page(root_ppn, fault_addr, sp_phys, sp_flags) {
                            // PT allocation failure: the mapping's
                            // reference was NOT yet taken (bookkeeping
                            // follows the map) — plain OOM return.
                            return MmFaultResult::OutOfMemory;
                        }
                        crate::arch::cpu::invlpg(fault_addr.bits());
                    }
                    // Refcount/mapcount bookkeeping for the shared
                    // frame (multi-mapper: fork's COW walk exempts
                    // SHARED VMAs and bumps the refcount itself).
                    // CRITICAL (b84): each MAPPING must hold its own
                    // page REFERENCE — unmap_pages put_page()s every
                    // mapped PTE and frees at refcount 0. Without the
                    // get_page here, the FIRST unmap freed the frame
                    // back to the buddy while the file's shared
                    // backing still listed it — the buddy re-handed
                    // the same PFNs to the NEXT allocation (observed:
                    // param_sec_dac off 0x1000/0x2000 receiving
                    // param_selinux's frames 0x7f608/0x7f609), and
                    // the two files' contents cross-wired through the
                    // aliased pages — the same double-handout engine
                    // behind this family's "wrong file content"
                    // livelocks (and plausibly the historical
                    // random-global scribbles: user stores landing in
                    // re-handed kernel pages).
                    {
                        use crate::mm::page_desc::{page_desc, PageFlag};
                        // Descriptor of a live frame; the backing holds
                        // the base reference, so get_page cannot race a
                        // 0→1 resurrection (INV-FRAME-1).
                        if let Some(page) = page_desc((shared_phys >> PAGE_SHIFT) as usize) {
                            page.get_page();
                            page.set_flag(PageFlag::Anonymous);
                            page.inc_mapcount();
                            crate::mm::rmap::page_record_mapping(
                                page,
                                mm as *const _ as usize,
                                fault_addr.bits() as usize,
                            );
                        }
                    }
                    mm.add_rss(1);
                    return MmFaultResult::Handled;
                }
            }
        }

        // MAP_SHARED ext4 (session 25): coherent write-through alias of
        // the PAGE-CACHE frame — the PCACHE protocol's shared face. The
        // frame is pinned exactly like the private arm (get_phys /
        // Discard-warm) but mapped with W and NO COW bit: stores land
        // directly in the cache frame, so every mapper of the file —
        // across fork and across fresh mmaps — observes them (the old
        // private-copy fallback gave each writer its own frame, mappers
        // diverged, and nothing ever persisted). The PTE owns the pin
        // (dropped at unmap like every PCACHE leaf — alias memory is
        // bounded by live mappings, not forever); a W mapping marks its
        // page dirty at install (conservative v1) so msync/fsync write
        // it back through the ext4 machinery. Populate failure (I/O
        // error) falls through to the private-copy fill below.
        if vma_flags.contains(crate::mm::vma::VmaFlags::SHARED) {
            let file = vm_file
                .clone()
                .or_else(|| crate::fs::get_file_fd(vma_file_fd as usize));
            if let Some(file) = file.filter(|f| {
                f.get_ops().is_some_and(|ops| ops.fault.is_some())
                    || crate::fs::leaf_glue::tmpfs::is_tmpfs_file(f.as_ref())
            }) {
                // Fault-around for SHARED read faults (perf-gap-hunt
                // 2026-11-04): the reference's filemap_map_pages serves
                // SHARED read faults with the same batching as private
                // ones; the single-page arm below mapped exactly ONE
                // page per trap, which is the lat_pagefault 6.5x gap
                // (every touch of a MAP_SHARED read mapping trapped).
                // Warm path only: pin the contiguous RESIDENT run
                // starting at the faulting page in chunks
                // (get_range_into: one claim + one inode lock + one
                // BTreeMap range walk per chunk), map each chunk under
                // one verify + PTE-lock section with a SLOT-BATCHED
                // walker (one upper-level walk per 2 MB region; one
                // leaf load + one leaf store per page). A run of 0 on
                // the FIRST chunk (cold faulting page) falls through to
                // the single-page populate arm below, which warms it
                // for the next touch's batch. Write faults keep the
                // single-page arm (a write is the demand marker, not a
                // prefetch). Window: SHARED_FAULT_AROUND_PAGES — wide,
                // BIDIRECTIONAL (the backward gap below the fault is
                // scanned and filled first), but only pages ALREADY
                // RESIDENT in the page cache are prefetched (a hole
                // stops the run), so a cold sparse mapping still
                // faults one page at a time.
                if !is_write {
                    let fault_page = page_virt_addr.as_usize() & !(PAGE_SIZE_USIZE - 1);
                    // Window bounds: VMA end (INV-VMA-4), then EOF — a
                    // page whose file offset is >= effective_size stays
                    // UNMAPPED so a later touch re-faults into the
                    // SIGBUS gate above (mmap13 truncate semantics).
                    let eof_limit = vma_start + effective_size.saturating_sub(vma_offset);
                    let win_end = (fault_page + SHARED_FAULT_AROUND_PAGES * PAGE_SIZE_USIZE)
                        .min(vma_end)
                        .min(eof_limit);
                    // fault_page < win_end always: the SIGBUS gate
                    // above passed for the faulting page.
                    let page_index =
                        ((vma_offset + (fault_page - vma_start)) / PAGE_SIZE_USIZE) as u64;
                    let inode_ref = crate::fs::unsafe_edges::file_inode(&file);
                    if let Some(inode) = inode_ref {
                        let cache = crate::fs::page_cache::get_page_cache();
                        let (fs_id, ino) = (inode.fs_id, inode.ino as u64);
                        let want_w = vma_flags.is_writable();
                        // Leaf form: a W VMA maps cache frames writable
                        // with full pin+ref ownership (PCACHE_SHARED,
                        // session 25). A read fault on a read-only VMA
                        // installs the PIN-FREE RO alias (PCACHE_RO):
                        // mapcount-only protection (see cow_flags) —
                        // the pins taken by the batch below are
                        // RELEASED once the leaves + mapcounts are in
                        // (one put_range per chunk), so a 512-page
                        // window costs one mapcount RMW per page
                        // instead of a pin+refcount round trip each.
                        let sp_flags = if want_w {
                            let mut f = PageTableEntry::P
                                | PageTableEntry::A
                                | PageTableEntry::D
                                | PageTableEntry::NX
                                | PageTableEntry::U
                                | crate::arch::mm::mm_ops::cow_flags::PCACHE
                                | crate::arch::mm::mm_ops::cow_flags::PCACHE_SHARED
                                | PageTableEntry::W;
                            if vma_flags.is_executable() {
                                f &= !PageTableEntry::NX;
                            }
                            f
                        } else {
                            let mut f = PageTableEntry::P
                                | PageTableEntry::A
                                | PageTableEntry::D
                                | PageTableEntry::NX
                                | PageTableEntry::U
                                | crate::arch::mm::mm_ops::cow_flags::PCACHE
                                | crate::arch::mm::mm_ops::cow_flags::PCACHE_RO;
                            if vma_flags.is_executable() {
                                f &= !PageTableEntry::NX;
                            }
                            f
                        };
                        let mut mapped_total: u64 = 0;
                        let mut fault_pg_failed = false;
                        let mut verify_failed = false;
                        // BACKWARD GAP (perf-gap-hunt, pagefault 2x
                        // push): with random touch order the unmapped
                        // run containing the fault usually extends
                        // BELOW the faulting page too; a forward-only
                        // window leaves that half to trap again one
                        // page at a time. Scan the PTEs below the
                        // faulting page for the first mapped page (no
                        // locks — advisory only: the install loop
                        // re-checks every leaf under the PTE lock and
                        // its gap-fill stop treats a racy
                        // "already mapped" finding as the run end).
                        // Cap: the same 512-page window, VMA-start
                        // bound (pages below the fault have smaller
                        // file offsets, always inside the file).
                        let win_back_start = fault_page
                            .saturating_sub(SHARED_FAULT_AROUND_PAGES * PAGE_SIZE_USIZE)
                            .max(vma_start);
                        let mut back_start = fault_page; // gap = [back_start, fault_page)
                        {
                            let mut l1b: Option<(PtTable, usize)> = None;
                            let mut b = fault_page;
                            while b > win_back_start {
                                let p = b - PAGE_SIZE_USIZE;
                                let region = p & !0x1F_FFFF;
                                if l1b.map_or(true, |(_t, rb)| rb != region) {
                                    // SAFETY: root_ppn is this address
                                    // space's valid root; read-only walk.
                                    l1b =
                                         l1_table_for(root_ppn, VirtAddr::new(p as u64)) 
                                            .map(|t| (t, region));
                                }
                                match l1b {
                                    Some((table, _)) => {
                                        let slot = (p >> 12) & 0x1FF;
                                        let bits = table.get_raw(slot);
                                        if bits != 0 && pte_counts_as_mapped(bits, p as u64) {
                                            break; // mapped page below:
                                                   // the gap starts above it
                                        }
                                        back_start = p;
                                        b = p;
                                    }
                                    None => {
                                        // Missing upper level: the
                                        // whole 2 MB region below is
                                        // unmapped — jump to its start
                                        // (never below win_back_start).
                                        let region_start = region.max(win_back_start);
                                        back_start = region_start;
                                        b = region_start;
                                    }
                                }
                            }
                        }
                        // Fill order: the backward gap first, then the
                        // forward window (the faulting page lives in
                        // the forward range; the backward range can
                        // only add pages). Both run the same chunk
                        // loop: pin a resident run, verify, map,
                        // repeat while the run stayed contiguous.
                        let ranges: [(usize, usize); 2] =
                            [(back_start, fault_page), (fault_page, win_end)];
                        let mut dirty_spans: [(u64, u64); 2] = [(0, 0); 2]; // (start index, pages advanced)
                        for (ri, &(range_va, range_end)) in ranges.iter().enumerate() {
                            if range_va >= range_end {
                                continue;
                            }
                            let range_pages = (range_end - range_va) / PAGE_SIZE_USIZE;
                            let range_index =
                                ((vma_offset + (range_va - vma_start)) / PAGE_SIZE_USIZE) as u64;
                            // The faulting page sits at offset 0 of
                            // the FORWARD range only; usize::MAX never
                            // matches a page offset.
                            let fault_off = if range_va == fault_page {
                                0
                            } else {
                                usize::MAX
                            };
                            let mut base_off = 0usize; // range pages consumed
                            while base_off < range_pages {
                                let want = (range_pages - base_off).min(SHARED_PIN_CHUNK);
                                let mut run: [(u64, *const u8); SHARED_PIN_CHUNK] =
                                    [(0, core::ptr::null::<u8>()); SHARED_PIN_CHUNK];
                                // Pin phase — NO locks held (the vma guard
                                // is taken after; R73: never pin/IO under
                                // the vma guard). Stops at the first hole.
                                let n = cache.get_range_into(
                                    fs_id,
                                    ino,
                                    range_index + base_off as u64,
                                    &mut run[..want],
                                );
                                if n == 0 {
                                    break; // cold page (first chunk) or hole
                                }
                                // Map-phase guard discipline (the PRIVATE
                                // arm's twin, INV-LOCK-7 order vma -> pte):
                                // re-verify the VMA snapshot under a read
                                // guard held across this chunk's maps —
                                // every mm-side mutator of the range needs
                                // the write lock, so it cannot interleave
                                // between verify and map.
                                let vma_mgr2 = mm.vma_read();
                                let snapshot_ok = vma_mgr2
                                    .find(PageVirtAddr::new(fault_page))
                                    .is_some_and(|v| {
                                        v.start().as_usize() == vma_start
                                            && v.end().as_usize() >= win_end
                                            && v.offset() == vma_offset
                                            && v.file_fd() == vma_file_fd
                                            && v.flags() == vma_flags
                                    });
                                if !snapshot_ok {
                                    drop(vma_mgr2);
                                    // VMA changed under us: release this
                                    // chunk's pins and retry — the fresh
                                    // fault takes current VMA state. Pages
                                    // mapped by EARLIER chunks were mapped
                                    // under their own live verification.
                                    cache.put_range(fs_id, ino, &run[..n]);
                                    verify_failed = true;
                                    break;
                                }
                                // One PTE-lock section for the chunk: a
                                // concurrent fork COW walk cannot observe a
                                // half-mapped window.
                                let _pte_guard =
                                    crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
                                // Slot-batched walk: the L1 (PT) table for
                                // the current 2 MB region is resolved once
                                // and reused; only the leaf load/store
                                // remains per page. None (missing upper
                                // level / huge leaf) falls back to
                                // map_page_noflush for that page, which
                                // allocates/demotes, then re-resolves.
                                let mut l1: Option<(PtTable, usize)> = None;
                                // Pin ownership per page of this chunk: all
                                // n start pinned (get_range_into). A page
                                // that fails its map releases its own pin
                                // immediately (pin_released). At chunk end
                                // the RO form releases every still-held pin
                                // (its leaves own none — mapcount only);
                                // the W form releases only the pins of
                                // pages that never got installed (an
                                // installed W leaf owns its pin).
                                let mut pin_released = [false; SHARED_PIN_CHUNK];
                                let mut installed_mark = [false; SHARED_PIN_CHUNK];
                                // Gap-fill stop: the first ALREADY-MAPPED
                                // page ends the chunk (and the window).
                                // Random touch order makes pages ahead of
                                // the faulting one mostly covered by a
                                // prior window — pinning+checking hundreds
                                // of them per trap was the dominant batch
                                // overhead.
                                let mut stop_at = n;
                                let mut hit_mapped = false;
                                for i in 0..n {
                                    let va = range_va + (base_off + i) * PAGE_SIZE_USIZE;
                                    let region = va & !0x1F_FFFF;
                                    if l1.map_or(true, |(_t, b)| b != region) {
                                        // SAFETY: root_ppn is the address
                                        // space's valid root; read-only walk.
                                        l1 = 
                                            l1_table_for(root_ppn, VirtAddr::new(va as u64))
                                        
                                        .map(|t| (t, region));
                                    }
                                    let mut installed = false;
                                    match l1 {
                                        Some((table, _)) => {
                                            let slot = (va >> 12) & 0x1FF;
                                            let bits = table.get_raw(slot);
                                            if bits != 0 && pte_counts_as_mapped(bits, va as u64) {
                                                // Already mapped — stop here.
                                                stop_at = i;
                                                hit_mapped = true;
                                                break;
                                            }
                                            let phys_i =
                                                virt_to_phys(VirtAddr::new(run[i].1 as u64));
                                            // Leaf slot verified
                                            // non-present above, under the
                                            // PTE lock; phys is the pinned
                                            // cache frame. No invlpg —
                                            // fresh installs are not TLB
                                            // cached (map_page_noflush's
                                            // contract).
                                            table.set(
                                                slot,
                                                PageTableEntry::from_bits(
                                                    phys_i.bits() | sp_flags,
                                                ),
                                            );
                                            installed = true;
                                        }
                                        None => {
                                            // SAFETY: fallback with full
                                            // allocation/demotion handling;
                                            // same contract as the arms.
                                            let ok = 
                                                map_page_noflush(
                                                    root_ppn,
                                                    VirtAddr::new(va as u64),
                                                    virt_to_phys(VirtAddr::new(run[i].1 as u64)),
                                                    sp_flags,
                                                )
                                            ;
                                            if ok {
                                                installed = true;
                                            } else {
                                                // PT alloc failure: release
                                                // this page's pin; the
                                                // FAULTING page failing
                                                // means OOM.
                                                cache.put_range(fs_id, ino, &run[i..i + 1]);
                                                pin_released[i] = true;
                                                if base_off + i == fault_off {
                                                    fault_pg_failed = true;
                                                }
                                                l1 = None;
                                                continue;
                                            }
                                        }
                                    }
                                    if installed {
                                        installed_mark[i] = true;
                                        if want_w {
                                            // Full-form leaf: one descriptor
                                            // ref + mapcount (dropped at
                                            // unmap like every PCACHE
                                            // mapping); no rmap — the
                                            // frame's eviction keys belong
                                            // to the page cache.
                                            // SAFETY: pinned cache frame
                                            // just mapped under the PTE
                                            // lock.
                                            filemap_bookkeep_mapped_cache_page(
                                                virt_to_phys(VirtAddr::new(run[i].1 as u64)).bits(),
                                            );
                                        } else {
                                            // RO-form leaf: mapcount ONLY
                                            // (the descriptor ref and the
                                            // cache pin are deliberately
                                            // absent — see PCACHE_RO). The
                                            // mapcount is what blocks
                                            // eviction from freeing the
                                            // frame while mapped.
                                            use crate::mm::page_desc::page_desc;
                                            // Descriptor of a live cache
                                            // frame, pinned at this
                                            // moment (released only after
                                            // this loop).
                                            if let Some(pd) = page_desc(
                                                (virt_to_phys(VirtAddr::new(run[i].1 as u64))
                                                    .bits()
                                                    >> PAGE_SHIFT)
                                                    as usize,
                                            ) {
                                                pd.inc_mapcount();
                                            }
                                        }
                                        mapped_total += 1;
                                    }
                                }
                                drop(_pte_guard);
                                drop(vma_mgr2);
                                // Chunk-end pin accounting. RO form: every
                                // still-held pin goes back (mapcounts were
                                // installed under the PTE lock above —
                                // eviction safety has transferred). W form:
                                // only the never-installed pages' pins
                                // (installed W leaves own theirs).
                                {
                                    let mut rel: [(u64, *const u8); SHARED_PIN_CHUNK] =
                                        [(0, core::ptr::null::<u8>()); SHARED_PIN_CHUNK];
                                    let mut rn = 0usize;
                                    for i in 0..n {
                                        let leaf_owns_pin = want_w && installed_mark[i];
                                        if !pin_released[i] && !leaf_owns_pin {
                                            rel[rn] = run[i];
                                            rn += 1;
                                        }
                                    }
                                    if rn > 0 {
                                        cache.put_range(fs_id, ino, &rel[..rn]);
                                    }
                                }
                                // Advance past the processed pages; a
                                // gap-fill stop also ends the WINDOW (the
                                // mapped page and its followers belong to a
                                // prior window; a hole ends the resident
                                // run). Both stop the outer chunk loop.
                                base_off += if hit_mapped { stop_at + 1 } else { n };
                                if hit_mapped || n < want {
                                    break;
                                }
                            }
                            // Record the span this range ADVANCED over
                            // (includes the stopping page on a gap-fill
                            // stop — the conservative over-mark rule
                            // below blesses that) for the W dirty marks.
                            dirty_spans[ri] = (range_index, base_off as u64);
                        }
                        if verify_failed {
                            return MmFaultResult::Handled;
                        }
                        if mapped_total > 0 {
                            if want_w
                                && (file.get_ops().is_some_and(|ops| ops.fault.is_some())
                                    || crate::fs::leaf_glue::tmpfs::is_tmpfs_file(file.as_ref()))
                            {
                                // Conservative v1 dirty mark (the
                                // single-page arm's rule, applied to the
                                // prefetched window): a writable mapping
                                // MAY write; flushing an unwritten page
                                // is harmless I/O.
                                for &(ds_start, ds_pages) in dirty_spans.iter() {
                                    for idx in 0..ds_pages {
                                        cache.mark_dirty(fs_id, ino, ds_start + idx);
                                    }
                                }
                            }
                            mm.add_rss(mapped_total);
                            if fault_pg_failed {
                                return MmFaultResult::OutOfMemory;
                            }
                            return MmFaultResult::Handled;
                        }
                        if fault_pg_failed {
                            return MmFaultResult::OutOfMemory;
                        }
                        // mapped_total == 0: cold faulting page — the
                        // single-page populate arm below warms it (and
                        // the next touch in this window batches).
                    }
                }
                // tmpfs (LTP setup_ipc prefers /dev/shm for its shared
                // results/checkpoint page): alias through the page
                // cache via the fs-agnostic populate (bounce read of
                // the node's page into a cache frame, zero for holes)
                // — one frame per (fs_id, ino, page) shared by every
                // mapper across fork AND across fresh mmaps after
                // exec. Frames have no disk writeback; read(2) stays
                // on the node pages (documented deviation class, same
                // as ext4's write(2) invalidation).
                let cache_phys = if crate::fs::leaf_glue::tmpfs::is_tmpfs_file(file.as_ref()) {
                    crate::fs::page_cache::shared_file_fault_page(file.as_ref(), file_offset as u64)
                } else {
                    filemap_pin_cache_page(file.as_ref(), file_offset, effective_size, true)
                };
                if let Some(cache_phys) = cache_phys {
                    // file_offset < effective_size held (SIGBUS gate).
                    let mut sp_flags = PageTableEntry::P
                        | PageTableEntry::A
                        | PageTableEntry::D
                        | PageTableEntry::NX
                        | PageTableEntry::U
                        | crate::arch::mm::mm_ops::cow_flags::PCACHE;
                    let want_w = vma_flags.is_writable();
                    if want_w {
                        sp_flags |= PageTableEntry::W | crate::arch::mm::mm_ops::cow_flags::PCACHE_SHARED;
                    }
                    if vma_flags.is_executable() {
                        sp_flags &= !PageTableEntry::NX;
                    }
                    let _sp_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
                    if  PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) 
                        .map_or(false, |(_ppn, bits)| {
                            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
                        })
                    {
                        // Lost the race to an identical mapping: our
                        // pin is surplus — release it before retrying.
                        drop(_sp_guard);
                        release_fill_page(cache_phys, true);
                        return MmFaultResult::Handled;
                    }
                    // SAFETY: root_ppn valid; fault_addr page-aligned;
                    // cache_phys is the pinned cache frame; flags built
                    // from the VMA's permissions plus the PCACHE bits.
                    {
                        if !map_page(root_ppn, fault_addr, PhysAddr::new(cache_phys), sp_flags) {
                            // PT allocation failure: the mapping never
                            // happened — return the pin (anti-leak) and
                            // deliver OOM (trap layer SIGKILLs).
                            release_fill_page(cache_phys, true);
                            return MmFaultResult::OutOfMemory;
                        }
                        crate::arch::cpu::invlpg(fault_addr.bits());
                        // One descriptor ref + mapcount (dropped at
                        // unmap like every PCACHE mapping); deliberately
                        // NO rmap/set_index — the frame's eviction keys
                        // belong to the page cache.
                        filemap_bookkeep_mapped_cache_page(cache_phys);
                    }
                    if want_w
                        && (file.get_ops().is_some_and(|ops| ops.fault.is_some())
                            || crate::fs::leaf_glue::tmpfs::is_tmpfs_file(file.as_ref()))
                    {
                        // Conservative v1 dirty mark, AFTER the mapping
                        // is installed (a failed/raced attempt leaves no
                        // phantom dirty): a writable mapping MAY write;
                        // flushing an unwritten page is harmless I/O.
                        // Cleared only when no mapping holds a pin
                        // anymore (flush_done). pte -> inodes order,
                        // same as the release sites.
                        // ext4 only: the flush walker is the ext4
                        // writeback; tmpfs aliases have no disk and
                        // their frames are authoritative while mapped.
                        if let Some(inode) = crate::fs::unsafe_edges::file_inode(&file) {
                            crate::fs::page_cache::get_page_cache().mark_dirty(
                                inode.fs_id,
                                inode.ino as u64,
                                (file_offset / PAGE_SIZE_USIZE) as u64,
                            );
                        }
                    }
                    mm.add_rss(1);
                    return MmFaultResult::Handled;
                }
            }
        }
    }

    // Fault-around (session 14; filemap_fault rework, session 23): on
    // read/exec faults into FileBacked VMAs, fill and map up to
    // FAULT_AROUND_PAGES pages starting at the faulting page — one VMA
    // find, one PTE-lock section. For page-cache-coherent files (ext4)
    // the window pages are the CACHE frames themselves: pinned per
    // mapping, mapped read-only (writable private VMAs get the COW bit —
    // Linux writenotify/do_read_fault semantics). One resident copy per
    // file page, shared by every mapper and every refault, reclaimable
    // the moment the last mapping drops its pin. This replaces the old
    // private-copy fill, which allocated an EXCLUSIVE unreclaimable copy
    // per fault with no page-descriptor state — the fake-OOM drainer
    // (session 22 attribution: 539-558k pages, 74-76% of the zone at
    // OOM, while the same data also lived in the page cache).
    // Non-ext4 files and holes keep the private-copy fill (now with
    // proper page bookkeeping). Write faults opt out (Linux
    // do_fault_around parity — the single-page arm below COW-fills
    // them), the SHARED-devfs path and the SHARED ext4 PCACHE arm above
    // have already returned, and SHARED VMAs never reach here at all
    // (their W-in-place semantics belong to the dedicated arm).
    if vma_type == VmaType::FileBacked
        && vma_file_fd >= 0
        && !is_write
        && !vma_flags.contains(crate::mm::vma::VmaFlags::SHARED)
    {
        let file = vm_file.clone().or_else(|| {
            // SAFETY: fd-table lookup by number for the CURRENT task,
            // same as the single-page fill arm; we are in that task's
            // fault context.
            crate::fs::get_file_fd(vma_file_fd as usize)
        });
        if let Some(file) = file {
            let fault_page = page_virt_addr.as_usize() & !(PAGE_SIZE_USIZE - 1);
            // Window bounds: the VMA end (INV-VMA-4 page-aligned), then
            // EOF — a page whose file offset is >= effective_size stays
            // UNMAPPED so a later touch re-faults into the SIGBUS gate
            // above (LTP mmap13 truncate semantics unchanged).
            let eof_limit = vma_start + effective_size.saturating_sub(vma_offset);
            let win_end = (fault_page + FAULT_AROUND_PAGES * PAGE_SIZE_USIZE)
                .min(vma_end)
                .min(eof_limit);
            // fault_page < win_end always: the SIGBUS gate above passed
            // for the faulting page. Ceil so a partial final page at the
            // window edge is included (its offset is still < effective_size).
            let n_pages = (win_end - fault_page + PAGE_SIZE_USIZE - 1) / PAGE_SIZE_USIZE;

            // Fill phase — NO locks held (R73: read_at may block on I/O;
            // the vma guard was dropped above and the pte lock must not
            // serialize I/O across mms). Cache pages are PINNED here
            // (pin released by the map-phase losers below, or owned by
            // the PTE via the PCACHE bit on success).
            let mut phys: [u64; FAULT_AROUND_PAGES] = [0; FAULT_AROUND_PAGES];
            let mut from_cache = [false; FAULT_AROUND_PAGES];
            let mut filled = 0usize;
            // The demand-populate capability is the ext4 probe (only
            // ext4 fills the page cache through the read machinery on a
            // fault; tmpfs pages are always resident).
            let cacheable = file.get_ops().is_some_and(|ops| ops.fault.is_some());
            for i in 0..n_pages {
                let off_i = vma_offset + (fault_page - vma_start) + i * PAGE_SIZE_USIZE;
                if cacheable {
                    // hole_as_zero = true: the reference instantiates a
                    // page-cache folio for HOLE reads too (block_read_full
                    // zeroes the folio and marks it uptodate IN THE CACHE);
                    // a private read fault maps that shared zero folio
                    // read-only with the COW bit below. Keeping holes out
                    // of the cache made a hole page faulted by one process
                    // invisible to every other mapper's mincore (the
                    // reference's mincore consults the page cache; LTP
                    // mincore04 locks a ftruncate-only file in a child and
                    // reads residency from the parent).
                    if let Some(p) =
                        filemap_pin_cache_page(file.as_ref(), off_i, effective_size, true)
                    {
                        phys[filled] = p;
                        from_cache[filled] = true;
                        filled += 1;
                        continue;
                    }
                    // Populate failure only — holes are covered above.
                }
                // Partial window on allocator failure is fine (Linux
                // behaves the same); zero filled pages means the single
                // page path below reports OutOfMemory.
                let Some(pa) = alloc_user_phys_page() else {
                    break;
                };
                let page_ptr = phys_to_virt(PhysAddr::new(pa)).bits() as *mut u8;
                // SAFETY: page_ptr is phys_to_virt of a freshly
                // allocated, exclusively owned page; fill_file_page only
                // writes within its PAGE_SIZE bounds.
                fill_file_page(file.as_ref(), off_i, page_ptr, effective_size);
                phys[filled] = pa;
                filled += 1;
            }

            if filled > 0 {
                // Base flags: NEVER W for cache-backed leaves — the frame
                // is shared with the page cache; a writable private VMA
                // gets the COW bit and the first write copies
                // (do_cow_fault parity). Private fill copies (holes,
                // non-ext4) keep the old writable-direct rule: they are
                // exclusive fresh pages.
                let mut pte_flags = PageTableEntry::P
                    | PageTableEntry::A
                    | PageTableEntry::D
                    | PageTableEntry::NX
                    | PageTableEntry::U;
                if vma_flags.is_executable() {
                    pte_flags &= !PageTableEntry::NX;
                }
                let cow_bit = if vma_flags.is_writable() {
                    crate::arch::mm::mm_ops::cow_flags::COW
                } else {
                    0
                };

                // Re-verify the VMA snapshot under a RE-TAKEN read guard
                // and hold it across the whole map section (INV-LOCK-7
                // order vma -> pte; the PROT_NONE marker path above
                // already nests this way). Every mm-side mutator of this
                // range — munmap, mprotect split, MAP_FIXED replace —
                // needs the vma WRITE lock, so it cannot interleave
                // between the verify and the maps: the stale-capture
                // race (filling against a VMA that got replaced by a
                // different mapping, then installing file content into
                // it) is closed for the whole batch at once.
                let vma_mgr2 = mm.vma_read();
                let snapshot_ok = vma_mgr2
                    .find(PageVirtAddr::new(fault_page))
                    .is_some_and(|v| {
                        v.start().as_usize() == vma_start
                            && v.end().as_usize() >= win_end
                            && v.offset() == vma_offset
                            && v.file_fd() == vma_file_fd
                            && v.flags() == vma_flags
                    });
                if !snapshot_ok {
                    drop(vma_mgr2);
                    for i in 0..filled {
                        release_fill_page(phys[i], from_cache[i]);
                    }
                    // VMA changed under us: retry, the fresh fault takes
                    // current VMA state (a now-dead range Segfaults, a
                    // re-flagged one re-faults through its own perms).
                    return MmFaultResult::Handled;
                }

                // One PTE-lock section for the whole batch: a concurrent
                // fork COW walk cannot observe a half-mapped window, and
                // per-mm faults serialize against it once (not 16 times).
                let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
                let mut mapped = 0u64;
                let mut fault_pg_failed = false;
                for i in 0..filled {
                    let va_i = VirtAddr::new((fault_page + i * PAGE_SIZE_USIZE) as u64);
                    // Re-check per page (E8-MM device-window-hole rule).
                    // A page that lost a race (concurrent CLONE_VM fault
                    // or a racing mprotect marker) releases its pin/copy
                    // and is skipped.
                    if  PageTableWalker::walk(root_ppn, va_i.bits() as u64) 
                        .map_or(false, |(_ppn, bits)| {
                            pte_counts_as_mapped(bits, va_i.bits() as u64)
                        })
                    {
                        release_fill_page(phys[i], from_cache[i]);
                        continue;
                    }
                    let flags_i = if from_cache[i] {
                        pte_flags | cow_bit | crate::arch::mm::mm_ops::cow_flags::PCACHE
                    } else if vma_flags.is_writable() {
                        pte_flags | PageTableEntry::W
                    } else {
                        pte_flags
                    };
                    // SAFETY: root_ppn is a valid root, va_i is
                    // page-aligned and VMA-verified, phys[i] is a freshly
                    // filled private page or a pinned cache frame, flags_i
                    // built from the verified VMA's permissions.
                    {
                        if !map_page(root_ppn, va_i, PhysAddr::new(phys[i]), flags_i) {
                            // PT allocation failure (fake-OOM family):
                            // release the data page (anti-leak). Around-pages
                            // just shrink the window (Linux parity); the
                            // FAULTING page failing means OOM.
                            release_fill_page(phys[i], from_cache[i]);
                            if i == 0 {
                                fault_pg_failed = true;
                            }
                            continue;
                        }
                        crate::arch::cpu::invlpg(va_i.bits());
                    }
                    // Bookkeeping INSIDE the lock (a concurrent fork walk
                    // must not observe the map before the refcounts).
                    if from_cache[i] {
                        // SAFETY: phys[i] is a cache frame pinned for THIS
                        // mapping, just mapped under the PTE lock.
                        filemap_bookkeep_mapped_cache_page(phys[i]);
                    } else {
                        // Private fill copy: an exclusive anonymous page
                        // (the session-22 "rmap not wired yet" gap —
                        // without flags these were the census-invisible
                        // unreclaimable copies).
                        use crate::mm::page_desc::{page_desc, PageFlag};
                        // Freshly allocated, exclusively owned (INV-FRAME-1).
                        if let Some(page) = page_desc((phys[i] >> PAGE_SHIFT) as usize) {
                            page.set_flag(PageFlag::Anonymous);
                            page.set_flag(PageFlag::SwapBacked);
                            page.set_index(va_i.bits() as usize / PAGE_SIZE_USIZE);
                            page.inc_mapcount();
                            crate::mm::rmap::page_record_mapping(
                                page,
                                mm as *const _ as usize,
                                va_i.bits() as usize,
                            );
                            crate::mm::lru::page_add_anon_lru(page);
                        }
                    }
                    mapped += 1;
                }
                drop(_pte_guard);
                drop(vma_mgr2);

                if mapped > 0 {
                    mm.add_rss(mapped);
                }
                if fault_pg_failed {
                    return MmFaultResult::OutOfMemory;
                }
                // The faulting page is either mapped here or lost a race
                // to an identical mapping — retry the instruction either
                // way (R8-4: Handled, never AlreadyMapped, so the loser
                // is not killed).
                return MmFaultResult::Handled;
            }
            // filled == 0: the allocator failed on the first page; fall
            // through so the single-page path returns OutOfMemory.
        }
    }

    // sc181: anonymous write-fault forward fault-around (hilogd-ring
    // family). sc179/sc180 KILLDUMPs: hilogd burned 91s of kernel stime
    // in 16 min faulting its 10MB anonymous log ring page-by-page —
    // 2560 full trap/VMA-lookup/alloc/zero/map/rmap/LRU passes per cycle
    // (the VmaManager::find_mut attribution). One trap now installs up
    // to ANON_FAULTAROUND_PAGES forward zero pages for sequential
    // writers, in the same batch shape as the file fault-around above.
    // Gates: WRITE faults only, VMAs >= 1MB, not SHARED, forward-only,
    // stops at the first already-mapped page (a downward-growing stack
    // fault finds its older frames immediately above and degrades to
    // single-page), best-effort on allocation. Window pages get the full
    // Anonymous bookkeeping — identical to individually faulted pages.
    // `faaround=0` disables. mincore03's untouched-mmap contract is safe:
    // no fault means no window.
    if vma_type == VmaType::Anonymous
        && is_write
        && !vma_flags.contains(crate::mm::vma::VmaFlags::SHARED)
        && vma_end - vma_start >= ANON_FAULTAROUND_MIN_VMA
        && crate::cmdline::get_param("faaround")
            .map(|v| v != "0")
            .unwrap_or(true)
    {
        let fault_page = page_virt_addr.as_usize() & !(PAGE_SIZE_USIZE - 1);
        // Window: forward pages up to the first ALREADY-MAPPED one (the
        // sequential-writer model; stack faults exit at the first hop),
        // bounded by the VMA end and ANON_FAULTAROUND_PAGES.
        let max_end = (fault_page + ANON_FAULTAROUND_PAGES * PAGE_SIZE_USIZE).min(vma_end);
        let mut win_end = fault_page + PAGE_SIZE_USIZE;
        while win_end < max_end {
            if  PageTableWalker::walk(root_ppn, win_end as u64) 
                .map_or(false, |(_p, bits)| {
                    pte_counts_as_mapped(bits, win_end as u64)
                })
            {
                break;
            }
            win_end += PAGE_SIZE_USIZE;
        }
        let n_pages = (win_end - fault_page) / PAGE_SIZE_USIZE;

        // Fill phase — NO locks held (same discipline as the file batch).
        let mut phys: [u64; ANON_FAULTAROUND_PAGES] = [0; ANON_FAULTAROUND_PAGES];
        let mut filled = 0usize;
        for _i in 0..n_pages {
            // Partial window on allocator failure is fine (Linux parity);
            // filled == 0 falls through to the single-page OOM return.
            let Some(pa) = alloc_user_phys_page() else {
                break;
            };
            // SAFETY: phys_to_virt of a freshly allocated, exclusively
            // owned page; the memset stays within its PAGE_SIZE bounds.
            crate::mm::edges::page_slice_mut(phys_to_virt(PhysAddr::new(pa)).bits() as usize).fill(0);
            phys[filled] = pa;
            filled += 1;
        }

        if filled > 0 {
            let mut pte_flags = PageTableEntry::P
                | PageTableEntry::A
                | PageTableEntry::D
                | PageTableEntry::NX
                | PageTableEntry::U;
            if vma_flags.is_writable() {
                pte_flags |= PageTableEntry::W;
            }
            if vma_flags.is_executable() {
                pte_flags &= !PageTableEntry::NX;
            }

            // Re-verify the VMA snapshot under a re-taken read guard
            // (INV-LOCK-7 order vma -> pte; same race closure as the
            // file batch above — every mm-side mutator of this range
            // needs the vma WRITE lock).
            let vma_mgr2 = mm.vma_read();
            let snapshot_ok = vma_mgr2
                .find(PageVirtAddr::new(fault_page))
                .is_some_and(|v| {
                    v.start().as_usize() == vma_start
                        && v.end().as_usize() >= win_end
                        && v.flags() == vma_flags
                        && v.vma_type() == VmaType::Anonymous
                });
            if !snapshot_ok {
                drop(vma_mgr2);
                for i in 0..filled {
                    crate::mm::page_alloc::free_page(phys[i] as usize);
                }
                // VMA changed under us: retry via the fresh fault.
                return MmFaultResult::Handled;
            }

            // One PTE-lock section for the whole batch.
            let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
            let mut mapped = 0u64;
            let mut fault_pg_failed = false;
            for i in 0..filled {
                let va_i = VirtAddr::new((fault_page + i * PAGE_SIZE_USIZE) as u64);
                // Re-check per page (E8-MM rule): a page that lost a race
                // frees its copy and is skipped.
                if  PageTableWalker::walk(root_ppn, va_i.bits() as u64) 
                    .map_or(false, |(_p, bits)| {
                        pte_counts_as_mapped(bits, va_i.bits() as u64)
                    })
                {
                    crate::mm::page_alloc::free_page(phys[i] as usize);
                    continue;
                }
                // SAFETY: root_ppn valid, va_i page-aligned and
                // VMA-verified, phys[i] a freshly zeroed page.
                {
                    if !map_page(root_ppn, va_i, PhysAddr::new(phys[i]), pte_flags) {
                        crate::mm::page_alloc::free_page(phys[i] as usize);
                        if i == 0 {
                            fault_pg_failed = true;
                        }
                        continue;
                    }
                    crate::arch::cpu::invlpg(va_i.bits());
                }
                // Bookkeeping INSIDE the lock — the Anonymous arm of the
                // single-page path below, verbatim.
                {
                    use crate::mm::page_desc::{page_desc, PageFlag};
                    // Freshly allocated, exclusively owned until the map
                    // above (INV-FRAME-1).
                    if let Some(page) = page_desc((phys[i] >> PAGE_SHIFT) as usize) {
                        page.set_flag(PageFlag::Anonymous);
                        page.set_flag(PageFlag::SwapBacked);
                        page.set_index(va_i.bits() as usize / PAGE_SIZE_USIZE);
                        page.inc_mapcount();
                        crate::mm::rmap::page_record_mapping(
                            page,
                            mm as *const _ as usize,
                            va_i.bits() as usize,
                        );
                        crate::mm::lru::page_add_anon_lru(page);
                    }
                }
                mapped += 1;
            }
            drop(_pte_guard);
            drop(vma_mgr2);

            if mapped > 0 {
                mm.add_rss(mapped);
            }
            if fault_pg_failed {
                return MmFaultResult::OutOfMemory;
            }
            return MmFaultResult::Handled;
        }
        // filled == 0: allocator failed on the faulting page itself;
        // fall through so the single-page path returns OutOfMemory.
    }

    // Allocate new page
    let phys_addr = match alloc_user_phys_page() {
        Some(addr) => PhysAddr::new(addr),
        None => return MmFaultResult::OutOfMemory,
    };

    // Convert physical address to virtual address for kernel access
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;

    // Initialize page content based on type: page_ptr is from
    // phys_to_virt() on a freshly allocated, exclusively owned page
    // (INV-XE-ZP); each raw op below carries its own block.
    {
        match vma_type {
            VmaType::Anonymous => {
                // SAFETY: fresh exclusive page (INV-XE-ZP).
                crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
            }
            VmaType::FileBacked => {
                // Fill from the pinned vm_file using the entry-time
                // captures (vma_start / vm_file / file_offset /
                // effective_size) — no per-fault VMA re-find, no
                // current() round trip (the R73 guard rule is honored:
                // no VMA guard is held across read_at; the values were
                // captured under the entry guard, which only needs to
                // be consistent with the map-side recheck below).
                //
                // Pre-zero only when a full-page overwrite is NOT
                // guaranteed: partial tail reads and no-file cases need
                // the zero base; a planned full-page read overwrites
                // every byte (short/failed reads re-zero below, so no
                // stale kernel data can leak into a user page).
                let file = vm_file
                    .clone()
                    .or_else(|| crate::fs::get_file_fd(vma_file_fd as usize));
                let will_full_read = file.is_some()
                    && file_offset < effective_size
                    && effective_size.saturating_sub(file_offset) >= PAGE_SIZE_USIZE;
                if !will_full_read {
                    // SAFETY: fresh exclusive page (INV-XE-ZP).
                    crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
                }

                // Read file data if we have a valid fd
                if vma_file_fd >= 0 {
                    // Read from file if within file bounds
                    if file_offset < effective_size {
                        // Prefer the VMA's pinned file (Linux vm_file):
                        // the mapping fd may already be closed —
                        // resolving by fd number failed silently and
                        // produced zero pages (Ubuntu ld.so).
                        if let Some(file) = file {
                            let bytes_to_read = core::cmp::min(
                                PAGE_SIZE_USIZE,
                                effective_size.saturating_sub(file_offset),
                            );
                            // R71: positional read — the old
                            // get_pos/set_pos/read/restore dance
                            // raced against concurrent plain
                            // read()s on the same pinned vm_file
                            // (CLONE_VM threads faulting while
                            // peers read) and served pages from
                            // the wrong offset. See the riscv64
                            // twin and File::read_at.
                            // SAFETY: read_at fills the fresh exclusive
                            // page (INV-XE-ZP); short tails re-zero below.
                            let bytes_read =
                                file.read_at_slice(file_offset as u64, &mut crate::mm::edges::page_slice_mut(page_ptr as usize)[..bytes_to_read]);

                            // Any read that does not verifiably cover the
                            // whole page — short tail, 0, or a NEGATIVE
                            // errno return — must leave the uncovered
                            // bytes zero. `will_full_read` skipped the
                            // pre-zero, so without this a failed read maps
                            // recycled-buddy content (a previous owner's
                            // bytes) into user space — the OH leg-22
                            // dirty-bss-slack-page evidence class. The old
                            // `bytes_read == 0` check missed negative
                            // returns (riscv64 twin: fill_file_page; the
                            // riscv64 single-page path always pre-zeroes).
                            let good = if bytes_read > 0 {
                                bytes_read as usize
                            } else {
                                0
                            };
                            if will_full_read && good < PAGE_SIZE_USIZE {
                                // SAFETY: same fresh page; tail zero.
                                crate::mm::edges::page_slice_mut(page_ptr as usize)[good..]
                                    .fill(0);
                            }
                        }
                    }
                    // Beyond file size: page stays zero-filled (sparse / hole)
                }
            }
            VmaType::Device => {
                // Device mapping: don't zero
            }
            VmaType::SharedMemory => {
                // SAFETY: fresh exclusive page (INV-XE-ZP).
                crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
            }
        }
    }

    // Build page table entry flags (x86: exec is default and non-exec is
    // NX; write adds RW; user pages carry US. A write-only VMA folds to
    // RW — write requires read on x86, mirroring the riscv twin's
    // W-requires-R rule.)
    let mut pte_flags =
        PageTableEntry::P | PageTableEntry::A | PageTableEntry::D | PageTableEntry::NX;
    pte_flags |= PageTableEntry::U; // User page

    if vma_flags.is_writable() {
        // MAP_PRIVATE file-backed pages: map writable directly.
        // The page was just allocated and is exclusive (refcount=1), so no COW
        // is needed. COW marking only happens in copy_page_table_cow() during
        // fork when the page actually becomes shared (refcount >= 2).
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags &= !PageTableEntry::NX;
    }

    // Map page under the PTE-modify lock and keep it held across the rmap
    // setup: a concurrent fork's copy_page_table_cow walk landing between
    // the map and the rmap/refcount update takes a reference the rmap
    // never sees (round 6 MED — demand-fault PTE lock).
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
    // Re-check under the lock (same double-fault race as above; E8-MM
    // device-window-hole rule applies).
    if  PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) 
        .map_or(false, |(_ppn, bits)| {
            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
        })
    {
        drop(_pte_guard);
        // Lost the race: free our exclusively-owned fresh page and let the
        // caller retry — the next entry sees the mapping present.
        crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
        // R8-4: Handled — see the stack-growth path (no SIGSEGV for the
        // race loser).
        return MmFaultResult::Handled;
    }
    // Map page (root_ppn valid, fault_addr page-aligned, phys_addr
    // fresh, pte_flags well-formed — the walk primitive's contract).
    {
        if !map_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure (fake-OOM family): free the data
            // page, deliver OOM (trap layer SIGKILLs the user task).
            crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
            return MmFaultResult::OutOfMemory;
        }

        // Address-specific TLB flush
        crate::arch::cpu::invlpg(fault_addr.bits());
    }

    // Set up reverse mapping for the newly mapped page
    {
        use crate::mm::page_desc::{page_desc, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        // The page was just allocated and mapped (INV-FRAME-1 handle).
        if let Some(page) = page_desc(page_pfn) {
            {
                match vma_type {
                    VmaType::Anonymous => {
                        page.set_flag(PageFlag::Anonymous);
                        // SwapBacked + LRU membership: required for vmscan
                        // to consider this page for swap-out (see the stack
                        // growth path above).
                        page.set_flag(PageFlag::SwapBacked);
                        page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        page.inc_mapcount();
                        // Multi-mapping bookkeeping: record (mm, vpn) so
                        // try_to_unmap can find re-mapped instances of this
                        // frame.
                        crate::mm::rmap::page_record_mapping(
                            page,
                            mm as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                        crate::mm::lru::page_add_anon_lru(page);
                    }
                    VmaType::SharedMemory => {
                        page.set_flag(PageFlag::Anonymous);
                        page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        page.inc_mapcount();
                        // Shmem stays off the anon LRU: try_to_unmap only
                        // walks VmaType::Anonymous VMAs, so marking these
                        // SwapBacked would only add reclaim scan churn.
                        crate::mm::rmap::page_record_mapping(
                            page,
                            mm as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                    }
                    VmaType::FileBacked => {
                        // Private file copy (write-fault COW fill on a
                        // private VMA — do_cow_fault parity; also the
                        // non-ext4/hole fallbacks). Session 22: this arm
                        // used to leave the page with NO descriptor state
                        // ("rmap not wired yet") — census-invisible and
                        // unreclaimable, the fake-OOM drainer's
                        // accounting face. An exclusive copy IS an
                        // anonymous page from reclaim's point of view.
                        page.set_flag(PageFlag::Anonymous);
                        page.set_flag(PageFlag::SwapBacked);
                        page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        page.inc_mapcount();
                        crate::mm::rmap::page_record_mapping(
                            page,
                            mm as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                        crate::mm::lru::page_add_anon_lru(page);
                    }
                    _ => {
                        // Device mappings: no descriptor state (frames
                        // belong to the device/foreign owners).
                    }
                }
            }
        }
    }
    drop(_pte_guard);

    // RSS accounting: this address space just gained a resident page.
    mm.add_rss(1);

    MmFaultResult::Handled
}

/// Read the raw PTE value at a virtual address.
///
/// Walks the four-level page table and returns the raw bits of the leaf
/// PTE, even if P=0 (e.g. a swap entry). Returns None if the walk cannot
/// reach the leaf level.
/// R-WRITESTORM-2: is this leaf a PROT_NONE marker (P=0, U=1, non-swap,
/// non-migration, with a preserved PPN) as written by mprotect_pte_range
/// for prot==0? Such a leaf is NOT a mapping decision — the covering VMA
/// owns protection; the fault layer re-presents it when the VMA grants
/// the access.
pub(crate) fn is_protnone_marker(raw: u64) -> bool {
    (raw & PageTableEntry::P) == 0
        && (raw & PageTableEntry::U) != 0
        && !crate::mm::swap::is_swap_entry(raw)
        && !crate::mm::swap::is_migration_entry(raw)
        && (raw & PageTableEntry::PHYS_MASK_PUBLIC) != 0
}

/// Handle a swap fault — read the page back from swap and map it.
///
/// Steps:
/// 1. Extract swap type and offset from the PTE
/// 2. Allocate a physical page
/// 3. Read page contents from the swap device
/// 4. Build PTE flags from VMA permissions
/// 5. Map the page and flush TLB
/// 6. Set up rmap (anonymous + SwapBacked)
/// 7. Free the swap slot
fn handle_swap_fault(
    mm: &MmStruct,
    fault_addr: VirtAddr,
    flags: u32,
    swap_entry: u64,
    root_ppn: u64,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;
    use crate::mm::page_desc::{page_desc, PageFlag};
    use crate::mm::swap;

    // Extract swap type and offset
    let swap_type = swap::swap_entry_type(swap_entry);
    let swap_offset = swap::swap_entry_offset(swap_entry);

    // Find VMA for permission bits
    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());
    let vma_mgr = mm.vma_read();
    let vma = match vma_mgr.find(page_virt_addr) {
        Some(v) => v,
        None => return MmFaultResult::Segfault,
    };

    let vma_flags = vma.flags();
    let is_write = flags & FaultFlags::WRITE != 0;
    let is_exec = flags & FaultFlags::EXEC != 0;
    let is_read = flags & FaultFlags::READ != 0;

    // Verify permissions
    if is_write && !vma_flags.is_writable() {
        return MmFaultResult::PermissionDenied;
    }
    if is_exec && !vma_flags.is_executable() {
        return MmFaultResult::PermissionDenied;
    }
    if is_read && !vma_flags.is_readable() {
        return MmFaultResult::PermissionDenied;
    }

    drop(vma_mgr);

    // Allocate a physical page
    let phys_addr = match alloc_user_phys_page() {
        Some(addr) => addr,
        None => return MmFaultResult::OutOfMemory,
    };

    // Wait for an in-flight swap-out of this slot: the single-mapping
    // reclaim path installs the swap entry in the PTE BEFORE the device
    // write; reading the slot before that write lands would swap in
    // garbage. Bounded spin — the writer is a block write on another
    // context.
    {
        let mut spins: u64 = 0;
        while swap::swap_slot_pending(swap_type, swap_offset) {
            spins += 1;
            if spins > 64_000_000 {
                break; // give up waiting; the read below fails -> retry
            }
            core::hint::spin_loop();
        }
    }

    // Read page contents from swap device
    if swap::swap_read_page(swap_type, swap_offset, phys_addr as usize).is_err() {
        crate::println!(
            "swap: failed to read page from swap (type={}, offset={})",
            swap_type,
            swap_offset
        );
        // R21-6: free the freshly allocated page on the failed read (was a
        // per-error page leak).
        crate::mm::page_alloc::free_page(phys_addr as usize);
        return MmFaultResult::OutOfMemory;
    }

    // Build PTE flags from VMA permissions
    let mut pte_flags =
        PageTableEntry::P | PageTableEntry::A | PageTableEntry::D | PageTableEntry::NX;
    pte_flags |= PageTableEntry::U;

    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags &= !PageTableEntry::NX;
    }

    // Map the page
    // Swap-in map under the PTE-modify lock (round 6 MED). The swap read
    // finished above, so no I/O happens inside the irqsave section.
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
    // Re-check under the lock (same double-fault race as the demand
    // paths). The swap-in page is exclusively owned, so freeing on loss is
    // safe; the swap slot is freed by the winner. E8-MM: device-window-
    // hole rule applies here too (see pte_counts_as_mapped).
    if  PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) 
        .map_or(false, |(_ppn, bits)| {
            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
        })
    {
        drop(_pte_guard);
        crate::mm::page_alloc::free_page(phys_addr as usize);
        // R8-4: Handled — see the demand-fault sites (no SIGSEGV for the
        // race loser).
        return MmFaultResult::Handled;
    }
    // SAFETY: root_ppn is a valid page table root, phys_addr was just
    // allocated, and pte_flags are built from valid VMA permissions. The
    // page is exclusively owned.
    {
        if !map_page(root_ppn, fault_addr, PhysAddr::new(phys_addr), pte_flags) {
            // PT allocation failure (fake-OOM family): free the swap-in
            // page, deliver OOM.
            crate::mm::page_alloc::free_page(phys_addr as usize);
            return MmFaultResult::OutOfMemory;
        }

        // TLB flush
        crate::arch::cpu::invlpg(fault_addr.bits());
    }

    // Set up rmap for the swapped-in page
    let page_pfn = (phys_addr >> PAGE_SHIFT) as usize;
    // Fresh exclusive page (INV-FRAME-1).
    if let Some(page) = crate::mm::page_desc::page_desc(page_pfn) {
        page.set_flag(PageFlag::Anonymous);
        page.set_flag(PageFlag::SwapBacked);
        page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
        page.inc_mapcount();
        crate::mm::rmap::page_record_mapping(
            page,
            mm as *const _ as usize,
            fault_addr.bits() as usize,
        );

        // Add back to anon LRU
        crate::mm::lru::page_add_anon_lru(page);
    }
    drop(_pte_guard);

    // Free the swap slot (page is back in memory)
    swap::swap_free_slot(swap_type, swap_offset);

    // RSS accounting: the page is resident again.
    mm.add_rss(1);

    MmFaultResult::Handled
}

// ==================== VMA/mmap/brk/fork policy (moved from mm_ops.rs) ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages.
    pub fn map_vma(mm: &MmStruct, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let _ = perm; // demand paging: PTEs come from faults (perm kept in the VMA)
        let mut vma_mgr = mm.vma_write();

        let start = vma.start();
        let end = vma.end();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // Update virtual memory statistics
        let size = end.as_usize().saturating_sub(start.as_usize());
        let pages = (size / PAGE_SIZE_USIZE) as u64;
        mm.add_total_vm(pages);
        mm.update_highest_vm_end(end.as_usize());

        Ok(())
    }

    /// Adjust heap pointer (brk system call)
    /// LEGACY/UNUSED: the live brk path is sys_brk (syscall/memory.rs,
    /// task.brk + VMA surgery + demand-fault pages); this eager-map
    /// variant has no callers (do_brk below is its only wrapper, also
    /// uncalled). Kept in sync with the mm.brk mirror for when an
    /// in-kernel caller reappears.
    pub fn set_brk(mm: &MmStruct, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        use user_addr::{HEAP_START, HEAP_MAX_SIZE, BRK_DEFAULT, MMAP_START};

        if new_brk.as_usize() == 0 {
            return Ok(mm.brk());
        }

        if mm.space_type() != PageTableType::User {
            return Err(MapError::Invalid);
        }

        let heap_end = BRK_DEFAULT + HEAP_MAX_SIZE;

        if new_brk.as_usize() < HEAP_START || new_brk.as_usize() > heap_end.min(MMAP_START) {
            return Ok(mm.brk());
        }

        let old_brk = mm.brk().as_usize();

        if new_brk.as_usize() < old_brk {
            mm.set_brk_val(new_brk.as_usize());
            return Ok(new_brk);
        }

        if new_brk.as_usize() > old_brk {
            let old_brk_aligned = old_brk & !(PAGE_SIZE_USIZE - 1);
            let new_brk_aligned = new_brk.as_usize() & !(PAGE_SIZE_USIZE - 1);

            let mut addr = old_brk_aligned;
            while addr < new_brk_aligned {
                if PageTableWalker::walk(mm.pgd, addr as u64).is_none() {
                    let phys_addr = alloc_pages(GfpFlags::GFP_KERNEL, 0);
                    if phys_addr == 0 {
                        return Err(MapError::OutOfMemory);
                    }
                    let flags = perm_to_flags(Perm::ReadWrite, mm.space_type());
                    // Serialize the leaf-PTE write + rmap setup against a
                    // concurrent fork()/COW walk on this mm — brk was the
                    // one leaf-PTE writer left outside the PTE section
                    // (same protocol as the demand-fault paths in
                    // page_fault.rs). Per-mm section (INV-LOCK-7).
                    let _pte_guard = pte_lock_for(mm).lock();
                    // Re-check under the lock: a racing thread of this mm
                    // (CLONE_VM) may have mapped this address between the
                    // walk above and here; mapping again would orphan the
                    // winner's page.
                    if PageTableWalker::walk(mm.pgd, addr as u64).is_some() {
                        drop(_pte_guard);
                        crate::mm::page_alloc::free_page(phys_addr);
                    } else {
                        {
                            if !map_page(
                                mm.pgd,
                                VirtAddr::new(addr as u64),
                                PhysAddr::new(phys_addr as u64),
                                flags,
                            ) {
                                // PT allocation failure (fake-OOM family):
                                // free the data page (anti-leak) and leave
                                // the address unmapped — the access faults
                                // later and the demand-fault path delivers
                                // the OOM kill (Linux pagefault-OOM
                                // semantics) instead of the old panic.
                                static HEAPMAP_N: core::sync::atomic::AtomicUsize =
                                    core::sync::atomic::AtomicUsize::new(0);
                                if HEAPMAP_N
                                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                                    < 8
                                {
                                    crate::pr_err!(
                                        "brk eager map: PT alloc failed at {:#x} — page left for demand fault",
                                        addr
                                    );
                                }
                                drop(_pte_guard);
                                continue;
                            }
                        }
                        // Set up reverse mapping for heap page
                        {
                            use crate::mm::page_desc::{page_desc, PageFlag};
                            if let Some(page) = page_desc(phys_addr / (PAGE_SIZE as usize)) {
                                page.set_flag(PageFlag::Anonymous);
                                // SwapBacked + LRU membership: heap pages
                                // are swap-out candidates too (vmscan).
                                page.set_flag(PageFlag::SwapBacked);
                                page.set_index(addr / (PAGE_SIZE as usize));
                                page.inc_mapcount();
                                crate::mm::rmap::page_record_mapping(
                                    page,
                                    mm as *const _ as usize,
                                    addr,
                                );
                                crate::mm::lru::page_add_anon_lru(page);
                            }
                        }
                        drop(_pte_guard);
                    }

                    let mut vma_mgr = mm.vma_write();
                    let mut vma_flags = VmaFlags::new();
                    vma_flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSUP);
                    let mut vma = Vma::new(
                        PageVirtAddr::new(addr),
                        PageVirtAddr::new(addr + PAGE_SIZE_USIZE),
                        vma_flags,
                    );
                    vma.set_origin(VmaOrigin::from_bits(VmaOrigin::BRK));
                    // Account the page like every other installed VMA.
                    let brk_page = vma.page_count() as u64;
                    let _ = vma_mgr.add(vma);
                    mm.add_total_vm(brk_page);
                }
                addr += PAGE_SIZE_USIZE;
            }

            mm.set_brk_val(new_brk.as_usize());
        }

        Ok(new_brk)
    }

    /// mmap system call implementation
    pub fn mmap(
        mm: &MmStruct,
        addr: PageVirtAddr,
        size: usize,
        flags: VmaFlags,
        vma_type: VmaType,
        perm: Perm,
        map_flags: u32,
    ) -> Result<PageVirtAddr, MapError> {
        use crate::arch::mm::memory_layout::map;

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
        // LTP shapes) all returned EINVAL (session-14/15 probe, MF1-3).
        // Refuse only the LIVE brk heap span
        // [align_down(start_brk), align(brk)): mapping over malloc's
        // own storage stays EINVAL (deliberate divergence — Linux would
        // unmap the heap; we never silently free brk-managed pages).
        // Everything above the live heap is safe: the fixed-path surgery
        // below splits/zaps/replaces whatever VMA+PTE state exists, brk
        // growth into foreign VMAs is refused by sys_brk's VmaManager
        // overlap check, and the mm.brk mirror is synced by sys_brk so
        // this span tracks the real break.
        let brk_cur = mm.brk().as_usize();
        let heap_lo = mm.start_brk() & !(PAGE_SIZE_USIZE - 1);
        let heap_hi = if brk_cur > mm.start_brk() {
            (brk_cur + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1)
        } else {
            heap_lo
        };
        let has_brk_conflict = addr.as_usize() < heap_hi && end_addr > heap_lo;

        // ATOMIC find + insert: the non-fixed paths pick the address AND
        // install the VMA under ONE vma_write guard. The old shape
        // (find_free_area on a read lock, then map_vma's add on a write
        // lock) left a window where two threads of the same mm were both
        // handed the same free address; the second VmaManager::add at the
        // same start key silently REPLACED the first (BTreeMap insert),
        // both mappings then demand-faulted the same VAs, and the PTE
        // overwrite aliased the two mappings' frames — silent cross-task
        // heap corruption (gnome-shell malloc.c:4302 _int_malloc assert;
        // reproduced by test/gnome-x86/heapstress.c within a minute via
        // glibc's own arena/mremap churn once blocks cross the mmap
        // threshold).
        let mut vma_installed = false;
        let start = if is_fixed {
            let start = addr;
            if start.as_usize() % PAGE_SIZE_USIZE != 0 {
                return Err(MapError::Invalid);
            }
            if start.as_usize() < user_addr::USER_START {
                return Err(MapError::Invalid);
            }
            // mmap_min_addr parity: the reference kernel refuses a
            // MAP_FIXED below vm.mmap_min_addr for callers without
            // CAP_SYS_RAWIO (EPERM). Our USER_START is 0, so before
            // this check a fixed mapping at address 0 was accepted and
            // the null page was mappable (guest probe: the kernel
            // without this check returned 0 for mmap(0, 4K, MAP_FIXED)).
            // No correct program maps there (placement never puts a
            // mapping below MIN_MAP_ADDR, lower hints are discarded),
            // so the guard is unobservable to legitimate callers.
            //
            // Scope: a hardening step, NOT the fix for the NOVMA-EXEC
            // storm. That storm's destroy is a direct
            // munmap(0, 0x3fbf0bb000) syscall from the victim's
            // userspace; it never enters this path (the kernel frame
            // walk and the mm-op ring both show sys_munmap only) and
            // it still reproduces with this check in place. Which
            // caller issues it (application or libc internals) and why
            // is not yet identified; that hunt is tracked separately.
            //
            // Deliberate divergence from the reference, for now: EINVAL
            // instead of EPERM, and no CAP_SYS_RAWIO exemption — the
            // refusal has to hold for every caller until the cause of
            // the bogus munmap is found. MIN_MAP_ADDR (4 KiB) is this
            // kernel's own floor; the reference's default sysctl value
            // is 65536.
            if start.as_usize() < user_addr::MIN_MAP_ADDR {
                return Err(MapError::Invalid);
            }
            // Upper bound: a fixed mapping at or above USER_END would write
            // leaf PTEs into the kernel-shared upper tables (user page
            // tables copy the kernel PML4 entries) — reject outright.
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
        } else if map_flags & map::MAP_32BIT != 0 || addr.as_usize() == 0 {
            // ATOMIC pick + insert for the kernel-chosen placements too
            // (mm-audit, OH heap-corruption hunt): MAP_32BIT and the
            // no-hint anonymous path (musl pthread stacks, mallocng
            // groups — the hottest concurrent-mmap paths) previously ran
            // find_free_area under a READ lock and added the VMA later
            // under a separate WRITE lock. Two same-mm threads in that
            // window were both handed the same address; the loser's add
            // then failed and userspace saw a bogus EINVAL (Linux never
            // fails here short of ENOMEM), or worse, the interleaving
            // with MAP_FIXED's multi-phase surgery dropped a live VMA
            // piece. Pick AND insert under ONE guard, like the hint path
            // below and the riscv64 twin.
            let mp = crate::debug::mmprof::on();
            let mp_tl = if mp { crate::debug::mmprof::now() } else { 0 };
            let mut vma_mgr = mm.vma_write();
            if mp {
                crate::debug::mmprof::add(crate::debug::mmprof::M_LOCKWAIT, mp_tl);
            }
            let mp_tf = if mp { crate::debug::mmprof::now() } else { 0 };
            let chosen = if map_flags & map::MAP_32BIT != 0 {
                // Linux x86_64 arch_get_unmapped_area: MAP_32BIT forces
                // the mapping into the FIRST 2GB, bottom-up from 1GB
                // (observed Linux placement: a MAP_32BIT mmap on the
                // host lands just above 0x40000000). The caller's hint
                // is subordinate to the constraint. OH's musl ld.so
                // issues MAP_PRIVATE|MAP_32BIT for every library header
                // probe.
                find_free_area_32bit_locked(mm, &vma_mgr, aligned_size)?
            } else {
                find_free_area_locked(mm, &vma_mgr, aligned_size)?
            };
            if mp {
                crate::debug::mmprof::add(crate::debug::mmprof::M_FINDFREE, mp_tf);
            }
            let mp_ta = if mp { crate::debug::mmprof::now() } else { 0 };
            let vend = PageVirtAddr::new(chosen.as_usize() + aligned_size);
            let mut vma = Vma::new(chosen, vend, flags);
            vma.set_type(vma_type);
            vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
            vma_mgr
                .add(vma)
                .map_err(|_| MapError::Invalid)?;
            drop(vma_mgr);
            mm.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
            mm.update_highest_vm_end(chosen.as_usize() + aligned_size);
            if mp {
                crate::debug::mmprof::add(crate::debug::mmprof::M_ADD, mp_ta);
            }
            vma_installed = true;
            chosen
        } else {
            // Non-fixed placement: reserve atomically (see the comment at
            // vma_installed above). Linux honors a free hint ANYWHERE in
            // the user range (arch_get_unmapped_area): the hint is
            // rounded DOWN and used when its gap is free, otherwise the
            // kernel-chosen search runs. The historical blanket discard
            // of hints below MMAP_START is gone (session-17 parity):
            // low hints (arena-class placement requests, LTP shapes)
            // now land where Linux would place them. Like Linux the
            // hint is IGNORED (never an error) when it is below
            // MIN_MAP_ADDR (mmap_min_addr analog), lands on the LIVE
            // brk heap span (Linux's heap is a VMA from exec time; ours
            // below MMAP_START can be mirror-only before the first
            // growth, so the same span check as the fixed path applies),
            // or ends past USER_END (the syscall layer pre-checked the
            // raw length; rounding the length UP can still push the
            // last page over). Residual divergence, documented: no
            // stack-guard-gap reservation below GROWSDOWN VMAs (Linux
            // vm_start_gap reserves 1MB) — this VMA has no guard-gap
            // model. The round-down also closes a kernel panic vector:
            // an unaligned hint used to reach Vma::new and trip its
            // page-alignment assert (Vma::new asserts; Linux masks the
            // hint instead).
            let mp = crate::debug::mmprof::on();
            let mp_tl = if mp { crate::debug::mmprof::now() } else { 0 };
            let mut vma_mgr = mm.vma_write();
            if mp {
                crate::debug::mmprof::add(crate::debug::mmprof::M_LOCKWAIT, mp_tl);
            }
            let mp_tf = if mp { crate::debug::mmprof::now() } else { 0 };
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
                    find_free_area_locked(mm, &vma_mgr, aligned_size)?
                }
            } else {
                find_free_area_locked(mm, &vma_mgr, aligned_size)?
            };
            if mp {
                crate::debug::mmprof::add(crate::debug::mmprof::M_FINDFREE, mp_tf);
            }
            let mp_ta = if mp { crate::debug::mmprof::now() } else { 0 };
            let vend = PageVirtAddr::new(chosen.as_usize() + aligned_size);
            let mut vma = Vma::new(chosen, vend, flags);
            vma.set_type(vma_type);
            vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
            vma_mgr
                .add(vma)
                .map_err(|_| MapError::Invalid)?;
            drop(vma_mgr);
            mm.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
            mm.update_highest_vm_end(chosen.as_usize() + aligned_size);
            if mp {
                crate::debug::mmprof::add(crate::debug::mmprof::M_ADD, mp_ta);
            }
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
            // concurrent mmap/MAP_FIXED on the same range — the reference
            // kernel permits any interleaving there too). The b89
            // piece-drop hole cannot recur: pieces are never held across
            // a guard drop by the chunked loop.
            let mp = crate::debug::mmprof::on();
            let mp_tf = if mp { crate::debug::mmprof::now() } else { 0 };
            let mut attempts = 0;
            loop {
                munmap(mm, start, aligned_size)?;
                let end = PageVirtAddr::new(start.as_usize() + aligned_size);
                let mut vma_mgr = mm.vma_write();
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
                mm.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
                mm.update_highest_vm_end(end.as_usize());
                if mp {
                    crate::debug::mmprof::add(crate::debug::mmprof::M_FIXEDMUN, mp_tf);
                }
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
            map_vma(mm, vma, perm)?;
        }
        Ok(start)
    }

    /// Find free virtual address area
    /// MAP_32BIT placement (Linux x86_64): bottom-up gap search inside
    /// the first 2GB, low limit 1GB — static ELF segments (0x400000+),
    /// the brk region below BRK_DEFAULT, and vDSO-less early mappings all
    /// live below; the gap walk skips whatever exists. ENOMEM when the
    /// 2GB window has no room.
    pub fn find_free_area_32bit(mm: &MmStruct, size: usize) -> Result<PageVirtAddr, MapError> {
        let vma_mgr = mm.vma_read();
        find_free_area_32bit_locked(mm, &vma_mgr, size)
    }

    /// Scan body of find_free_area_32bit under a lock the CALLER holds
    /// (read for the wrapper, write for mmap's atomic pick-and-insert —
    /// same TOCTOU contract as find_free_area_locked).
    fn find_free_area_32bit_locked(
        mm: &MmStruct,
        vma_mgr: &crate::mm::vma::VmaManager,
        size: usize,
    ) -> Result<PageVirtAddr, MapError> {
        const MAP32_LOW: usize = 0x4000_0000; // 1GB
        const MAP32_HIGH: usize = 0x8000_0000; // 2GB

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 || MAP32_HIGH - MAP32_LOW < aligned_size {
            return Err(MapError::Invalid);
        }

        let mut search_start = MAP32_LOW;
        for vma in vma_mgr.iter() {
            let vma_start = vma.start().as_usize();
            if vma_start > search_start
                && vma_start - search_start >= aligned_size
            {
                return Ok(PageVirtAddr::new(search_start));
            }
            let vma_end = vma.end().as_usize();
            if vma_end > search_start {
                search_start = (vma_end + PAGE_SIZE_USIZE - 1)
                    & !(PAGE_SIZE_USIZE - 1);
            }
            if search_start.saturating_add(aligned_size) > MAP32_HIGH {
                break;
            }
        }

        if search_start.saturating_add(aligned_size) <= MAP32_HIGH {
            return Ok(PageVirtAddr::new(search_start));
        }
        Err(MapError::OutOfMemory)
    }

    pub fn find_free_area(mm: &MmStruct, size: usize) -> Result<PageVirtAddr, MapError> {
        let vma_mgr = mm.vma_read();
        find_free_area_locked(mm, &vma_mgr, size)
    }

    /// Scan body of find_free_area, run under a lock the CALLER holds
    /// (read for the standalone wrapper, write for mmap's atomic
    /// pick-and-insert — see the TOCTOU comment in mmap). Both guard
    /// types deref to VmaManager.
    fn find_free_area_locked(
        mm: &MmStruct,
        vma_mgr: &crate::mm::vma::VmaManager,
        size: usize,
    ) -> Result<PageVirtAddr, MapError> {
        use user_addr::{MMAP_START, MMAP_END, USER_END};
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        let mut search_start = MMAP_START;
        let search_end = MMAP_END.min(USER_END.saturating_sub(aligned_size));

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
    /// split VMAs at the unmapped boundaries, remove the overlapping parts.
    pub fn munmap(mm: &MmStruct, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // s15 catch (see the riscv64 twin): the page-round add overflows
        // for user lengths within one page of usize::MAX — Linux returns
        // EINVAL for such ranges; reject as Invalid instead of panicking
        // (debug) or wrapping (release).
        let aligned_size = match size.checked_add(PAGE_SIZE_USIZE - 1) {
            Some(rounded) => rounded & !(PAGE_SIZE_USIZE - 1),
            None => return Err(MapError::Invalid),
        };

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        // R7-1: user-range bound — a kernel-range munmap would walk the
        // SHARED kernel PML4 entries, put_page/rmap kernel page descriptors
        // and zero shared kernel PTEs (NEW-C1 class; covers sys_munmap and
        // madvise(MADV_REMOVE), which both funnel through here).
        {
            use crate::arch::mm::memory_layout::user_addr;
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

        // (fork-frag lane) drain previously defer-freed leaf tables whose
        // target-CPU flush epochs have advanced — the free side of the
        // emptied-table protocol. Entry of every munmap: the natural
        // churn point, before any new work.
        crate::arch::mm::mmu_init::pt_pending_drain();

        // Per-piece cursor loop (89th session, the mallocng-corruption
        // root fix): each iteration removes at most MUNMAP_CHUNK_BYTES of
        // ONE VMA and zaps that piece's PTEs INSIDE the SAME vma_write()
        // guard as its surgery, then drops the guard, cond_resched()s, and
        // advances an explicit CURSOR — the search for the next piece
        // starts at the cursor, never back at addr_us.
        //
        // Both prior shapes had a data-destroying same-mm race:
        // - Face A (per-chunk re-derive from addr_us): between chunk
        //   guards a racing same-mm mmap legally landed in the freed hole
        //   BEHIND the processing position, and the NEXT chunk's
        //   re-derived search treated the FRESH VMA as an unprocessed
        //   piece of THIS munmap and deleted it (the arena-churn /
        //   fontconfig crash class). The cursor closes it: processed
        //   holes are never revisited.
        // - Face B (detach-all-then-zap with a >4MB deferred zap): the
        //   guard dropped with (VMA gone AND PTEs live) — the state the
        //   mm-audit invariant forbids. A racing same-mm mmap landed in
        //   the pending-zap hole (find_free_area prefers the LOWEST gap,
        //   i.e. the freshly freed FFRT arena), demand-faulted its pages
        //   in and STAMPED live mallocng group headers into them; the
        //   deferred zap then cleared those FRESH PTEs and freed the
        //   frames — the owner's next touch refaulted a ZERO page and
        //   get_meta walked zeroed metadata (the hp88 #GP family: victim
        //   PTEs perfectly healthy, rc=1/mapcount=0 non-ring frames with
        //   zeroed data). "Self-healing refill" only heals pages whose
        //   content lives elsewhere (page cache); anonymous pages with
        //   user stores are DESTROYED. The window was microseconds wide
        //   until the perf ring mmap began holding the GLOBAL PTE epoch
        //   exclusive across its 257-page install: the zap chunk and the
        //   fresh fault install queued together behind the same epoch
        //   writer and then raced ~50/50 on the mm spin — the ~40%
        //   build-flaky corruption rate of the hp88 noarm boots.
        //
        // Per-piece surgery+zap keeps BOTH faces closed at every guard
        // drop (a racing mmap may only ever land in a FULLY-CONSISTENT
        // hole: VMA gone ⇒ PTEs gone) while the vma_write hold stays
        // bounded at one 256KB piece's zap — the convoy bound that
        // motivated the chunking (the 100s-class brk convoy). unmap_pages
        // takes no VMA lock (PTE lock only, nested one-way, INV-LOCK-7
        // order vma → pte), so the hold is safe.
        //
        // Pin-table discipline (vm_file semantics): a split FILE VMA's
        // tail starts at a new address, so the pin keyed at the original
        // start no longer resolves for it — re-pin the tail at its own
        // start and drop the original key. Without this, demand faults on
        // the tail fell back to fd-number resolution and produced ZERO
        // pages once the mapping fd was closed (the reference's vm_file
        // follows the VMA, not the address).
        const MUNMAP_CHUNK_BYTES: usize = 256 * PAGE_SIZE_USIZE;

        // Whole-range fast path — see the block after the mp declarations
        // (mp/mp_t0 must exist before the fast path can record timings).

        let mp = crate::debug::mmprof::on();
        let mp_t0 = if mp { crate::debug::mmprof::now() } else { 0 };

        // Whole-range fast path (gb-opt lane, 2026-11-15): glibc allocator
        // churn (large malloc/free pairs, thread-stack teardown, arena
        // trim) munmaps ranges whose leaf PTEs are ALL zero — the mapping
        // was never faulted or was already zapped. The chunked loop below
        // pays its fixed per-chunk cost (vma_write cycle + VMA
        // remove/re-add + pin re-key + cond_resched) once per 1 MB chunk:
        // 64 lock/tree/pin cycles for a 64 MB arena teardown measured
        // 23.2 µs vs the reference kernel's 0.38 µs (mm-probe D, same-rig
        // A/B). When the range carries NO page-table state, run the whole
        // munmap as ONE guard cycle.
        //
        // Correctness: the scan is a COST DECISION, not a correctness gate.
        // The zap (unmap_pages) still runs inside the same vma_write guard
        // as the surgery, so the Face-B invariant (VMA gone ⇒ PTEs gone at
        // every guard drop) holds exactly as in the chunked path: a
        // CLONE_VM fault that sneaks in between the lock-free scan and the
        // zap is cleared by the zap like any other page; the scan cannot
        // miss it in a way the chunked path would have caught — the
        // chunked path zaps the same range with the same primitive. A
        // racing fork's COW downgrade only clears W on existing P=1
        // leaves; it cannot turn an empty range non-empty.
        {
            let mut vma_mgr = mm.vma_write();
            // Authoritative emptiness scan under the PTE section: with
            // pte_lock held, a same-mm fault cannot install (the install
            // maps under the pte lock), so an empty scan here means the
            // range stays empty through the surgery — the per-range zap
            // (unmap_pages) is skipped entirely. A fault that ALREADY
            // installed shows up in this scan (the chunked path below
            // then frees it page by page). A fault whose install is still
            // queued behind this pte section installs after the drop — a
            // PTE in a VMA-gone hole, the same post-drop shape the
            // chunked path's zap cannot prevent either (its install
            // re-check is mapped-ness, not VMA coverage); exit teardown
            // reaps it. Lock order vma → pte, INV-LOCK-7.
            let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock();
            if range_is_pte_empty(mm, addr_us, aligned_size, true) {
                let mut cur = addr_us;
                while cur < end_addr {
                    let hit = vma_mgr
                        .last_at_or_before(PageVirtAddr::new(cur))
                        .filter(|v| v.end().as_usize() > cur)
                        .or_else(|| {
                            vma_mgr.find_vma_after(PageVirtAddr::new(cur))
                        })
                        .filter(|v| v.start().as_usize() < end_addr);
                    let vma = match hit {
                        Some(v) => v,
                        None => break,
                    };
                    let vs = vma.start().as_usize();
                    let ve = vma.end().as_usize();
                    let seg_start = vs.max(cur);
                    let seg_end = ve.min(end_addr);
                    // Clone the snapshot: remove_vma_piece mutates the tree.
                    let v = *vma;
                    remove_vma_piece(mm, &mut *vma_mgr, &v, seg_start, seg_end);
                    // Advance past the WHOLE VMA: the tree search skips holes.
                    cur = ve;
                }
                drop(_pte_guard);
                drop(vma_mgr);
                crate::sched::cond_resched();

                crate::arch::ipi::flush_tlb_others_mm(
                    mm.tlb_cpu_mask(),
                    addr_us as u64,
                    end_addr as u64,
                );
                crate::arch::mm::asid::flush_tlb_all();
                if mp {
                    crate::debug::mmprof::add(crate::debug::mmprof::MU_BODY, mp_t0);
                }
                // (fork-frag) post-flush drain: single-CPU mms' pending
                // entries (targets empty) free right here.
                crate::arch::mm::mmu_init::pt_pending_drain();
                return Ok(());
            }
            drop(_pte_guard);
            drop(vma_mgr);
            // Range carries page-table state: fall through to the chunked
            // path, which frees the pages piece by piece.
        }

        let mp = crate::debug::mmprof::on();
        let mp_t0 = if mp { crate::debug::mmprof::now() } else { 0 };

        let mut cur = addr_us;
        while cur < end_addr {
            let mut vma_mgr = mm.vma_write();

            // First VMA overlapping [cur, end_addr): the one CONTAINING
            // cur, else the first one starting inside the remaining
            // range. Re-derived every iteration — but always FORWARD from
            // the cursor, never from addr_us (Face A).
            let hit = vma_mgr
                .last_at_or_before(PageVirtAddr::new(cur))
                .filter(|v| v.end().as_usize() > cur)
                .or_else(|| {
                    vma_mgr.find_vma_after(PageVirtAddr::new(cur))
                })
                .filter(|v| v.start().as_usize() < end_addr);
            let vma = match hit {
                Some(v) => v,
                None => break,
            };

            let vs = vma.start().as_usize();
            let ve = vma.end().as_usize();

            let seg_start = vs.max(cur);
            let seg_end = ve.min(end_addr);
            let chunk_end = seg_start + MUNMAP_CHUNK_BYTES.min(seg_end - seg_start);

            if mp {
                crate::debug::mmprof::bump(
                    crate::debug::mmprof::S_MUNLOOPS,
                    1,
                );
            }

            // Pages of this piece leave the mapping: charge release, pin
            // re-key, removal, survivor re-add — the shared piece surgery
            // (remove_vma_piece) does all of it.
            let v_snapshot = *vma;
            remove_vma_piece(mm, &mut *vma_mgr, &v_snapshot, seg_start, chunk_end);

            // Face-B closure: this piece's PTE zap runs INSIDE the same
            // guard as its surgery (unmap_pages nests vma → pte one-way),
            // so a guard drop never leaves live PTEs under a VMA-free
            // hole.
            mm.unmap_pages(
                PageVirtAddr::new(seg_start),
                chunk_end - seg_start,
                true,
                true,
            )?;

            // Drop the guard BEFORE yielding: the write lock must never
            // be held across schedule().
            drop(vma_mgr);
            crate::sched::cond_resched();
            cur = chunk_end;
        }

        // ONE TLB flush for the whole munmap (perf-gap-hunt; RESTORED
        // 2026-11-06 — 5b4a4bc7 dropped this block and the deferred
        // flushes were never shot down: stale present+writable entries
        // to FREED frames survived, and after the allocator reused those
        // frames same-mm accesses silently read/wrote the new owner's
        // pages — deterministic init #GP on the OH image, mallocng
        // get_meta consistency check, t=1.2). The chunked unmap_pages
        // above deferred its flushes — a multi-MB munmap runs ONE IPI
        // broadcast + CR3 reload instead of one per chunk (~1 µs/chunk
        // on the bench shape). Cleared-range visibility for user mode is
        // restored before the syscall returns (the reference batches its
        // unmapped-range flushes the same way).
        crate::arch::ipi::flush_tlb_others_mm(mm.tlb_cpu_mask(), addr_us as u64, end_addr as u64);
        crate::arch::mm::asid::flush_tlb_all();

        // (fork-frag) post-flush drain — see the fast-path twin.
        crate::arch::mm::mmu_init::pt_pending_drain();

        if mp {
            crate::debug::mmprof::add(crate::debug::mmprof::MU_BODY, mp_t0);
        }
        Ok(())
    }

    /// Zap the mapped pages in [start, start+size) WITHOUT touching the
    /// VMAs — madvise(MADV_DONTNEED) semantics. Reuses unmap_pages so the
    /// walk/rmap/refcount/clear_pte/TLB teardown happens under this mm's
    /// PTE section.
    pub fn zap_page_range(mm: &MmStruct, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if start.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }
        if start.as_usize().checked_add(aligned_size).is_none() {
            return Err(MapError::Invalid);
        }
        mm.unmap_pages(start, aligned_size, false, false)
    }

    /// Remove one VMA — or one [seg_start, piece_end) piece of it — from
    /// the tree: virtual-size charge release, pin un-key, removal, and
    /// survivor re-add (head before seg_start, tail after piece_end, each
    /// re-keyed in the pin table). Shared by the chunked munmap loop and
    /// the whole-range fast path. Caller holds vma_write.
    fn remove_vma_piece(
        mm: &MmStruct,
        vma_mgr: &mut crate::mm::vma::VmaManager,
        v: &Vma,
        seg_start: usize,
        piece_end: usize,
    ) {
        let vs = v.start().as_usize();
        let ve = v.end().as_usize();
        let vkey = v.start();

        mm.release_unmapped_pages(vs, ve, seg_start, piece_end);
        let pinned = mm.get_vma_file(vs);
        let _ = vma_mgr.remove(vkey);
        mm.unpin_vma_file(vs);
        if vs < seg_start {
            let mut head = Vma::new(
                v.start(),
                crate::mm::page::VirtAddr::new(seg_start),
                v.flags(),
            );
            head.set_type(v.vma_type());
            head.set_origin(v.origin());
            head.set_file_fd(v.file_fd());
            head.set_file_size(v.file_size());
            head.set_offset(v.offset());
            let _ = vma_mgr.add(head);
            if let Some(f) = pinned.clone() {
                mm.pin_vma_file(vs, f);
            }
        }
        if piece_end < ve {
            let mut tail = Vma::new(
                crate::mm::page::VirtAddr::new(piece_end),
                v.end(),
                v.flags(),
            );
            tail.set_type(v.vma_type());
            tail.set_origin(v.origin());
            tail.set_file_fd(v.file_fd());
            tail.set_file_size(v.file_size());
            tail.set_offset(v.offset() + (piece_end - vs));
            let _ = vma_mgr.add(tail);
            if let Some(f) = pinned.clone() {
                mm.pin_vma_file(piece_end, f);
            }
        }
    }

    /// True when every leaf PTE in [start, start+size) is raw-zero AND no
    /// upper-level leaf covers any block — i.e. the range carries no
    /// page-table state at all (never faulted, or already zapped).
    ///
    /// Read-only, lock-free (the caller's munmap fast path re-zaps under
    /// the surgery guard, so a racing CLONE_VM fault that installs a PTE
    /// after this scan is cleared by the zap — see the fast-path comment
    /// in munmap). Same stale-intermediate guards as unmap_pages, minus
    /// the mutation. The upper chain (PML4 entry, PUD entry) is resolved
    /// once per 1 GB PUD region — sequential ranges pay ~1 leaf read per
    /// 2 MB instead of three upper reads per block (gb-opt lane: the
    /// per-block walk made the scan ~5 µs of a 10.8 µs 64 MB munmap).
    ///
    /// (fork-frag lane) `unlink_emptied`: with it set (the munmap fast
    /// path — the arena-teardown shape), a linked leaf table whose FULL
    /// 512 entries are raw-zero is unlinked and defer-freed on the way
    /// past. This is where "already zapped" fragmentation lives: the
    /// range is PTE-empty but the tables stay linked otherwise, and
    /// every later fork pays one child-table alloc per empty table. The
    /// caller holds this mm's PTE section for the whole scan (see the
    /// fast-path comment), so the unlink cannot race a same-mm fault;
    /// cross-walker safety is pt_free_emptied_table's protocol.
    fn range_is_pte_empty(mm: &MmStruct, start: usize, size: usize, unlink_emptied: bool) -> bool {
        let mut addr = start;
        let end = addr.saturating_add(size);

        // Root guard (same shape as unmap_pages / PageTableWalker::walk).
        {
            let root_pa = mm.pgd << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published()
                && root_pa != 0
                && !crate::mm::phys_valid(root_pa as usize)
            {
                // Garbage root: treat as empty (unmap_pages bails Ok too).
                return true;
            }
        }
        let Some(root_table) = PtTable::from_phys(mm.pgd << PAGE_SHIFT) else {
            return true;
        };

        // Upper-chain state, re-resolved on 1 GB crossings.
        let mut table2: Option<PtTable> = None;
        let mut pud_end: usize = 0;
        let mut pud_empty: bool = true; // current PUD region has no PMD table

        while addr < end {
            if addr >= pud_end {
                // (Re)resolve PML4 → PUD for this 1 GB region.
                let pud_base = addr & !(PUD_SIZE as usize - 1);
                pud_end = pud_base + PUD_SIZE as usize;
                let a = VirtAddr::new(addr as u64);
                let pte4 = root_table.get(a.pgd_index() as usize);
                if !pte4.is_valid() {
                    pud_empty = true;
                    addr = pud_end.min(end);
                    continue;
                }
                if pte4.is_leaf() {
                    return false; // 1GB leaf covers the region
                }
                {
                    let pa = pte4.phys_addr();
                    if crate::mm::page_desc::descs_published()
                        && pa != 0
                        && !crate::mm::phys_valid(pa as usize)
                    {
                        pud_empty = true;
                        addr = pud_end.min(end);
                        continue;
                    }
                }
                let Some(table3) = PtTable::from_phys(pte4.phys_addr()) else {
                    pud_empty = true;
                    addr = pud_end.min(end);
                    continue;
                };
                let pte3 = table3.get(a.pud_index() as usize);
                if !pte3.is_valid() {
                    pud_empty = true;
                    addr = pud_end.min(end);
                    continue;
                }
                if pte3.is_leaf() {
                    return false; // 2MB-or-1GB leaf covers the region
                }
                {
                    let pa = pte3.phys_addr();
                    if crate::mm::page_desc::descs_published()
                        && pa != 0
                        && !crate::mm::phys_valid(pa as usize)
                    {
                        pud_empty = true;
                        addr = pud_end.min(end);
                        continue;
                    }
                }
                table2 = PtTable::from_phys(pte3.phys_addr());
                pud_empty = false;
            }

            let pmd_end = (addr & !(PMD_SIZE as usize - 1)) + PMD_SIZE as usize;
            let block_end = end.min(pmd_end);
            if pud_empty {
                addr = block_end;
                continue;
            }
            let a = VirtAddr::new(addr as u64);
            let Some(t2) = table2 else {
                addr = block_end;
                continue;
            };
            let pte2 = t2.get(a.pmd_index() as usize);
            if !pte2.is_valid() {
                addr = block_end;
                continue;
            }
            if pte2.is_leaf() {
                return false; // 2MB leaf covers the block
            }
            {
                let pa = pte2.phys_addr();
                if crate::mm::page_desc::descs_published()
                    && pa != 0
                    && !crate::mm::phys_valid(pa as usize)
                {
                    addr = block_end;
                    continue;
                }
            }
            let Some(table1) = PtTable::from_phys(pte2.phys_addr()) else {
                addr = block_end;
                continue;
            };
            // Leaf-table scan: ANY non-zero raw means state (resident
            // frame, swap entry, PROT_NONE marker, kernel window leaf) —
            // the conservative answer sends the munmap to the chunked
            // path, which frees/zaps each of those by its own discipline.
            let mut va = addr;
            while va < block_end {
                let idx = (((va as u64) >> PAGE_SHIFT) & 0x1ff) as usize;
                if table1.get_raw(idx) != 0 {
                    return false;
                }
                va += PAGE_SIZE_USIZE;
            }
            // In-range entries are zero. For the unlink decision the
            // WHOLE table must be zero (out-of-range PTEs of an
            // edge-straddling table may hold live mappings). When this
            // block covered the table ENTIRELY (the arena-teardown
            // shape) the scan above already proved it — no second pass.
            if unlink_emptied {
                let block_covers_table =
                    (addr & !(PMD_SIZE as usize - 1)) == addr && block_end == pmd_end;
                let all_zero = block_covers_table || table1.all_zero();
                if all_zero {
                    crate::arch::mm::mmu_init::pt_free_emptied_table(
                        t2,
                        a.pmd_index() as usize,
                        pte2.phys_addr(),
                        mm.tlb_cpu_mask(),
                    );
                }
            }
            addr = block_end;
        }
        true
    }

    /// Rewrite the leaf-PTE permission flags for [start, start+size) in the
    /// USER portion of this address space, preserving each PTE's PPN.
    ///
    /// Used by the init/ELF loaders to tighten a one-shot RWX pre-map down
    /// to per-segment (W^X) permissions. `flags` holds the PTE flag bits
    /// (P/U/RW/NX/...); pages not currently mapped are left alone. Callers
    /// must ensure `end <= USER_END` (kernel range PTEs are shared and must
    /// never be rewritten from here).
    pub fn set_range_permissions(mm: &MmStruct, start: u64, size: usize, flags: u64) {
        use crate::arch::mm::memory_layout::user_addr;
        let aligned_start = start & !(PAGE_SIZE_USIZE as u64 - 1);
        let end = match aligned_start.checked_add(size as u64) {
            Some(e) => (e + PAGE_SIZE_USIZE as u64 - 1) & !(PAGE_SIZE_USIZE as u64 - 1),
            None => return,
        };
        if aligned_start < user_addr::USER_START as u64 || end > user_addr::USER_END as u64 {
            return; // refuse kernel-range rewrites
        }

        let _pte_guard = pte_lock_for(mm).lock();
        let mut va = aligned_start;
        while va < end {
            let a = VirtAddr::new(va);
            let vpn4 = a.pgd_index() as usize;
            let vpn3 = a.pud_index() as usize;
            let vpn2 = a.pmd_index() as usize;
            let vpn1 = a.pte_index() as usize;

            let Some(root_table) = PtTable::from_phys(mm.pgd << PAGE_SHIFT) else {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            };
            let pte4 = root_table.get(vpn4);
            if !pte4.is_valid() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            };
            let pte3 = table3.get(vpn3);
            if !pte3.is_valid() || pte3.is_leaf() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            };
            let pte2 = table2.get(vpn2);
            if !pte2.is_valid() || pte2.is_leaf() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            };
            let old = table1.get(vpn1);
            if old.is_valid() {
                // Preserve the physical address ([51:12]), replace the
                // flag bits (low 12 + NX + software bits).
                let ppn_bits = old.bits() & PageTableEntry::PHYS_MASK_PUBLIC;
                table1.set(vpn1, PageTableEntry::from_bits(ppn_bits | flags));
            }
            va += PAGE_SIZE_USIZE as u64;
        }
        // SAFETY: full TLB flush so the tightened permissions take effect
        // immediately. Remote shootdown too: a peer CPU running a task
        // of THIS mm may hold the old permissive entries — with PCID
        // those survive context switches, and even PCID-less they only
        // died at the peer's next CR3 reload (latent same-mm stale
        // window, closed with the broadcast; mask-filtered to the CPUs
        // that ever loaded this mm, 2026-11-12).
        crate::arch::ipi::flush_tlb_others_mm(mm.tlb_cpu_mask(), 0, 0);
        crate::arch::mm::asid::flush_tlb_all();
    }

    /// brk system call implementation (legacy interface)
    pub fn do_brk(mm: &MmStruct, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        set_brk(mm, new_brk)
    }

    /// Allocate stack space
    pub fn allocate_stack(mm: &MmStruct, size: usize) -> Result<PageVirtAddr, MapError> {
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
        map_vma(mm, vma, Perm::ReadWrite)?;

        mm.setup_stack(stack_top.as_usize(), stack_size);

        Ok(stack_top)
    }

    /// Copy address space using Copy-on-Write mechanism
    pub fn fork(mm: &MmStruct) -> Result<MmStruct, MapError> {
        // R-WRITESTORM-3 (fork/VMA atomicity), phase-2 shape: SNAPSHOT
        // FIRST. The original protocol held ONE vma_read guard across the
        // cow_exempt scan, the COW page-table copy, AND the VMA snapshot so
        // the child could never pair OLD PTEs with NEW (mprotect-upgraded)
        // VMA flags — the SIGSEGV/EACCES "Permission denied" family. But a
        // vma_read guard is preempt-disabling, so that shape also pinned the
        // whole (multi-second, large-mm) COW walk to one CPU — the launcher
        // line's structural wall (session-G sc62/64 strand monsters).
        //
        // Snapshot-first keeps the consistency and drops the long hold:
        // exempt scan AND VMA snapshot come from the SAME read-guard era,
        // so the child pairs PTEs and VMAs of one coherent pre-fork
        // generation. The walk then runs with NO vma lock and only
        // per-leaf-table PTE-lock chunks (see copy_page_table_cow); a
        // concurrent mprotect/munmap/mmap on the parent interleaves at
        // per-PTE granularity and simply stays out of the child's
        // snapshot — the race-equivalent outcome POSIX already assigns to
        // mm writers concurrent with fork.
        // forkprof (cmdline forkprof=1): phase split of the mm fork —
        // snapshot / COW walk / VMA replay. One relaxed load when off.
        let fp = crate::debug::forkprof::on();
        let fp_t_snap = if fp { crate::debug::forkprof::now() } else { 0 };
        let (cow_exempt, vma_snapshot, vma_ranges): (Vec<(u64, u64)>, Vec<(PageVirtAddr, PageVirtAddr, VmaFlags, VmaType, i32, u64, usize, VmaOrigin)>, Vec<(u64, u64)>) = {
            let parent_vma_guard = mm.vma_read();

            // COW-exempt VMA regions (MAP_SHARED / device mappings). fork's COW
            // walk historically write-protected EVERY user-writable PTE — in
            // the PARENT too. A MAP_SHARED mapping then COW-faulted on the
            // first post-fork store: the fault handler allocated a private
            // copy and every subsequent store landed there, never in the
            // shared backing (the fbterm framebuffer-freeze root cause on
            // riscv64). Shared and device VMAs must be inherited as-is:
            // bump the refcount, keep the write permission.
            let exempt: Vec<(u64, u64)> = parent_vma_guard
                .iter()
                .filter(|vma| {
                    vma.flags().contains(crate::mm::vma::VmaFlags::SHARED)
                        || vma.vma_type() == crate::mm::vma::VmaType::Device
                })
                .map(|vma| (vma.start().0 as u64, vma.end().0 as u64))
                .collect();

            // Snapshot VMA data under the SAME parent read-guard (R-WRITESTORM-3
            // consistency era — see the comment above).
            let snapshot: Vec<(PageVirtAddr, PageVirtAddr, VmaFlags, VmaType, i32, u64, usize, VmaOrigin)> =
                parent_vma_guard.iter().map(|vma| {
                    (vma.start(), vma.end(), vma.flags(), vma.vma_type(), vma.file_fd(), vma.file_size(), vma.offset(), vma.origin())
                }).collect();

            // (sysmm-2x lane) Full VMA range list, same guard, same era:
            // bounds the COW walk's leaf-table scans (see
            // copy_page_table_cow). VMA iteration is sorted and disjoint.
            let ranges: Vec<(u64, u64)> = parent_vma_guard
                .iter()
                .map(|vma| (vma.start().0 as u64, vma.end().0 as u64))
                .collect();

            (exempt, snapshot, ranges)
        };
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_MM_SNAPSHOT, fp_t_snap);
            crate::debug::forkprof::bump(
                crate::debug::forkprof::S_FORKS_VMASES,
                vma_snapshot.len() as u64,
            );
        }

        // SAFETY: mm.pgd is a valid root PPN for the current address space. The caller
        // (fork) guarantees the parent address space is fully initialized and consistent.
        // The walk takes the PARENT's per-mm PTE section internally per leaf
        // table and may cond_resched between chunks (launcher wall fix — see
        // the function's own comment for the exclusion argument).
        let fp_t_cow = if fp { crate::debug::forkprof::now() } else { 0 };
        let new_root_ppn =
            copy_page_table_cow(mm, &cow_exempt, &vma_ranges).ok_or(MapError::OutOfMemory)?;
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_MM_COW, fp_t_cow);
        }

        // new_root_ppn was just returned from copy_page_table_cow, so it
        // points to a valid, freshly allocated root page table. space_type
        // and brk are valid by construction from the current address space.
        // (new_shared still carries the mm-tree's root contract — the
        // mm/ lane owns its conversion.)
        let new_space = MmStruct::new_shared(new_root_ppn, mm.space_type(), mm.brk());

        {
            // Everything after this point only touches the CHILD's manager
            // (the snapshot was taken before the walk — no parent lock held).
            let fp_t_replay = if fp { crate::debug::forkprof::now() } else { 0 };
            if !vma_snapshot.is_empty() {
                let mut new_vma_mgr = new_space.vma_write();
                for (start, end, flags, vma_type, file_fd, file_size, offset, origin) in vma_snapshot {
                    let mut new_vma = Vma::new(start, end, flags);
                    new_vma.set_type(vma_type);
                    // Fork copy: inherit the parent's origin stamp and add
                    // the FORKED modifier (adjudication: "mprotect+FORK"
                    // = an mprotect established the shape pre-fork).
                    let mut o = origin;
                    o.mark_forked();
                    new_vma.set_origin(o);
                    new_vma.set_file_fd(file_fd);
                    new_vma.set_file_size(file_size);
                    // File-offset continuity across fork (P1: also needed by
                    // the exec-image-backed segment VMAs' fd fallback path).
                    new_vma.set_offset(offset);
                    // Clone the parent's pinned vm_file so the child's
                    // demand faults read the file even after fd close.
                    if vma_type == crate::mm::vma::VmaType::FileBacked {
                        if let Some(f) = mm.get_vma_file(start.as_usize()) {
                            new_space.pin_vma_file(start.as_usize(), f);
                        }
                    }
                    // Increment nattch for shared memory attachments inherited by child
                    if vma_type == crate::mm::vma::VmaType::SharedMemory && file_fd >= 0 {
                        crate::ipc::ipc_hooks::shm_attach_vma(file_fd);
                    }
                    // Fork copy: the child's mapping carries the same
                    // virtual size as the parent's (the reference kernel
                    // copies mm->total_vm with the VMA set). Charge the
                    // copied VMA's own pages; the merge the manager may
                    // perform keeps the neighbour's charge intact.
                    let copied_pages = new_vma.page_count() as u64;
                    let _ = new_vma_mgr.add(new_vma);
                    new_space.add_total_vm(copied_pages);
                }
            }
            if fp {
                crate::debug::forkprof::add(crate::debug::forkprof::P_MM_REPLAY, fp_t_replay);
            }
        }

        new_space.set_start_code(mm.start_code());
        new_space.set_end_code(mm.end_code());
        new_space.set_start_data(mm.start_data());
        new_space.set_end_data(mm.end_data());
        new_space.set_start_stack(mm.start_stack());
        new_space.set_stack_limit(mm.stack_limit());
        new_space.set_arg_start(mm.arg_start());
        new_space.set_arg_end(mm.arg_end());
        new_space.set_env_start(mm.env_start());
        new_space.set_env_end(mm.env_end());
        // Linux dup_mm copies mm->saved_auxv: a forked child that later
        // crashes dumps the auxv of the exec that built its image.
        {
            let auxv = mm.saved_auxv();
            if !auxv.is_empty() {
                new_space.set_saved_auxv(&auxv);
            }
        }

        // RSS: the child's page tables map (COW-shared) every page the
        // parent had resident — inherit the count so OOM badness sees
        // forked hogs, not zero-rss newborns.
        new_space.add_rss(mm.rss());

        Ok(new_space)
    }

