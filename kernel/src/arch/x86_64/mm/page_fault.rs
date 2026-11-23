//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 page-fault outcome types + handle_mm_fault entry.
//!
//! Ported from arch/riscv64/mm/page_fault.rs: demand paging (anon
//! zero-fill, file-backed reads through the pinned vm_file, swap-in),
//! COW dispatch, stack growth, permission enforcement — all driven
//! through the generic VMA layer; only PTE flag construction and the
//! walk depth are x86-specific.

use super::memory_layout::*;
use super::unsafe_edges::PtTable;
use super::mm_ops::{alloc_user_phys_page, check_pte_permissions, is_cow_page, PageTableWalker};
use super::mmu_init::{get_page_table_virt, map_page, map_page_noflush, ROOT_PAGE_TABLE};
use super::pagetable::*;
use crate::mm::page::PAGE_SIZE as PAGE_SIZE_USIZE;
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Fault access flags (u32 bitset, matching generic callers)


// ==================== Fault Result ====================
// FaultFlags + MmFaultResult moved to the SEAM (mm/mm_hooks.rs) with the
// M1a carve: the trap layer, uaccess, ptrace/futex/gup and the hook table
// all name them; the routing logic itself now lives in the rux-mm crate
// (kernel/src/mm/policy/x86_64.rs). Re-export keeps every historical
// path (arch::mm::X, page_fault::X) resolving.
pub use crate::mm::mm_hooks::{FaultFlags, MmFaultResult};

/// Outcome of a handled fault


// ==================== Stack Expansion ====================


/// Fill one freshly allocated (exclusively owned) user page from `file`
/// at `file_offset` — the single-page demand-fault fill semantics,
/// extracted so the fault-around batch shares it byte-for-byte.
///
/// Pre-zero only when a full-page read is not guaranteed; any read that
/// does not verifiably cover the whole page (short tail, 0, or a NEGATIVE
/// errno return from read_at — the old inline `bytes_read == 0` check
/// missed negative returns and could leak recycled-buddy content) leaves
/// the uncovered tail zeroed.
///
/// # SAFETY
/// `page_ptr` must be a PAGE_SIZE-byte kernel VA of an exclusively owned
/// fresh physical page (phys_to_virt of an alloc_user_phys_page result).
/// `page_ptr` is a freshly allocated, exclusively owned page
/// (INV-XE-ZP discipline; the caller holds it before mapping).
pub fn fill_file_page(
    file: &crate::fs::file::File,
    file_offset: usize,
    page_ptr: *mut u8,
    effective_size: usize,
) {
    let will_full_read =
        file_offset < effective_size && effective_size - file_offset >= PAGE_SIZE_USIZE;
    if !will_full_read {
        // SAFETY: fresh exclusive page frame (INV-XE-ZP).
        unsafe { core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE) };
    }
    if file_offset >= effective_size {
        return; // within VMA but past EOF: stays zero (sparse/hole)
    }
    let bytes_to_read = core::cmp::min(PAGE_SIZE_USIZE, effective_size - file_offset);
    // SAFETY: read_at fills the fresh exclusive frame (INV-XE-ZP); a
    // short/failed read is re-zeroed below.
    let bytes_read = unsafe { file.read_at(file_offset as u64, page_ptr, bytes_to_read) };
    let good = if bytes_read > 0 {
        bytes_read as usize
    } else {
        0
    };
    if good < PAGE_SIZE_USIZE {
        // SAFETY: same frame; tail zero of the uncovered bytes.
        unsafe { core::ptr::write_bytes(page_ptr.add(good), 0, PAGE_SIZE_USIZE - good) };
    }
}

// ==================== filemap_fault (page-cache mapping) ====================

/// Pin the page-cache frame for one page-shaped file offset, populating
/// the cache on a miss through the ext4 read machinery (the fs-side
/// fault-around detector warms the surrounding window in the same device
/// round trips a read_at would have used).
///
/// Returns the PINNED physical address for direct mapping. The pin (plus
/// the descriptor `get_page` the map path takes) is owned by the PTE for
/// its mapping lifetime — see `cow_flags::PCACHE`.
pub fn filemap_pin_cache_page(
    file: &crate::fs::file::File,
    file_offset: usize,
    effective_size: usize,
    hole_as_zero: bool,
) -> Option<u64> {
    if file_offset >= effective_size {
        return None;
    }
    // SAFETY: inode cell written at open time, read-only afterwards.
    let inode = unsafe { &*file.inode.get() }.as_ref()?;
    let (fs_id, ino) = (inode.fs_id, inode.ino as u64);
    let page_index = (file_offset / PAGE_SIZE_USIZE) as u64;
    let cache = crate::fs::page_cache::get_page_cache();
    if let Some(phys) = cache.get_phys(fs_id, ino, page_index) {
        return Some(phys);
    }
    // Miss: warm the cache (ReadDst::Discard — no bounce copy), then pin.
    // Holes never enter the page cache through the READ path (the fill
    // skips unallocated blocks and zero-fills the destination in place),
    // so populate can "succeed" without the page existing. For the
    // hole_as_zero arm (SHARED mappings, session 25) that must not fall
    // back to a private zero copy — it would break mapper coherence (each
    // mapper gets its own zeros) and nothing would persist. Insert an
    // explicit ZERO page into the cache and alias THAT: one shared frame
    // for every mapper, and the flush path allocates the on-disk block at
    // msync/fsync time. Private arms keep the private-copy fill.
    // The demand-populate capability dispatch (rung-5 fault seam:
    // FileOps.fault). Absent slot behaves as the old internal
    // non-ext4 guard (populate = false).
    let populated = file
        .get_ops()
        .and_then(|ops| ops.fault)
        .is_some_and(|f| f(file, file_offset as u64));
    if populated {
        if let Some(phys) = cache.get_phys(fs_id, ino, page_index) {
            return Some(phys);
        }
    }
    if !hole_as_zero {
        return None;
    }
    // SAFETY: same read-only inode resolution as above.
    let inode = unsafe { &*file.inode.get() }.as_ref()?;
    static ZERO_PAGE: [u8; 4096] = [0; 4096];
    cache.insert(inode.fs_id, inode.ino as u64, page_index, 0, &ZERO_PAGE);
    cache.get_phys(fs_id, ino, page_index)
}

/// Bookkeeping for a successfully mapped page-cache frame: one descriptor
/// reference (dropped by unmap/COW's put_page) and mapcount visibility.
/// Deliberately NO set_index / page_record_mapping / LRU add — the
/// descriptor's mapping/index are the page cache's eviction keys and the
/// file-LRU membership belongs to the cache (the fake-OOM-era clobber of
/// either would corrupt eviction: wrong-key lookup or an unlisted frame).
///
/// # SAFETY
/// `phys` is a page-cache frame pinned for THIS mapping (pin taken by
/// `filemap_pin_cache_page`), being mapped right now under the PTE lock.
pub fn filemap_bookkeep_mapped_cache_page(phys: u64) {
    use crate::mm::page_desc::page_desc;
    // Descriptor of a live, pinned cache frame.
    if let Some(page) = page_desc((phys >> PAGE_SHIFT) as usize) {
        page.get_page();
        page.inc_mapcount();
    }
}

/// Release a fill-phase page that lost the map race (or whose map failed):
/// a pinned cache frame returns its pin; a private fill copy is freed.
pub fn release_fill_page(phys: u64, from_cache: bool) {
    if from_cache {
        super::mm_ops::release_cache_mapping_pin((phys >> PAGE_SHIFT) as usize);
    } else {
        crate::mm::page_alloc::free_page(phys as usize);
    }
}

// ==================== Main Fault Handler ====================


/// handle_mm_fault - Handle user mode page fault
///
/// # Arguments
/// - `addr_space`: Address space
/// - `fault_addr`: Virtual address that triggered fault
/// - `flags`: Fault type flags (FaultFlags)
/// L7 defect-A instrument: WHICH early-refusal arm returned Segfault
/// (set at each site; printed once per boot by the trap Segfault arm).
/// 1=stack_limit 2=no_stack_vma 3=not_below_start 4=below_stack_limit
/// 5=expand_failed 6=perm(is_read&&!readable via Segfault) 9=entry_perm
pub static FILL_REFUSAL_SITE: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);


// ==================== Utility ====================

/// Physical address behind a user virtual address, if mapped.
///
/// Walks the page table to find the PTE for the given virtual address
/// and returns the physical page address (page-aligned).
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    // SAFETY: caller guarantees root_ppn is a live user root.
     PageTableWalker::walk(root_ppn, vaddr).map(|(ppn, _)| ppn << 12) 
}

// ==================== Swap-In Support ====================


/// R-WRITESTORM-2: write one 4K leaf PTE (raw bits), following the same
/// 4-level walk as read_pte_raw. Caller must hold PTE_MODIFY_LOCK and
/// only call it for addresses whose walk read_pte_raw already resolved
/// (the upper levels exist).
pub fn write_pte_raw(root_ppn: u64, vaddr: VirtAddr, bits: u64) {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;
    // Same walk as read_pte_raw (typed); the leaf exists (caller read it
    // via read_pte_raw on the same root/addr), so the intermediate
    // tables are present. Only the leaf is replaced.
    {
        let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else { return };
        let pte4 = root_table.get(vpn4);
        if !pte4.is_valid() {
            return;
        }
        let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else { return };
        let pte3 = table3.get(vpn3);
        if !pte3.is_valid() || pte3.is_leaf() {
            return;
        }
        let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else { return };
        let pte2 = table2.get(vpn2);
        if !pte2.is_valid() || pte2.is_leaf() {
            return;
        }
        let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else { return };
        // 0xb9 install validator (oh-launch round 7): a PRESENT entry
        // with zero address bits is corrupt by construction (physical
        // page 0 is reserved at boot on x86). The hilogd teardown
        // fault walked one; this arm names the INSTALLER the moment a
        // bad value is written, live. Gated by rux:mmforensics;
        // deferred printk (lock-safe); one load when off.
        if crate::trace::EVENT_MMFORENSICS.armed()
            && (bits & 0x1) != 0
            && (bits & 0x000F_FFFF_FFFF_F000) == 0
        {
            static BADPT_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if BADPT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16 {
                crate::pr_err!(
                    "PT-BAD-INSTALL: write_pte_raw bits={:#x} vaddr={:#x} root={:#x} pid={} ra={:#x}",
                    bits,
                    a.bits(),
                    root_ppn,
                    crate::sched::get_current_pid(),
                    crate::debug::backtrace::caller_return_address()
                );
            }
        }
        table1.set(vpn1, super::pagetable::PageTableEntry::from_bits(bits));
    }
}

/// Slot-batch helper (fault-around map phase, perf-gap-hunt): walk the
/// upper three levels ONLY and return the level-1 (PT) table pointer
/// covering `vaddr`'s 2 MB region. Returns None when an intermediate
/// level is missing (caller falls back to `map_page_noflush`, which
/// allocates) or is a huge leaf needing demotion (same fallback). The
/// leaf slot itself is the CALLER's to read/write — this is the one-walk
/// primitive that keeps a 512-page batch at one leaf load + one leaf
/// store per page.
///
/// # SAFETY
/// `root_ppn` is a valid root page-table PPN for the current address
/// space. Read-only walk of paging structures.
/// Typed leaf-table handle for the fault paths (the stale-frame guard is
/// baked into construction).
pub fn l1_table_for(root_ppn: u64, vaddr: VirtAddr) -> Option<PtTable> {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
    let pte4 = root_table.get(vpn4);
    if !pte4.is_valid() {
        return None;
    }
    let table3 = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT)?;
    let pte3 = table3.get(vpn3);
    if !pte3.is_valid() || pte3.is_leaf() {
        return None;
    }
    let table2 = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT)?;
    let pte2 = table2.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return None;
    }
    PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)
}

pub fn read_pte_raw(root_ppn: u64, vaddr: VirtAddr) -> Option<u64> {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    // Typed walk; only reads PTEs, including the leaf even if P=0.
    {
        let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
        let pte4 = root_table.get(vpn4);
        if !pte4.is_valid() {
            return None;
        }

        let table3 = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT)?;
        let pte3 = table3.get(vpn3);
        if !pte3.is_valid() {
            return None;
        }
        // Superpage leaves (identity/device windows): return the leaf
        // itself — never a swap/migration entry, and its PPN must not be
        // followed as a table pointer.
        if pte3.is_leaf() {
            return Some(pte3.bits());
        }

        let table2 = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT)?;
        let pte2 = table2.get(vpn2);
        if !pte2.is_valid() {
            return None;
        }
        if pte2.is_leaf() {
            return Some(pte2.bits());
        }

        let table1 = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)?;
        let pte1 = table1.get(vpn1);

        let raw = pte1.bits();
        if raw == 0 {
            return None;
        }

        Some(raw)
    }
}


// Keep the static root reachable for diagnostics (interface parity with
// the riscv64 twin's imports).
#[allow(unused)]
fn _root_table_ref() -> usize {
    // SAFETY: address-of only, no dereference.
    unsafe { &raw mut ROOT_PAGE_TABLE as usize }
}
