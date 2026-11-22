//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! riscv64 mm POLICY — the carved half of arch/riscv64/mm/{mm_ops,page_fault}.rs
//! (M1a, the x86_64 twin). Compiled ONLY by the rux-mm service crate under
//! crate-level #![forbid(unsafe_code)]; the mechanism twins (walks, PTE
//! atomics, the fill family, unmap machinery) stay in the arch files.
//! Moved verbatim except: self->mm free-fn shape, the asm TLB fences onto
//! the published asid::sfence_vma_addr/flush_tlb_all helpers, raw
//! zero/inode-deref/read_at onto the audited edges, vestigial unsafe
//! wrappers dropped around already-safe calls, and set_brk's raw
//! pfn_to_page_mut block onto the typed page_desc edge.

use crate::arch::mm::memory_layout::*;
use crate::arch::mm::unsafe_edges::PtTable;
use crate::arch::mm::mmu_init::{map_page, map_page_noflush, get_page_table_virt, ROOT_PAGE_TABLE};
use crate::arch::mm::mm_ops::{
    alloc_user_phys_page, check_pte_permissions, copy_page_table_cow, is_cow_page,
    pte_lock_for, PageTableWalker, perm_to_flags, handle_cow_fault,
};
use crate::arch::mm::page_fault::{
    filemap_pin_cache_page, read_pte_raw, write_pte_raw,
};
use crate::arch::mm::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;
use crate::mm::mm_hooks::{FaultFlags, MmFaultResult};
use crate::mm::mm_struct::MmStruct;
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::vma::{Vma, VmaFlags, VmaOrigin};
use crate::mm::{alloc_pages, GfpFlags};
use alloc::vec::Vec;

/// sc181: forward fault-around window (pages) for anonymous write faults.
const ANON_FAULTAROUND_PAGES: usize = 32;

/// sc181: minimum VMA size that gets fault-around (small stacks/brk keep
/// exact one-page semantics).
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
    use crate::mm::vma::VmaFlags;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    let stack_limit = mm.stack_limit();
    let fault_addr_val = fault_addr.as_usize();

    // Check if fault address is within stack expansion range
    if fault_addr_val < stack_limit {
        return MmFaultResult::Segfault;
    }

    // Try to find the stack VMA (with GROWSDOWN flag)
    let vma_mgr_read = mm.vma_read();
    let (vma_start, stack_vma) = match vma_mgr_read.find_stack_vma(page_virt_addr) {
        Some((start, vma)) => (start, vma),
        None => {
            return MmFaultResult::Segfault;
        }
    };

    // Calculate new start address (page-aligned)
    let new_start = PageVirtAddr::new(fault_addr_val & !(MM_PAGE_SIZE - 1));

    // New start must be below current VMA start
    if new_start.as_usize() >= vma_start.as_usize() {
        return MmFaultResult::Segfault;
    }

    // Check if expansion would exceed stack limit
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
        if vma_mgr_write.expand_downwards(vma_start, new_start).is_err() {
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

    // Initialize page content based on type
    // SAFETY: page_ptr is derived from phys_to_virt() on a freshly allocated page
    // (alloc_user_phys_page). The page is PAGE_SIZE bytes and exclusively owned.
    match vma_type {
        VmaType::Anonymous => {
            crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
        }
        VmaType::FileBacked | VmaType::SharedMemory => {
            crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
        }
        VmaType::Device => {
            // Device mapping: don't zero
        }
    }

    // Build page table entry flags
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U; // User page

    // Sv39 forbids W-without-R leaf PTEs: a write-only VMA (PROT_WRITE
    // alone) folds to RW here while the VMA itself stays W-only so
    // /proc/self/maps shows the true "-w" permissions (LTP mmap04).
    if vma_flags.is_readable() || vma_flags.is_writable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }

    // Map page under the PTE-modify lock and keep it held across the rmap
    // setup: a concurrent fork's copy_page_table_cow walk landing between
    // the map and the rmap/refcount update takes a reference the rmap
    // never sees (round 6 MED — demand-fault PTE lock).
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock_irqsave();
    // R7-C3: re-check under the lock. The already_mapped walk at entry was
    // lock-free; two threads sharing this mm (CLONE_VM) can both see "not
    // mapped", both allocate+zero (+read for file VMAs), and the second
    // map_page would orphan the first page (refcount 1, mapcount 0,
    // unreclaimable) and silently discard stores landed in it.
    // E8-MM: same device-window-hole rule as the entry walk (see
    // pte_counts_as_mapped) — a restored kernel PTE must not be mistaken
    // for the winner's mapping here, or this path "Handles" forever
    // against it while the user instruction keeps refaulting.
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
    // Map page (root_ppn valid, fault_addr page-aligned, phys_addr a
    // fresh page, pte_flags well-formed — the walk primitive's contract).
    {
        if !map_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure (fake-OOM family): free the data
            // page, deliver OOM — the trap layer SIGKILLs the user task
            // (Linux pagefault-OOM semantics; the old path panicked the
            // kernel here).
            crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
            return MmFaultResult::OutOfMemory;
        }

        // Address-specific TLB flush
        let vaddr = fault_addr.bits();
        // SAFETY: ASID-scoped TLB fence (memory-safe in any context).
        crate::arch::mm::asid::sfence_vma_addr(vaddr);
    }

    // Set up reverse mapping for stack page (VMA lock already dropped,
    // so set fields directly instead of calling page_add_anon_rmap)
    {
        use crate::mm::page_desc::{page_desc, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        // This page was just allocated and is not yet shared (INV-FRAME-1).
        if let Some(page) = page_desc(page_pfn) {
            {
                page.set_flag(PageFlag::Anonymous);
                // SwapBacked + LRU_INACTIVE_ANON membership make the page
                // visible to the reclaim engine — vmscan only swaps out
                // pages that sit on LRU_INACTIVE_ANON with SwapBacked set.
                // Without this, anonymous demand faults accumulate as
                // unreclaimable memory and the system OOMs with swap idle.
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
    }
    drop(_pte_guard);

    // RSS accounting (review 4.14): stack page became resident.
    mm.add_rss(1);

    MmFaultResult::Handled
}

/// handle_mm_fault - Handle user mode page fault
///
/// # Arguments
/// - `mm`: Address space
/// - `fault_addr`: Virtual address that triggered fault
/// - `flags`: Fault type flags (FaultFlags)
///
/// # Returns
/// Returns handling result
///
/// # Function
/// 1. Find VMA to validate address validity and permissions
/// 2. Check if page is already mapped
/// 3. If COW page, return CowPending
/// 4. If unmapped, allocate new page (zero anonymous pages)
/// 5. Update page table, set correct permission bits
/// E8-MM: does a leaf PTE (raw bits) at `va` count as "mapped" for a
/// fault on this address space? A valid KERNEL (U=0) PTE inside a
/// low-half device window is the RESTORE fill clear_pte leaves behind
/// after a user mapping over the window was unmapped (megapage
/// demotion made those mmaps possible — MAP_FIXED replace semantics).
/// A user fault there is a demand fault on the user mapping, not a
/// protection violation: treat the device PTE as a hole and let the
/// VMA layer decide (a stray access with no covering VMA still
/// SIGSEGVs, just via the correct path). Caught by megapage_mmap's
/// re-map step: the old code returned Permission denied at 0x0c008007
/// (sig=11), and without this same rule in the R7-C3 re-check the
/// demand path spun forever "Handling" refaults against the restored
/// kernel PTE.
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
    // R-WRITESTORM-2 (x86_64 twin): a V|U leaf with NO R/W/X bits is a
    // PROT_NONE marker from mprotect(prot==0) (Sv39 keeps V set). Counting
    // it as "already mapped" sent the fault into the permission check,
    // which sees no R/W/X and kills the task without consulting the
    // covering VMA. Markers now fall through to the VMA layer, which
    // re-presents the leaf when the VMA grants the access.
    let already_mapped = 
        PageTableWalker::walk(root_ppn, fault_addr.bits() as u64).map_or(false, |(_ppn, bits)| {
            !(pte_counts_as_mapped(bits, fault_addr.bits() as u64)
              && (bits & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X)) == 0
              && (bits & PageTableEntry::V) != 0)
        })
    ;

    // If not mapped, check for a swap entry in the PTE (V=0 but non-zero bits)
    if !already_mapped {
        if let Some(entry) = read_pte_raw(root_ppn, fault_addr) {
            // Migration entry (review 4.16): compaction is relocating the
            // page that lived here. Wait for the migration window to close
            // instead of installing a zero page that the remap would then
            // overwrite (silent user-write loss). Bounded spin: the window
            // is a 4KB memcpy plus PTE rewrite on another CPU.
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
                // Stale marker (migration ended without remap — e.g. the
                // VMA was concurrently unmapped and re-faulted): fall
                // through to normal demand handling below.
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
            crate::pr_debug!("pagefault: cow at {:#x}", fault_addr.bits());
            return MmFaultResult::CowPending;
        }

        // Check if page permissions meet access requirements
        // SAFETY: root_ppn is a valid page table root. check_pte_permissions only reads PTEs.
        if let Some((has_read, has_write, has_exec, pte_is_user)) =
             check_pte_permissions(root_ppn, fault_addr)  {
            // Verify permissions
            let perm_ok = (!is_write || has_write)
                && (!is_read || has_read)
                && (!is_exec || has_exec)
                && (!is_user || pte_is_user);

            if perm_ok {
                // Permissions correct, flush TLB
                // SAFETY: sfence.vma with a specific virtual address invalidates TLB entries
                // for that address. The fence instructions ensure memory ordering.
                crate::arch::mm::asid::sfence_vma_addr(fault_addr.bits());
                return MmFaultResult::Handled;
            }
        }

        return MmFaultResult::PermissionDenied;
    }

    // Find VMA
    let vma_mgr = mm.vma_read();
    let vma = match vma_mgr.find(page_virt_addr) {
        Some(v) => v,
        None => {
            drop(vma_mgr);
            return try_expand_stack(mm, fault_addr, flags, root_ppn);
        }
    };

    // Get VMA attributes
    let vma_flags = vma.flags();
    let vma_type = vma.vma_type();
    let vma_file_fd = vma.file_fd();
    let vma_file_size = vma.file_size();
    let vma_offset = vma.offset();
    // sc181: bounds for the anonymous fault-around window (see the tail
    // and anon_faultaround_forward).
    let vma_start = vma.start().as_usize();
    let vma_end = vma.end().as_usize();

    // R-SC-RV64 (x86_64 twin parity): capture everything the file paths
    // need ONCE here, under the guard we already hold. The old shape
    // re-derived (vma_start, file, effective_size) in FOUR separately
    // locked lookups (SIGBUS gate, shared-alias resolution, private pin,
    // fill arm), each a sched::current() round trip + a fresh vma_read()
    // .find() + pin-table lookup — the shared-alias site even took TWO
    // vma_read() acquisitions back to back (file from one, vma_start from
    // the other). Between any two of those lookups a CLONE_VM sibling
    // could munmap/mremap/split the VMA, yielding a mismatched
    // (file, vma_offset, vma_start) triple: a wrong-offset fill or, in
    // the worst case, a cache pin taken under ANOTHER file's
    // (fs_id, ino, index) key. It is also pure overhead — 3 finds and up
    // to 4 pin-table lookups per file fault (the twin's own rationale:
    // x25600 faults for a 100 MB lib load). The guard is dropped before
    // any read_at below (the R73 preemption/EXT4_BIG_LOCK pinch rule is
    // about holding it across I/O, not about cloning an Arc under it).
    let vm_file = if vma_type == VmaType::FileBacked && vma_file_fd >= 0 {
        mm.get_vma_file(vma_start)
    } else {
        None
    };
    let file_offset = vma_offset + (page_virt_addr.as_usize() - vma_start);
    // Effective size: CURRENT inode size (a file grown or truncated since
    // mmap must serve/shrink accordingly); devfs regular files additionally
    // consult the live entry (b74 ftruncate snapshot — the x86_64 twin has
    // consulted it since its session-14 snapshot). The mmap-time snapshot
    // stays as the no-file fallback. One computation serves the SIGBUS
    // gate, the cache-population size gate and the private-fill arm.
    let effective_size = vm_file
        .as_ref()
        .and_then(|f| {
            if let Some(live) = crate::fs::devfs::devfs_reg_current_size(f.as_ref()) {
                return Some(live as usize);
            }
            // SAFETY: inode cell written at open time; read-only.
            let inode_opt = crate::fs::unsafe_edges::file_inode(f);
            inode_opt.map(|i| i.get_size() as usize)
        })
        .unwrap_or(vma_file_size as usize);

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

    // R-WRITESTORM-2 (x86_64 twin): if the leaf is a PROT_NONE marker
    // (V|U, no R/W/X, PPN preserved) and the covering VMA grants this
    // access, RE-PRESENT the leaf with the VMA's permissions — preserving
    // the PPN and the COW software bit (W deferred to the COW fault,
    // INV-COW-1). Linux parity: a fault under a VMA that grants the access
    // must succeed regardless of the region's mprotect history; the stale
    // marker is advisory PTE state, not a protection decision.
    {
        let page_base = VirtAddr::new(fault_addr.bits() & !0xFFF);
        if let Some(raw) = read_pte_raw(root_ppn, page_base) {
            if is_protnone_marker(raw) {
                let mut flags = PageTableEntry::V | PageTableEntry::U
                    | PageTableEntry::A | PageTableEntry::D;
                if vma_flags.is_readable() {
                    flags |= PageTableEntry::R;
                }
                if vma_flags.is_writable() {
                    flags |= PageTableEntry::W | PageTableEntry::R; // W requires R
                }
                if vma_flags.is_executable() {
                    flags |= PageTableEntry::X;
                }
                let cow_bit = raw & (1 << 8); // COW software bit
                if cow_bit != 0 {
                    flags |= cow_bit;
                    flags &= !PageTableEntry::W;
                }
                // PCACHE (RSW bit 9, filemap_fault port): the marker hides
                // a pinned page-cache frame — the pin ownership must
                // survive the re-present (else the later unmap leaks it).
                // A PRIVATE cache leaf never gains W in place: a W-upgrade
                // diverts the first write through COW like every other
                // private file page (Linux writenotify; x86_64 twin in
                // mprotect_pte_range). A SHARED cache leaf (PCACHE with no
                // COW — see cow_flags::pte_is_shared_cache; Sv39 has no
                // third RSW bit, bit 10 is the PPN field) keeps W — its
                // stores must land in the cache frame (session 25).
                let pcache_bit = raw & crate::arch::mm::mm_ops::cow_flags::PCACHE;
                if pcache_bit != 0 {
                    flags |= pcache_bit;
                    if flags & PageTableEntry::W != 0
                        && !crate::arch::mm::mm_ops::cow_flags::pte_is_shared_cache(raw)
                    {
                        flags |= 1 << 8; // force COW
                        flags &= !PageTableEntry::W;
                    }
                }
                let _pte_guard =
                    crate::arch::mm::mm_ops::pte_lock_for(mm).lock_irqsave();
                if let Some(now) = read_pte_raw(root_ppn, page_base) {
                    if is_protnone_marker(now) {
                        let new_pte = (now & !0x3FF) | flags;
                        write_pte_raw(root_ppn, page_base, new_pte);
                        // SAFETY: sfence.vma for this address; kernel ctx.
                        crate::arch::mm::asid::sfence_vma_addr(page_base.bits());
                        return MmFaultResult::Handled;
                    }
                }
            }
        }
    }

    crate::pr_debug!("pagefault: map new page at {:#x}, type={:?}", fault_addr.bits(), vma_type);

    // Release read lock
    drop(vma_mgr);

    // File-backed fault BEYOND the file's end is SIGBUS, not a zero page
    // (Linux filemap_fault: no page at/after EOF can be served for a
    // mapping — LTP mmap13 truncates a mapped file and expects the next
    // touch of the cut page to raise SIGBUS). Decided on the single
    // entry-time snapshot (R-SC-RV64 above) — the effective size is the
    // CURRENT inode size (a truncate after mmap shrinks it), falling
    // back to the snapshot taken at mmap time.
    if vma_type == VmaType::FileBacked && vma_file_fd >= 0 {
        if file_offset >= effective_size {
            return MmFaultResult::BusError;
        }
    }

    // MAP_SHARED file mapping: alias the PAGE-CACHE frame for this file
    // offset instead of a private copy, so every mapper of the file —
    // across fork and across a fresh mmap after exec — observes the
    // others' stores. Linux filemap semantics: VM_SHARED faults map the
    // page-cache page directly. Private mappings keep the copy path
    // below. (LTP execl01..execveat02: the child re-mmaps the IPC
    // results file after exec; with private copies the parent never saw
    // the child's TPASS bits and TBROK'd "Test haven't reported
    // results".)
    //
    // Session 25 (writeback coherence): ext4 files migrate onto the
    // PCACHE protocol — the SAME pin the private arm takes (get_phys /
    // Discard-warm via filemap_pin_cache_page), released at unmap by the
    // PTE's PCACHE bit. This retires the old PERMANENT pin (alias frames
    // used to stay unevictable forever, bounding memory by every file
    // ever shared-mapped instead of by live mappings), fixes write(2)
    // divergence (the write path now updates pinned frames in place
    // instead of invalidating the inode cache — a stale invalidated
    // entry served the next fault from DISK, splitting mappers from the
    // still-live alias frame), and gives the mapping W-through + dirty
    // + msync/fsync persistence (stores land in the frame; the
    // conservative dirty-at-install mark makes sync flush it). Non-ext4
    // files keep the legacy permanent-pin alias.
    let (shared_alias_phys, shared_pcache): (Option<u64>, bool) =
        if vma_type == VmaType::FileBacked && vma_flags.is_shared() {
            // R-SC-RV64: (vm_file, file_offset) come from the single
            // entry-time snapshot — the old shape resolved the file from
            // one vma_read() lookup and vma_start from a second, so a
            // concurrent VMA mutation could pair one file with another
            // VMA's start and pin a foreign file's cache page.
            let file = vm_file.clone().or_else(|| {
                if vma_file_fd >= 0 {
                    crate::fs::get_file_fd(vma_file_fd as usize)
                } else {
                    None
                }
            });
            match file {
                Some(f) => {
                    if crate::fs::ext4::file::is_ext4_file(f.as_ref()) {
                        match filemap_pin_cache_page(f.as_ref(), file_offset, effective_size, true)
                        {
                            Some(phys) => {
                                // Conservative v1 dirty mark for a W
                                // mapping (x86_64 twin): the store may
                                // never come; flushing an unwritten page
                                // is harmless I/O.
                                if vma_flags.is_writable() {
                                    if let Some(inode) =
                                        crate::fs::unsafe_edges::file_inode(&f)
                                    {
                                        crate::fs::page_cache::get_page_cache().mark_dirty(
                                            inode.fs_id,
                                            inode.ino as u64,
                                            (file_offset / PAGE_SIZE_USIZE) as u64,
                                        );
                                    }
                                }
                                (Some(phys), true)
                            }
                            // Hole/populate failure: fall to the private
                            // copy below (pre-existing behavior).
                            None => (None, false),
                        }
                    } else {
                        // Legacy non-ext4 alias: permanent pin, unchanged.
                        match crate::fs::page_cache::shared_file_fault_page(
                            &f,
                            file_offset as u64,
                        ) {
                            Some(p) => (Some(p), false),
                            None => (None, false),
                        }
                    }
                }
                None => (None, false),
            }
        } else {
            (None, false)
        };

    // MAP_PRIVATE file read/exec fault (filemap_fault port of the x86_64
    // session-23 fix): pin the PAGE-CACHE frame and map it directly,
    // read-only — one resident copy per file page shared by every mapper
    // and refault, reclaimable the moment the last mapping drops its pin.
    // The old path allocated an EXCLUSIVE private copy per fault and left
    // it with no page-descriptor state — the fake-OOM drainer (session-22
    // attribution; the GNOME riscv64 image's OOM-at-342MB/2G is the same
    // family). Writable private VMAs get the COW bit instead of W (Linux
    // writenotify/do_read_fault): the first write COWs a private copy in
    // handle_cow_fault, which also drops this mapping's pin. Write faults
    // and non-ext4 files/holes keep the private-copy fill below (now with
    // proper page bookkeeping).
    let private_cache_phys: Option<u64> = if vma_type == VmaType::FileBacked
        && !vma_flags.is_shared()
        && !is_write
    {
        // R-SC-RV64: the pinned file, offset and size gate all come from
        // the entry-time snapshot (the old shape re-found the VMA and the
        // pin-table entry here — a second, independently racy lookup).
        match vm_file {
            Some(ref f) => filemap_pin_cache_page(f, file_offset, effective_size, false),
            None => None,
        }
    } else {
        None
    };

    // Allocate new page (skipped when aliasing a shared cache frame or
    // mapping a pinned private-cache frame)
    let phys_addr = if let Some(p) = shared_alias_phys.or(private_cache_phys) {
        PhysAddr::new(p)
    } else {
        match alloc_user_phys_page() {
            Some(addr) => PhysAddr::new(addr),
            None => return MmFaultResult::OutOfMemory,
        }
    };

    // Convert physical address to virtual address for kernel access
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;

    // Initialize page content based on type
    // SAFETY: page_ptr is from phys_to_virt() on a freshly allocated physical page.
    // The page is PAGE_SIZE bytes and exclusively owned by this fault handler.
    // (MAP_SHARED aliased cache frames and MAP_PRIVATE pinned cache frames
    // skip this entirely — their content is the shared page-cache page and
    // must not be reinitialized.)
    if shared_alias_phys.is_none() && private_cache_phys.is_none() {
    // The page is fresh and exclusively owned (INV-RE-ZP: alloc_user_phys_page
    // above); every write/read below targets it.
    {
        match vma_type {
            VmaType::Anonymous => {
                // SAFETY: fresh exclusive page (INV-RE-ZP).
                crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
            }
            VmaType::FileBacked => {
                // Zero-fill the page first (for partial reads and beyond-EOF)
                // SAFETY: fresh exclusive page (INV-RE-ZP).
                crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);

                // Read file data if we have a valid fd
                // (bounds use the CURRENT inode size when the pinned
                // vm_file is available — a file grown by write(2) after
                // the mmap must serve its new pages; the mmap-time
                // snapshot stays only as a fallback: LTP mmapstress04
                // mapped a 1-page file, grew it to 384 pages, and read
                // stale zeros through the snapshot.)
                if vma_file_fd >= 0 {
                    // R-SC-RV64: file, offset and size all come from the
                    // entry-time snapshot; no VMA guard is taken here at
                    // all (the R73 rule — never hold a guard across
                    // read_at's file I/O — is satisfied trivially, and the
                    // old pre-read re-lookup's extra rwlock acquisition is
                    // gone).
                    //
                    // Read from file if within file bounds
                    if file_offset < effective_size {
                        // Prefer the VMA's pinned file (Linux vm_file):
                        // the mapping fd may already be closed —
                        // resolving by fd number failed silently and
                        // produced zero pages (Ubuntu ld.so).
                        let file = vm_file
                            .clone()
                            .or_else(|| crate::fs::get_file_fd(vma_file_fd as usize));
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
                            // peers read): the interleaving read
                            // from the OTHER thread's offset and
                            // mapped .rodata bytes over a .text
                            // page (GNOME family: Xorg InputThread
                            // refault livelock, gnome-session
                            // stack smash, lost tmpfs pages).
                            // read_at never touches the shared
                            // file position.
                            // SAFETY: read_at fills the fresh exclusive
                            // page (INV-RE-ZP); short tails re-zero below.
                            let bytes_read = file.read_at_slice(file_offset as u64, &mut crate::mm::edges::page_slice_mut(page_ptr as usize)[..bytes_to_read]);

                            // Zero remaining bytes after file data (partial last page)
                            if bytes_read > 0 && (bytes_read as usize) < PAGE_SIZE_USIZE {
                                // SAFETY: same fresh page; tail zero.
                                crate::mm::edges::page_slice_mut(page_ptr as usize)[bytes_read as usize..].fill(0);
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
                // SAFETY: fresh exclusive page (INV-RE-ZP).
                crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);
            }
        }
    }
    } // shared_alias_phys.is_none() && private_cache_phys.is_none()

    // Build page table entry flags
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U; // User page

    // Sv39 forbids W-without-R leaf PTEs: a write-only VMA (PROT_WRITE
    // alone) folds to RW here while the VMA itself stays W-only so
    // /proc/self/maps shows the true "-w" permissions (LTP mmap04).
    if vma_flags.is_readable() || vma_flags.is_writable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        // MAP_PRIVATE file-backed pages: map writable directly.
        // The page was just allocated and is exclusive (refcount=1), so no COW
        // is needed. COW marking only happens in copy_page_table_cow() during
        // fork when the page actually becomes shared (refcount >= 2).
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }
    if private_cache_phys.is_some() {
        // Cache-backed leaf (filemap_fault): NEVER W in place — the frame
        // is shared with the page cache (and possibly other mappers). A
        // writable private VMA gets the COW bit and the first write copies
        // (do_cow_fault parity; x86_64 twin). The PCACHE bit records that
        // this PTE owns one cache pin (see cow_flags). Session 26: the COW
        // bit is set on READ-ONLY installs too — "PCACHE set, COW clear"
        // is the SHARED-leaf predicate (pte_is_shared_cache; Sv39 has no
        // third software bit), so a private leaf must never present that
        // combination even before any W-upgrade.
        pte_flags &= !PageTableEntry::W;
        pte_flags |= crate::arch::mm::mm_ops::cow_flags::COW;
        pte_flags |= crate::arch::mm::mm_ops::cow_flags::PCACHE;
    }
    if shared_pcache {
        // SHARED cache leaf (session 25): W stays granted IN PLACE —
        // stores land directly in the cache frame (mapper coherence).
        // The PCACHE bit records the PTE-owned pin (released at unmap).
        // Deliberately NO COW bit: PCACHE-without-COW is exactly how the
        // mprotect/marker-represent sites recognize a shared leaf and
        // keep granting W in place (a COW divert here would detach this
        // mapping from the file's shared state — see pte_is_shared_cache).
        pte_flags |= crate::arch::mm::mm_ops::cow_flags::PCACHE;
    }

    // Map page under the PTE-modify lock and keep it held across the rmap
    // setup: a concurrent fork's copy_page_table_cow walk landing between
    // the map and the rmap/refcount update takes a reference the rmap
    // never sees (round 6 MED — demand-fault PTE lock).
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock_irqsave();
    // R7-C3: re-check under the lock. The already_mapped walk at entry was
    // lock-free; two threads sharing this mm (CLONE_VM) can both see "not
    // mapped", both allocate+zero (+read for file VMAs), and the second
    // map_page would orphan the first page (refcount 1, mapcount 0,
    // unreclaimable) and silently discard stores landed in it.
    // E8-MM: same device-window-hole rule as the entry walk (see
    // pte_counts_as_mapped) — a restored kernel PTE must not be mistaken
    // for the winner's mapping here, or this path "Handles" forever
    // against it while the user instruction keeps refaulting.
    if  PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) 
        .map_or(false, |(_ppn, bits)| {
            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
        })
    {
        drop(_pte_guard);
        // Lost the race: free our exclusively-owned fresh page and let the
        // caller retry — the next entry sees the mapping present.
        // (A cache frame — SHARED PCACHE alias or MAP_PRIVATE pin — is NOT
        // exclusively owned; the pin belongs to the page cache and is
        // returned via put_by_key. Only the LEGACY non-ext4 shared alias
        // keeps its permanent pin, released by nothing here.)
        if private_cache_phys.is_some() || shared_pcache {
            crate::arch::mm::mm_ops::release_cache_mapping_pin(
                (phys_addr.bits() >> PAGE_SHIFT) as usize,
            );
        } else if shared_alias_phys.is_none() {
            crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
        }
        // R8-4: Handled, not AlreadyMapped — the trap handler maps
        // AlreadyMapped to SIGSEGV for user faults, killing the race loser
        // instead of re-executing against the winner's mapping.
        return MmFaultResult::Handled;
    }
    // Map page (root_ppn valid, fault_addr page-aligned, phys_addr a
    // fresh page, pte_flags well-formed — the walk primitive's contract).
    {
        if !map_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure (fake-OOM family): release the data
            // page, deliver OOM (trap layer SIGKILLs the user task). A
            // private pin or SHARED PCACHE pin is returned via put_by_key;
            // only the LEGACY alias's pin is permanent (its frame stays
            // cache-owned).
            if private_cache_phys.is_some() || shared_pcache {
                crate::arch::mm::mm_ops::release_cache_mapping_pin(
                    (phys_addr.bits() >> PAGE_SHIFT) as usize,
                );
            } else if shared_alias_phys.is_none() {
                crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
            }
            return MmFaultResult::OutOfMemory;
        }

        // Address-specific TLB flush
        let vaddr = fault_addr.bits();
        // SAFETY: ASID-scoped TLB fence (memory-safe in any context).
        crate::arch::mm::asid::sfence_vma_addr(vaddr);
    }

    // Set up reverse mapping for the newly mapped page
    {
        use crate::mm::page_desc::{page_desc, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        // The page was just allocated and mapped (INV-FRAME-1 handle).
        if let Some(page) = page_desc(page_pfn) {
                // MAP_SHARED page-cache alias: the frame is owned by the
                // page cache (and possibly other mappers) — take one page
                // reference for THIS PTE mapping so the teardown walkers'
                // put_page() stays balanced, and skip the private-fill
                // text-stamp machinery (stamps assume an exclusive
                // file-fill page; a second mapper would trip FILL-DOUBLE
                // forensics, and the stamp adjudication must never decide
                // the fate of a cache-owned frame).
                if shared_alias_phys.is_some() {
                    page.get_page();
                    page.inc_mapcount();
                    if !shared_pcache {
                        // Legacy non-ext4 alias only. A PCACHE leaf takes
                        // NO rmap record: the descriptor's mapping/index
                        // are the page cache's eviction keys and rmap
                        // state would corrupt eviction (same discipline
                        // as the private cache arm; this retires the
                        // session-24 "unconditional page_record_mapping
                        // on a cache frame" hazard for ext4).
                        crate::mm::rmap::page_record_mapping(
                            page,
                            mm as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                    }
                } else if private_cache_phys.is_some() {
                    // MAP_PRIVATE pinned cache frame (filemap_fault port):
                    // one descriptor reference (dropped by the unmap/COW
                    // put_page) and mapcount visibility. Deliberately NO
                    // set_index / page_record_mapping / LRU add — the
                    // descriptor's mapping/index are the page cache's
                    // eviction keys and the file-LRU membership belongs to
                    // the cache (page_record_mapping's slot-0 set_index
                    // would clobber them; the x86_64 twin has the same
                    // rule). The pin release is keyed off the PTE's PCACHE
                    // bit at unmap/teardown/COW.
                    page.get_page();
                    page.inc_mapcount();
                } else {
                match vma_type {
                    VmaType::Anonymous => {
                        page.set_flag(PageFlag::Anonymous);
                        // SwapBacked + LRU membership: required for vmscan
                        // to consider this page for swap-out (see the stack
                        // growth path above).
                        page.set_flag(PageFlag::SwapBacked);
                        page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        page.inc_mapcount();
                        // Multi-mapping bookkeeping (review 4.10): record
                        // (mm, vpn) so try_to_unmap can find re-mapped
                        // instances of this frame.
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
                    _ => {
                        // Private file copy (write-fault COW fill on a
                        // private VMA — do_cow_fault parity; also the
                        // non-ext4/hole fallbacks). Session 24 (x86_64
                        // session-23 twin): this arm used to leave the
                        // page with NO descriptor state ("rmap not wired
                        // yet") — census-invisible and unreclaimable, the
                        // fake-OOM drainer's accounting face. An exclusive
                        // copy IS an anonymous page from reclaim's point
                        // of view. Device frames keep no state (they
                        // belong to the device/foreign owners).
                        if matches!(vma_type, VmaType::FileBacked) {
                            page.set_flag(PageFlag::Anonymous);
                            page.set_flag(PageFlag::SwapBacked);
                            page.set_index(
                                fault_addr.bits() as usize / (PAGE_SIZE as usize),
                            );
                            page.inc_mapcount();
                            crate::mm::rmap::page_record_mapping(
                                page,
                                mm as *const _ as usize,
                                fault_addr.bits() as usize,
                            );
                            crate::mm::lru::page_add_anon_lru(page);
                        }
                        // SIGILL-stamp (gnome-sigill): record this frame as
                        // a live file-fill page owned by (pid, va) — the
                        // teardown walkers adjudicate frees against the
                        // stamp (see mmu_init::release_user_leaf).
                        let displaced =
                            crate::mm::page_desc::text_stamp_insert(
                                page_pfn,
                                crate::process::current_pid() as u32,
                                fault_addr.bits() as usize,
                            );
                        crate::mm::page_desc::text_stamp_note(
                            page_pfn,
                            crate::process::current_pid() as u32,
                            'F',
                            1,
                        );
                        if let Some((prev_pid, prev_va)) = displaced {
                            // FILL-DOUBLE (gnome-sigill): this frame was
                            // JUST filled for (pid, va) while still stamped
                            // live for (prev_pid, prev_va). Either the
                            // allocator handed the frame out twice, or a
                            // free path returned it without clearing the
                            // stamp — both are zeroed-text-SIGILL family.
                            // Adjudicate the previous owner before printing.
                            let alive_and_maps = crate::arch::mm::mmu_init::
                                textfree_owner_maps(prev_pid, page_pfn);
                            static FILLDOUBLE_REPORTS:
                                core::sync::atomic::AtomicUsize =
                                core::sync::atomic::AtomicUsize::new(0);
                            if FILLDOUBLE_REPORTS.fetch_add(
                                1, core::sync::atomic::Ordering::Relaxed,
                            ) < 16 {
                                crate::pr_err!(
                                    "FILL-DOUBLE pfn={:#x} new=(pid={} va={:#x}) prev=(pid={} va={:#x}) prev_owner_maps={}",
                                    page_pfn,
                                    crate::process::current_pid(),
                                    fault_addr.bits(),
                                    prev_pid, prev_va,
                                    alive_and_maps
                                );
                            }
                        }
                    }
                }
                } // shared_alias bookkeeping else-branch
            }
    }
    drop(_pte_guard);

    // RSS accounting (review 4.14): this address space just gained a
    // resident page.
    mm.add_rss(1);

    // sc181: forward fault-around for large anonymous VMAs (write faults
    // only — see anon_faultaround_forward).
    if vma_type == VmaType::Anonymous
        && is_write
        && !vma_flags.is_shared()
        && vma_end - vma_start >= ANON_FAULTAROUND_MIN_VMA
    {
        anon_faultaround_forward(
            mm,
            root_ppn,
            (fault_addr.as_usize() | 0xFFF) + 1,
            vma_end,
            pte_flags,
        );
    }

    MmFaultResult::Handled
}

/// sc181: install a FORWARD window of zero pages after a successful
/// anonymous write fault.
///
/// hilogd's 10MB log ring (and every large arena) touched its pages one
/// 4KB fault at a time — 2560 full trap/VMA-lookup/alloc/zero/map/rmap/
/// LRU passes per ring cycle; that is the KILLDUMP stime burn behind the
/// sc179/sc180 wedge (hilogd.server 91s of kernel time in 16 min, the
/// VmaManager::find_mut page-by-page attribution). One trap now installs
/// up to ANON_FAULTAROUND_PAGES pages, cutting the trap count ~32x for
/// sequential writers.
///
/// Gated to stay conservative:
/// - WRITE faults only (allocation intent; read-first anon patterns keep
///   exact one-page semantics — mincore03's "untouched mmap reports 0
///   resident" contract is untouched because NO fault means NO window),
/// - VMAs >= ANON_FAULTAROUND_MIN_VMA (small stacks/heaps/brk unchanged),
/// - forward-only, bounded by the VMA end and the window size, and it
///   STOPS at the first already-mapped page (the sequential-writer model;
///   a stack fault's forward window hits older frames and exits at once),
/// - best-effort: an allocation failure or a lost race merely stops the
///   window — the faulting page itself already succeeded.
/// Every window page is zero-filled, SwapBacked, rmap/LRU-recorded and
/// RSS-counted exactly like an individually faulted page (the Anonymous
/// arm of the single-page path above). `faaround=0` on the kernel
/// cmdline disables the whole mechanism.
fn anon_faultaround_forward(
    mm: &MmStruct,
    root_ppn: u64,
    from_page: usize,
    vma_end: usize,
    pte_flags: u64,
) {
    if crate::cmdline::get_param("faaround").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    let window_end = (from_page + ANON_FAULTAROUND_PAGES * PAGE_SIZE_USIZE).min(vma_end);
    let mut va = from_page;
    while va < window_end {
        // Stop at the first already-mapped page (see the doc comment).
        if  PageTableWalker::walk(root_ppn, va as u64) 
            .map_or(false, |(_p, bits)| pte_counts_as_mapped(bits, va as u64))
        {
            return;
        }
        let Some(phys) = alloc_user_phys_page() else {
            return; // pressure: the window is best-effort
        };
        // SAFETY: freshly allocated page, exclusively owned here.
        let page_ptr = phys_to_virt(PhysAddr::new(phys)).bits() as *mut u8;
        crate::mm::edges::page_slice_mut(page_ptr as usize).fill(0);

        let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock_irqsave();
        // R7-C3 twin: re-check under the lock — a CLONE_VM sibling may
        // have installed the page between the lock-free walk and here.
        if  PageTableWalker::walk(root_ppn, va as u64) 
            .map_or(false, |(_p, bits)| pte_counts_as_mapped(bits, va as u64))
        {
            drop(_pte_guard);
            crate::mm::page_alloc::free_page(phys as usize);
            return;
        }
        // SAFETY: root_ppn valid, va page-aligned, phys freshly allocated.
        if !map_page(root_ppn, VirtAddr::new(va as u64), PhysAddr::new(phys), pte_flags)
        {
            drop(_pte_guard);
            crate::mm::page_alloc::free_page(phys as usize);
            return; // page-table allocation trouble — stop the window
        }
        // Address-specific TLB flush (same discipline as the single-page
        // path).
        crate::arch::mm::asid::sfence_vma_addr(va as u64);
        // Bookkeeping: the Anonymous arm of the single-page path.
        {
            use crate::mm::page_desc::{page_desc, PageFlag};
            let page_pfn = (phys >> PAGE_SHIFT) as usize;
            // The frame is exclusively owned until the PTE install above
            // (INV-FRAME-1).
            if let Some(page) = page_desc(page_pfn) {
                page.set_flag(PageFlag::Anonymous);
                page.set_flag(PageFlag::SwapBacked);
                page.set_index(va / PAGE_SIZE_USIZE);
                page.inc_mapcount();
                crate::mm::rmap::page_record_mapping(
                    page,
                    mm as *const _ as usize,
                    va,
                );
                crate::mm::lru::page_add_anon_lru(page);
            }
        }
        drop(_pte_guard);
        mm.add_rss(1);
        va += PAGE_SIZE_USIZE;
    }
}

/// Read the raw PTE value at a virtual address.
///
/// Walks the three-level page table and returns the raw bits of the
/// leaf PTE, even if V=0 (e.g. a swap entry).  Returns None if the
/// page table walk cannot reach the leaf level.
/// R-WRITESTORM-2: is this leaf a PROT_NONE marker (V|U set, no R/W/X,
/// PPN preserved) as written by sys_mprotect for prot==0? The covering
/// VMA owns the protection decision; the fault layer re-presents the
/// leaf when the VMA grants the access.
pub(crate) fn is_protnone_marker(raw: u64) -> bool {
    (raw & PageTableEntry::V) != 0
        && (raw & PageTableEntry::U) != 0
        && (raw & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X)) == 0
        && (raw >> 10) != 0 // has a PPN
}

/// Handle a swap fault — read the page back from swap and map it.
///
/// Called when a page fault hits a PTE that contains a swap entry
/// (V=0, SWAP_ENTRY_SIGNATURE bit set).
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
    use crate::mm::swap;
    use crate::mm::page_desc::{page_desc, PageFlag};
    use crate::mm::page::VirtAddr as PageVirtAddr;

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

    // Wait for an in-flight swap-out of this slot (review 4.12): the
    // single-mapping reclaim path installs the swap entry in the PTE
    // BEFORE the device write; reading the slot before that write lands
    // would swap in garbage. Bounded spin — the writer is a block write
    // on another context.
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
        crate::println!("swap: failed to read page from swap (type={}, offset={})", swap_type, swap_offset);
        // R21-6: free the freshly allocated page on the failed read (was a
        // per-error page leak).
        crate::mm::page_alloc::free_page(phys_addr as usize);
        return MmFaultResult::OutOfMemory;
    }

    // Build PTE flags from VMA permissions
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U;

    // Sv39 forbids W-without-R leaf PTEs: a write-only VMA (PROT_WRITE
    // alone) folds to RW here while the VMA itself stays W-only so
    // /proc/self/maps shows the true "-w" permissions (LTP mmap04).
    if vma_flags.is_readable() || vma_flags.is_writable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }

    // Map the page
    // SAFETY: root_ppn is a valid page table root, phys_addr was just allocated,
    // and pte_flags are built from valid VMA permissions. The page is exclusively owned.
    // Swap-in map under the PTE-modify lock (round 6 MED). The swap read
    // finished above, so no I/O happens inside the irqsave section.
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(mm).lock_irqsave();
    // R7-C3: re-check under the lock (same double-fault race as the demand
    // paths). The swap-in page is exclusively owned, so freeing on loss is
    // safe; the swap slot is freed by the winner.
    // E8-MM: device-window-hole rule applies here too (see
    // pte_counts_as_mapped).
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
    {
        if !map_page(root_ppn, fault_addr, PhysAddr::new(phys_addr), pte_flags) {
            // PT allocation failure (fake-OOM family): free the swap-in
            // page, deliver OOM.
            crate::mm::page_alloc::free_page(phys_addr as usize);
            return MmFaultResult::OutOfMemory;
        }

        // TLB flush
        crate::arch::mm::asid::sfence_vma_addr(fault_addr.bits());
    }

    // Set up rmap for the swapped-in page
    let page_pfn = (phys_addr >> PAGE_SHIFT) as usize;
    // The page was just allocated for swap-in and is not yet shared
    // (INV-FRAME-1).
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

        // Add back to anon LRU
        crate::mm::lru::page_add_anon_lru(page);
    }
    drop(_pte_guard);

    // Free the swap slot (page is back in memory)
    swap::swap_free_slot(swap_type, swap_offset);

    // RSS accounting (review 4.14): the page is resident again.
    mm.add_rss(1);

    MmFaultResult::Handled
}

// ==================== VMA/mmap/brk/fork policy (moved from mm_ops.rs) ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages.
    pub fn map_vma(mm: &MmStruct, vma: Vma, perm: Perm) -> Result<(), MapError> {
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
                // SAFETY: mm.pgd is a valid root PPN and addr is page-aligned within the user
                // heap region which is mapped by this address space.
                if PageTableWalker::walk(mm.pgd, addr as u64).is_none() {
                    let phys_addr = alloc_pages(GfpFlags::GFP_KERNEL, 0);
                    if phys_addr == 0 {
                        return Err(MapError::OutOfMemory);
                    }
                    let flags = perm_to_flags(Perm::ReadWrite, mm.space_type());
                    // Serialize the leaf-PTE write + rmap setup against a
                    // concurrent fork()/COW walk on this mm — brk was the
                    // one leaf-PTE writer left outside PTE_MODIFY_LOCK
                    // (same protocol as the demand-fault paths in
                    // page_fault.rs).
                    let _pte_guard = pte_lock_for(mm).lock_irqsave();
                    // Re-check under the lock: a racing thread of this mm
                    // (CLONE_VM) may have mapped this address between the
                    // walk above and here; mapping again would orphan the
                    // winner's page.
                    if PageTableWalker::walk(mm.pgd, addr as u64).is_some() {
                        drop(_pte_guard);
                        crate::mm::page_alloc::free_page(phys_addr);
                    } else {
                        // mm.pgd is a valid root PPN, addr is page-aligned
                        // in the heap region, and phys_addr is freshly
                        // allocated (the map walker's own contract).
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
                                // the OOM kill instead of the old panic.
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
                            // Fresh exclusive heap page (INV-FRAME-1) via the
                            // typed descriptor edge (the raw pfn_to_page_mut
                            // deref this carried is the core's to audit).
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
        let brk_cur = mm.brk().as_usize();
        let heap_lo = mm.start_brk() & !(PAGE_SIZE_USIZE - 1);
        let heap_hi = if brk_cur > mm.start_brk() {
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
            let mut vma_mgr = mm.vma_write();
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
    pub fn find_free_area(mm: &MmStruct, size: usize) -> Result<PageVirtAddr, MapError> {
        let vma_mgr = mm.vma_read();
        find_free_area_locked(mm, &vma_mgr, size)
    }

    /// Scan body of find_free_area under a lock the CALLER holds (read
    /// for the wrapper, write for mmap's atomic pick-and-insert).
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
    pub fn munmap(mm: &MmStruct, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
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
            let mut vma_mgr = mm.vma_write();

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
            mm.release_unmapped_pages(vs, ve, seg_start, chunk_end);

            let pinned = mm.get_vma_file(vs);
            let _ = vma_mgr.remove(vkey);
            mm.unpin_vma_file(vs);
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
                    mm.pin_vma_file(vs, f);
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
                    mm.pin_vma_file(chunk_end, f);
                }
            }

            mm.unmap_pages(PageVirtAddr::new(seg_start), chunk_end - seg_start)?;

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
    pub fn zap_page_range(mm: &MmStruct, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if start.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }
        if start.as_usize().checked_add(aligned_size).is_none() {
            return Err(MapError::Invalid);
        }
        mm.unmap_pages(start, aligned_size)
    }

    /// Legacy munmap body kept for reference — the new implementation above
    /// replaces it. Previously only whole-VMA removal was supported.
    #[allow(dead_code)]
    fn munmap_legacy(mm: &MmStruct, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        let end_addr = addr.as_usize() + aligned_size;

        {
            let vma_mgr = mm.vma_read();

            let vma_info = vma_mgr.find(addr).map(|vma| {
                (vma.start(), vma.end())
            });
            drop(vma_mgr);

            if let Some((vma_start, vma_end)) = vma_info {
                let vma_start_usize = vma_start.as_usize();
                let vma_end_usize = vma_end.as_usize();

                if addr.as_usize() <= vma_start_usize && end_addr >= vma_end_usize {
                    let mut vma_mgr = mm.vma_write();
                    vma_mgr.remove(vma_start)?;
                } else if addr.as_usize() > vma_start_usize && end_addr < vma_end_usize {
                    return Err(MapError::Invalid);
                } else {
                    return Err(MapError::Invalid);
                }
            }
        }

        mm.unmap_pages(addr, aligned_size)?;

        Ok(())
    }

    /// Rewrite the leaf-PTE permission flags for [start, start+size) in the
    /// USER portion of this address space, preserving each PTE's PPN.
    ///
    /// Used by the init/ELF loaders to tighten a one-shot RWX pre-map down
    /// to per-segment (W^X) permissions (review批次8). `flags` holds the
    /// low PTE bits (V/U/R/W/X/A/D); pages not currently mapped are left
    /// alone. Callers must ensure `end <= USER_END` (kernel range PTEs are
    /// shared and must never be rewritten from here).
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

        let _pte_guard = pte_lock_for(mm).lock_irqsave();
        let mut va = aligned_start;
        while va < end {
            let vpn2 = ((va >> 30) & 0x1FF) as usize;
            let vpn1 = ((va >> 21) & 0x1FF) as usize;
            let vpn0 = ((va >> 12) & 0x1FF) as usize;

            let Some(root_table) = PtTable::from_phys(mm.pgd << PAGE_SHIFT) else {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            };
            let pte2 = root_table.get(vpn2);
            if !pte2.is_valid() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            };
            let pte1 = table1.get(vpn1);
            if !pte1.is_valid() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            };
            let old = table0.get(vpn0);
            if old.is_valid() {
                // Preserve PPN ([53:10]), replace the flag bits.
                let ppn_bits = old.bits() & !0x3FFu64;
                table0.set(vpn0, PageTableEntry::from_bits(ppn_bits | flags));
            }
            va += PAGE_SIZE_USIZE as u64;
        }
        // sfence.vma invalidates TLB entries so the tightened permissions
        // take effect immediately. SAFETY: TLB fence (memory-safe).
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
        // COW-exempt VMA regions (MAP_SHARED / device mappings). fork's COW
        // walk historically write-protected EVERY user-writable PTE — in the
        // PARENT too. A MAP_SHARED mapping (the /dev/fb0 framebuffer under
        // fbterm) then COW-faulted on the first post-fork store: the fault
        // handler allocated a private copy and every subsequent pixel write
        // landed there, never in the scanout backing — the screen froze on
        // its pre-fork content while the program believed it was drawing
        // (kernel-side writes to the same buffer DID reach the display, so
        // the virtio-gpu path was exonerated). Shared and device VMAs must
        // be inherited as-is: bump the refcount, keep the write permission.
        // R-WRITESTORM-3 (fork/VMA atomicity, x86_64 twin): ONE vma_read
        // guard spans the cow_exempt scan, the COW page-table copy, and
        // the VMA snapshot below. The old two-acquisition shape let a
        // same-mm mprotect interleave between the PTE copy and the VMA
        // snapshot, handing the child stale PTEs with upgraded VMA flags.
        let parent_vma_guard = mm.vma_read();

        let cow_exempt: Vec<(u64, u64)> = parent_vma_guard
            .iter()
            .filter(|vma| {
                vma.flags().contains(crate::mm::vma::VmaFlags::SHARED)
                    || vma.vma_type() == crate::mm::vma::VmaType::Device
            })
            .map(|vma| (vma.start().0 as u64, vma.end().0 as u64))
            .collect();

        // SAFETY: mm.pgd is a valid root PPN for the current address space. The caller
        // (fork) guarantees the parent address space is fully initialized and consistent.
        // Per-mm section (split mode): the walk mutates only THIS mm's
        // tables and the child's not-yet-published ones; concurrent COW
        // faults of this mm take the same per-mm lock. Other mms' writers
        // are no longer excluded (they never needed to be).
        let _pte_guard = pte_lock_for(mm).lock_irqsave();
        let new_root_ppn =
            copy_page_table_cow(mm, &cow_exempt).ok_or(MapError::OutOfMemory)?;
        drop(_pte_guard);

        // new_root_ppn was just returned from copy_page_table_cow, so it
        // points to a valid, freshly allocated root page table; space_type
        // and brk are valid by construction. (new_shared still carries the
        // mm-tree's root contract — the mm/ lane owns its conversion.)
        let new_space = MmStruct::new_shared(new_root_ppn, mm.space_type(), mm.brk());

        {
            // Snapshot VMA data under the SAME parent read-guard held
            // since before the PTE copy (R-WRITESTORM-3). The child
            // write-lock below is a different lock (the child's manager).
            let vma_snapshot: Vec<(PageVirtAddr, PageVirtAddr, VmaFlags, VmaType, i32, u64, usize, VmaOrigin)> =
                parent_vma_guard.iter().map(|vma| {
                    (vma.start(), vma.end(), vma.flags(), vma.vma_type(), vma.file_fd(), vma.file_size(), vma.offset(), vma.origin())
                }).collect();
            // Parent read-lock released here: only the child's manager is
            // touched below.
            drop(parent_vma_guard);

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
                        crate::ipc::sysv_shm::shm_attach_vma(file_fd);
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
        }

        // P1 exec demand paging: the child's segment VMAs are unfaulted or
        // COW-shared; their backing image is shared (Arc) with the parent.

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

        // RSS (review 4.14): the child's page tables map (COW-shared) every
        // page the parent had resident — inherit the count so OOM badness
        // sees forked hogs, not zero-rss newborns.
        new_space.add_rss(mm.rss());

        Ok(new_space)
    }

