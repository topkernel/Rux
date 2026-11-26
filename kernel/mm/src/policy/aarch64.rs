//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! aarch64 mm POLICY — the carved half of arch/aarch64/mm/{mm_ops,page_fault}.rs
//! (M1a, the x86_64/riscv64 twin). Compiled ONLY by the mm service crate
//! under crate-level #![forbid(unsafe_code)]; the mechanism twins (walks, PTE
//! atomics, unmap machinery, get_user_phys) stay in the arch files. Moved
//! verbatim except: self->mm free-fn shape, raw zero/inode-deref/read_at onto
//! the audited edges, vestigial unsafe wrappers dropped around already-safe
//! calls, and set_brk's raw pfn_to_page_mut block onto the typed page_desc
//! edge.

use crate::arch::mm::memory_layout::*;
use crate::arch::mm::r#unsafe::PtTable;
use crate::arch::mm::mm_ops::{
    alloc_user_phys_page, check_pte_permissions, copy_page_table_cow, is_cow_page,
    is_protnone_marker, pte_lock_for, PageTableWalker, perm_to_flags, map_user_page,
    handle_cow_fault, leaf_pte_replace,
};
use crate::arch::mm::mmu_init::get_page_table_virt;
use crate::arch::mm::pagetable::*;
use rux::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use rux::mm::vma::VmaType;
use rux::mm::AddressSpace;
use rux::api::mm::{FaultFlags, MmFaultResult};
use rux::mm::mm_struct::MmStruct;
use rux::mm::pagemap::{MapError, Perm, PageTableType};
use rux::mm::vma::{Vma, VmaFlags, VmaOrigin};
use rux::mm::{alloc_pages, GfpFlags};
use alloc::vec::Vec;

/// Try to expand stack when a page fault occurs below the current stack
/// bottom (on-demand GROWSDOWN expansion; twin contract)
fn try_expand_stack(
    mm: &MmStruct,
    fault_addr: VirtAddr,
    flags: u32,
    root_ppn: u64,
) -> MmFaultResult {
    use rux::mm::page::VirtAddr as PageVirtAddr;
    use rux::mm::page::PAGE_SIZE as MM_PAGE_SIZE;
    use rux::mm::vma::VmaFlags;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    let stack_limit = mm.stack_limit();
    let fault_addr_val = fault_addr.as_usize();

    if fault_addr_val < stack_limit {
        return MmFaultResult::Segfault;
    }

    // Find the stack VMA (GROWSDOWN flag)
    let vma_mgr_read = mm.vma_read();
    let (vma_start, stack_vma) = match vma_mgr_read.find_stack_vma(page_virt_addr) {
        Some((start, vma)) => (start, vma),
        None => {
            return MmFaultResult::Segfault;
        }
    };

    let new_start = PageVirtAddr::new(fault_addr_val & !(MM_PAGE_SIZE - 1));

    // New start must be below the current VMA start and above the limit
    if new_start.as_usize() >= vma_start.as_usize() {
        return MmFaultResult::Segfault;
    }
    if new_start.as_usize() < stack_limit {
        return MmFaultResult::Segfault;
    }

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

    drop(vma_mgr_read);

    // Expand the stack VMA downward
    {
        let mut vma_mgr_write = mm.vma_write();
        if vma_mgr_write.expand_downwards(vma_start, new_start).is_err() {
            return MmFaultResult::Segfault;
        }
    }

    // Allocate the new page
    let phys_addr = match alloc_user_phys_page() {
        Some(addr) => PhysAddr::new(addr),
        None => return MmFaultResult::OutOfMemory,
    };

    // Initialize page content based on type
    // SAFETY: page_ptr derives from phys_to_virt on a freshly allocated,
    // exclusively owned page.
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;
    match vma_type {
        VmaType::Anonymous
        | VmaType::FileBacked
        | VmaType::SharedMemory => {
            rux::mm::r#unsafe::page_slice_mut(page_ptr as usize).fill(0);
        }
        VmaType::Device => {
            // Device mapping: don't zero
        }
    }

    // Build the software flag mask from the VMA permissions
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U; // user page
    if vma_flags.is_readable() || vma_flags.is_writable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }

    // Map under the PTE-modify lock, kept across the rmap setup (twin
    // round-6 MED protocol), with the R7-C3 re-check against a CLONE_VM
    // sibling that won the race.
    let _pte_guard = pte_lock_for(mm).lock_irqsave();
    // SAFETY: read-only walk under the lock.
    if PageTableWalker::walk(root_ppn, fault_addr.bits()).is_some() {
        drop(_pte_guard);
        // Lost the race: free our exclusively-owned page; the next entry
        // sees the winner's mapping (Handled, not AlreadyMapped — the
        // trap layer maps AlreadyMapped to SIGSEGV).
        rux::mm::page_alloc::free_page(phys_addr.bits() as usize);
        return MmFaultResult::Handled;
    }
    // SAFETY: root valid, page-aligned address, owned zeroed page.
    {
        if !map_user_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure: free the data page, deliver OOM
            // (trap layer SIGKILLs the user task; twin policy).
            rux::mm::page_alloc::free_page(phys_addr.bits() as usize);
            return MmFaultResult::OutOfMemory;
        }
    }

    // Reverse mapping for the stack page (VMA lock already dropped).
    {
        use rux::mm::page_desc::{page_desc, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        // Exclusively owned until the install (INV-FRAME-1); SwapBacked
        // + LRU membership give reclaim visibility.
        if let Some(page) = page_desc(page_pfn) {
            page.set_flag(PageFlag::Anonymous);
            page.set_flag(PageFlag::SwapBacked);
            page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
            page.inc_mapcount();
            rux::mm::rmap::page_record_mapping(
                page,
                mm as *const _ as usize,
                fault_addr.bits() as usize,
            );
            crate::lru::page_add_anon_lru(page);
        }
    }
    drop(_pte_guard);

    mm.add_rss(1);

    MmFaultResult::Handled
}

/// R-WRITESTORM-2 twin: a fault landed on a PROT_NONE marker leaf
/// (mprotect(prot==0)'s hardware-denied descriptor). The covering VMA
/// is the authority: if it grants the access, re-present the leaf with
/// the VMA's permissions (PPN + COW/PCACHE software bits preserved, W
/// deferred through COW per INV-COW-1); otherwise the access is a real
/// protection violation.
fn protnone_represent(
    mm: &MmStruct,
    root_ppn: u64,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    use rux::mm::page::VirtAddr as PageVirtAddr;

    let is_write = flags & FaultFlags::WRITE != 0;
    let is_exec = flags & FaultFlags::EXEC != 0;
    let is_read = flags & FaultFlags::READ != 0;

    // VMA lookup + access grant (riscv64 twin's verify order)
    let vma_mgr = mm.vma_read();
    let Some(vma) = (vma_mgr.find(PageVirtAddr::new(fault_addr.as_usize()))) else {
        // Was mapped (marker), now outside every VMA — a real gap.
        return MmFaultResult::PermissionDenied;
    };
    let vma_flags = vma.flags();
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

    // Compose the re-presented software mask from the VMA grant
    let mut sw = PageTableEntry::V | PageTableEntry::U
        | PageTableEntry::A | PageTableEntry::D;
    if vma_flags.is_readable() {
        sw |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        sw |= PageTableEntry::W | PageTableEntry::R; // W requires R
    }
    if vma_flags.is_executable() {
        sw |= PageTableEntry::X;
    }

    // Re-present under the PTE section, re-checking the marker (R7-C3
    // discipline: the leaf may have moved under us between the
    // lock-free walk and the section).
    let _pte_guard = pte_lock_for(mm).lock_irqsave();
    // SAFETY: read-only walk on the mm's root.
    let Some((_ppn, now)) = PageTableWalker::walk(root_ppn, fault_addr.bits()) else {
        return MmFaultResult::PermissionDenied;
    };
    if !crate::arch::mm::mm_ops::is_protnone_marker(now) {
        // Someone re-presented/unmapped it first: the fresh state rules.
        return MmFaultResult::Handled;
    }
    let cow_bit = now & crate::arch::mm::mm_ops::cow_flags::COW;
    let pcache_bit = now & crate::arch::mm::mm_ops::cow_flags::PCACHE;
    if cow_bit != 0 {
        sw |= cow_bit;
        sw &= !PageTableEntry::W; // W deferred to the COW fault
    }
    if pcache_bit != 0 {
        sw |= pcache_bit;
        // PRIVATE cache leaf never gains W in place (first write goes
        // through COW); a SHARED cache leaf keeps W (stores must land
        // in the cache frame). PCACHE is dormant on aarch64 today; the
        // ownership rule rides the bit (riscv64 twin).
        if sw & PageTableEntry::W != 0
            && !crate::arch::mm::mm_ops::cow_flags::pte_is_shared_cache(now)
        {
            sw |= crate::arch::mm::mm_ops::cow_flags::COW;
            sw &= !PageTableEntry::W;
        }
    }

    // Write the L3 leaf: rebuild preserves the PPN (OUTPUT_ADDR field).
    // SAFETY: root_ppn is this mm's live root; marker re-checked under
    // the section.
    if let Some(l3) = crate::arch::mm::mmu_init::user_l3_table_for(root_ppn, fault_addr.bits()) {
        let idx = ((fault_addr.bits() >> PAGE_SHIFT) & 0x1FF) as usize;
        l3.set(idx, PageTableEntry::rebuild_user_leaf(now, sw));
        // This translation changed; drop any cached walk.
        crate::arch::mm::asid::flush_tlb_page(fault_addr.bits() as usize, 0);
        return MmFaultResult::Handled;
    }
    MmFaultResult::PermissionDenied
}

/// handle_mm_fault - handle a user-mode page fault (twin contract):
/// 1. Find the VMA to validate the address and permissions
/// 2. Check whether the page is already mapped (COW / permission arms)
/// 3. If unmapped, allocate a new page (zero anonymous / file read /
///    shared-memory zero)
/// 4. Map it with the VMA's permissions under the PTE section, with the
///    R7-C3 re-check
/// 5. Set up rmap/LRU/RSS bookkeeping
pub fn handle_mm_fault(
    mm: &MmStruct,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    use rux::mm::page::VirtAddr as PageVirtAddr;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());
    let root_ppn = mm.root_ppn();

    // Already-mapped check (lock-free; the PROT_NONE marker arm runs in
    // the branch below — markers count as mapped leaves).
    // SAFETY: read-only walk on the mm's root.
    let already_mapped = {
        PageTableWalker::walk(root_ppn, fault_addr.bits()).is_some()
    };

    let is_write = flags & FaultFlags::WRITE != 0;
    let is_exec = flags & FaultFlags::EXEC != 0;
    let is_read = flags & FaultFlags::READ != 0;
    let is_user = flags & FaultFlags::USER != 0;

    if already_mapped {
        // R-WRITESTORM-2 (riscv64 twin): if the leaf is a PROT_NONE
        // marker (V|U, no R/W/X, PPN preserved — installed by
        // mprotect(prot==0)) and the covering VMA grants this access,
        // RE-PRESENT the leaf with the VMA's permissions, preserving
        // the PPN and the COW software bit (W deferred to the COW
        // fault, INV-COW-1). A stale marker is advisory PTE state, not
        // a protection decision: a fault under a VMA that grants the
        // access must succeed regardless of the region's mprotect
        // history (the marker's HW denial raised this fault; the VMA
        // is the authority now).
        // SAFETY: read-only walk on the mm's root.
        if let Some((_ppn, raw)) = PageTableWalker::walk(root_ppn, fault_addr.bits()) {
            if crate::arch::mm::mm_ops::is_protnone_marker(raw) {
                return protnone_represent(mm, root_ppn, fault_addr, flags);
            }
        }

        // COW check first (write fault on an RO+COW leaf)
        // SAFETY: read-only walk.
        if is_write && is_cow_page(root_ppn, fault_addr) {
            crate::pr_debug!("pagefault: cow at {:#x}", fault_addr.bits());
            return MmFaultResult::CowPending;
        }

        // Permission check against the leaf's software bits
        // SAFETY: read-only walk.
        if let Some((has_read, has_write, has_exec, pte_is_user)) =
            check_pte_permissions(root_ppn, fault_addr)
        {
            let perm_ok = (!is_write || has_write)
                && (!is_read || has_read)
                && (!is_exec || has_exec)
                && (!is_user || pte_is_user);
            if perm_ok {
                // Permissions fine (e.g. AF-clear style refault): flush
                // and retry.
                // SAFETY: translation unchanged; drop any stale entry.
                crate::arch::mm::asid::flush_tlb_page(fault_addr.bits() as usize, 0);
                return MmFaultResult::Handled;
            }
        }
        return MmFaultResult::PermissionDenied;
    }

    // Find the VMA
    let vma_mgr = mm.vma_read();
    let vma = match vma_mgr.find(page_virt_addr) {
        Some(v) => v,
        None => {
            drop(vma_mgr);
            return try_expand_stack(mm, fault_addr, flags, root_ppn);
        }
    };

    // Capture everything the fill arms need under this one guard (twin
    // R-SC note: no re-derived mismatched triples).
    let vma_flags = vma.flags();
    let vma_type = vma.vma_type();
    let vma_file_fd = vma.file_fd();
    let vma_file_size = vma.file_size();
    let vma_offset = vma.offset();
    let vma_start = vma.start().as_usize();

    // Pinned vm_file (Linux vm_file semantics: the mapping must survive
    // fd close).
    let vm_file = if vma_type == VmaType::FileBacked && vma_file_fd >= 0 {
        mm.get_vma_file(vma_start)
    } else {
        None
    };
    let file_offset = vma_offset + (page_virt_addr.as_usize() - vma_start);
    // Effective size: CURRENT inode size when resolvable (a file grown
    // or truncated since mmap must serve/shrink accordingly); the
    // mmap-time snapshot stays as the fallback.
    let effective_size = vm_file
        .as_ref()
        .and_then(|f| {
            if let Some(live) = crate::fs::hotpath::devfs::reg_current_size(f.as_ref()) {
                return Some(live as usize);
            }
            // SAFETY: inode cell written at open time; read-only.
            let inode_opt = crate::fs::r#unsafe::file_inode(f);
            inode_opt.map(|i| i.get_size() as usize)
        })
        .unwrap_or(vma_file_size as usize);

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

    crate::pr_debug!(
        "pagefault: map new page at {:#x}, type={:?}",
        fault_addr.bits(),
        vma_type
    );

    // Release the read lock before any file I/O
    drop(vma_mgr);

    // File-backed fault BEYOND the file's end is SIGBUS, not a zero page
    // (twin filemap_fault contract: LTP mmap13 truncation shape).
    if vma_type == VmaType::FileBacked && vma_file_fd >= 0 {
        if file_offset >= effective_size {
            return MmFaultResult::BusError;
        }
    }

    // MAP_SHARED file mappings and device mappings keep the twin's
    // private-copy semantics here (see the module doc: the page-cache
    // alias port is pending). Zero-fill + read covers correctness.

    // Allocate the new page
    let phys_addr = match alloc_user_phys_page() {
        Some(addr) => PhysAddr::new(addr),
        None => return MmFaultResult::OutOfMemory,
    };

    // Initialize page content based on type
    // Exclusively owned fresh page inside the linear map (INV-AE-ZP).
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;
    {
        match vma_type {
            VmaType::Anonymous | VmaType::SharedMemory => {
                // SAFETY: fresh exclusive page (INV-AE-ZP).
                rux::mm::r#unsafe::page_slice_mut(page_ptr as usize).fill(0);
            }
            VmaType::FileBacked => {
                // Zero-fill first (partial reads / tail holes), then read
                // the file data. Bounds use the CURRENT inode size (a
                // file grown by write(2) after mmap must serve its new
                // pages — twin mmapstress04 note); positional read_at so
                // the shared file position is never touched (twin R71).
                // SAFETY: fresh exclusive page (INV-AE-ZP).
                rux::mm::r#unsafe::page_slice_mut(page_ptr as usize).fill(0);
                if vma_file_fd >= 0 && file_offset < effective_size {
                    // Prefer the VMA's pinned vm_file (the mapping fd may
                    // already be closed — twin ld.so note).
                    let file = vm_file.clone().or_else(|| {
                        if vma_file_fd >= 0 {
                            // SAFETY: fd-table lookup.
                            crate::fs::hotpath::get_file_fd(vma_file_fd as usize)
                        } else {
                            None
                        }
                    });
                    if let Some(file) = file {
                        let bytes_to_read = core::cmp::min(
                            PAGE_SIZE_USIZE,
                            effective_size.saturating_sub(file_offset),
                        );
                        // SAFETY: page_ptr covers PAGE_SIZE bytes; the
                        // file was resolved from the fd table.
                        let bytes_read = file.read_at_slice(file_offset as u64, &mut rux::mm::r#unsafe::page_slice_mut(page_ptr as usize)[..bytes_to_read]);
                        if bytes_read > 0 && (bytes_read as usize) < PAGE_SIZE_USIZE {
                            // SAFETY: same fresh page; tail zero.
                            rux::mm::r#unsafe::page_slice_mut(page_ptr as usize)[bytes_read as usize..].fill(0);
                        }
                    }
                }
            }
            VmaType::Device => {
                // Device mapping: don't zero
            }
        }
    }

    // Build the software flag mask from the VMA permissions (aarch64 has
    // no W-without-R reserved encoding: a W-only VMA maps RW in hardware
    // while the VMA keeps the true "-w" view).
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U;
    if vma_flags.is_readable() || vma_flags.is_writable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        // Private pages map writable directly: the page is exclusive
        // (refcount 1), COW marking happens at fork (twin note).
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }

    // Map under the PTE-modify lock, held across the rmap setup (twin
    // round-6 MED), with the R7-C3 re-check (CLONE_VM double-fault race).
    let _pte_guard = pte_lock_for(mm).lock_irqsave();
    // SAFETY: read-only walk under the lock.
    if PageTableWalker::walk(root_ppn, fault_addr.bits()).is_some() {
        drop(_pte_guard);
        // Lost the race: free our exclusively-owned page; retry sees the
        // winner's mapping (Handled — never AlreadyMapped, which the trap
        // layer maps to SIGSEGV).
        rux::mm::page_alloc::free_page(phys_addr.bits() as usize);
        return MmFaultResult::Handled;
    }
    // SAFETY: valid root, page-aligned fault address, owned filled page.
    {
        if !map_user_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure: release the data page, deliver OOM.
            rux::mm::page_alloc::free_page(phys_addr.bits() as usize);
            return MmFaultResult::OutOfMemory;
        }
    }

    // Reverse mapping + LRU for the newly mapped page (twin arms).
    {
        use rux::mm::page_desc::{page_desc, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        // Frame exclusively owned until the install above (INV-FRAME-1).
        if let Some(page) = page_desc(page_pfn) {
            {
                match vma_type {
                    VmaType::Anonymous => {
                        page.set_flag(PageFlag::Anonymous);
                        page.set_flag(PageFlag::SwapBacked);
                        page.set_index(
                            fault_addr.bits() as usize / (PAGE_SIZE as usize),
                        );
                        page.inc_mapcount();
                        rux::mm::rmap::page_record_mapping(
                            page,
                            mm as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                        crate::lru::page_add_anon_lru(page);
                    }
                    VmaType::SharedMemory => {
                        page.set_flag(PageFlag::Anonymous);
                        page.set_index(
                            fault_addr.bits() as usize / (PAGE_SIZE as usize),
                        );
                        page.inc_mapcount();
                        // Shmem stays off the anon LRU (twin note:
                        // try_to_unmap only walks Anonymous VMAs).
                        rux::mm::rmap::page_record_mapping(
                            page,
                            mm as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                    }
                    _ => {
                        // Private file copy (the twin's do_cow_fault
                        // parity arm): an exclusive copy IS an anonymous
                        // page from reclaim's point of view. Device
                        // frames keep no state.
                        if matches!(vma_type, VmaType::FileBacked) {
                            page.set_flag(PageFlag::Anonymous);
                            page.set_flag(PageFlag::SwapBacked);
                            page.set_index(
                                fault_addr.bits() as usize / (PAGE_SIZE as usize),
                            );
                            page.inc_mapcount();
                            rux::mm::rmap::page_record_mapping(
                                page,
                                mm as *const _ as usize,
                                fault_addr.bits() as usize,
                            );
                            crate::lru::page_add_anon_lru(page);
                        }
                    }
                }
            }
        }
    }
    drop(_pte_guard);

    // RSS accounting: this address space gained a resident page.
    mm.add_rss(1);

    MmFaultResult::Handled
}

// ==================== VMA/mmap/brk/fork policy (moved from mm_ops.rs) ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages.
    pub fn map_vma(mm: &MmStruct, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let _ = perm;
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
    /// in-kernel caller reappears. (riscv64 twin note, carried over.)
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
                // SAFETY: mm.pgd is a valid root PPN and addr is
                // page-aligned within the user heap region.
                if PageTableWalker::walk(mm.pgd, addr as u64).is_none() {
                    let phys_addr = alloc_pages(GfpFlags::GFP_KERNEL, 0);
                    if phys_addr == 0 {
                        return Err(MapError::OutOfMemory);
                    }
                    let flags = perm_to_flags(Perm::ReadWrite, mm.space_type());
                    // Serialize the leaf-PTE write + rmap setup against a
                    // concurrent fork()/COW walk on this mm (riscv64 twin
                    // protocol, INV-LOCK-7 per-mm section).
                    let _pte_guard = pte_lock_for(mm).lock_irqsave();
                    // Re-check under the lock: a racing CLONE_VM thread
                    // may have mapped this address between the walk and
                    // here; mapping again would orphan the winner's page.
                    if PageTableWalker::walk(mm.pgd, addr as u64).is_some() {
                        drop(_pte_guard);
                        rux::mm::page_alloc::free_page(phys_addr);
                    } else {
                        // Valid root, page-aligned heap address,
                        // exclusively owned fresh page.
                        {
                            if !map_user_page(
                                mm.pgd,
                                VirtAddr::new(addr as u64),
                                PhysAddr::new(phys_addr as u64),
                                flags,
                            ) {
                                // PT allocation failure: free the data
                                // page (anti-leak) and leave the address
                                // unmapped — the demand-fault path
                                // delivers the OOM later instead of a
                                // kernel panic here.
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
                        // Reverse mapping for the heap page
                        {
                            use rux::mm::page_desc::{page_desc, PageFlag};
                            if let Some(page) = page_desc(phys_addr / (PAGE_SIZE as usize)) {
                                page.set_flag(PageFlag::Anonymous);
                                page.set_flag(PageFlag::SwapBacked);
                                page.set_index(addr / (PAGE_SIZE as usize));
                                page.inc_mapcount();
                                rux::mm::rmap::page_record_mapping(
                                    page,
                                    mm as *const _ as usize,
                                    addr,
                                );
                                crate::lru::page_add_anon_lru(page);
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
        let is_fixed = map_flags & (map::MAP_FIXED | map::MAP_FIXED_NOREPLACE) != 0;

        let end_addr = addr.as_usize() + aligned_size;
        // Reference-kernel parity (riscv64 twin): MAP_FIXED replaces only
        // the covered range; refuse only the LIVE brk heap span
        // [align_down(start_brk), align(brk)) — mapping over malloc's own
        // storage stays EINVAL (documented divergence: we never silently
        // free brk-managed pages).
        let brk_cur = mm.brk().as_usize();
        let heap_lo = mm.start_brk() & !(PAGE_SIZE_USIZE - 1);
        let heap_hi = if brk_cur > mm.start_brk() {
            (brk_cur + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1)
        } else {
            heap_lo
        };
        let has_brk_conflict = addr.as_usize() < heap_hi && end_addr > heap_lo;

        // Placement + VMA insert share one write-lock section (atomic
        // pick-and-insert; riscv64/x86_64 twin TOCTOU fix).
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
            // MAP_FIXED below vm.mmap_min_addr. See the x86_64 twin
            // for the full statement of scope (a hardening step, not
            // the NOVMA-EXEC fix) and the deliberate differences.
            if start.as_usize() < user_addr::MIN_MAP_ADDR {
                return Err(MapError::Invalid);
            }
            // Upper bound: user TASK_SIZE (256GB) — every user mapping
            // must stay under TTBR0 L0 entry 0.
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
            // ATOMIC find + insert under ONE vma_write guard; hint policy
            // mirrors the twins: a free hint anywhere in the user range is
            // honored, MIN_MAP_ADDR / live-brk-span / past-USER_END hints
            // are ignored (never an error).
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
            vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;
            drop(vma_mgr);
            mm.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
            mm.update_highest_vm_end(chosen.as_usize() + aligned_size);
            vma_installed = true;
            chosen
        };

        if is_fixed {
            // Reference-kernel semantics: MAP_FIXED replaces only the
            // covered range; overlapping VMAs are SPLIT with
            // type/flags/fd/offset preserved. Whole-range teardown via
            // the CHUNKED munmap loop (the same bounded-hold discipline
            // as sys_munmap — see munmap; the old shape held vma_write
            // across the ENTIRE fixed range's surgery AND PTE-zap walk,
            // the whole-range VMA-lock convoy shape). The install
            // re-checks under its own guard; a same-mm racer landing in
            // the cleared gap is cleared by one bounded retry (a legal
            // linearization for concurrent mmap/MAP_FIXED on the same
            // range). Split head/tail preservation and pin (vm_file)
            // discipline are munmap's own.
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
            // Unreachable today (both paths install inline); safety net
            // for future placement paths.
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
        vma_mgr: &rux::mm::vma::VmaManager,
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
                search_start =
                    (vma.end().as_usize() + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
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

    /// munmap system call implementation — partial-VMA removal with
    /// split at the unmapped boundaries (twin review ARCH-M3 / SYSA-H1).
    pub fn munmap(mm: &MmStruct, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // Overflow-safe page rounding (twin s15: user lengths within one
        // page of usize::MAX must EINVAL, not panic/wrap).
        let aligned_size = match size.checked_add(PAGE_SIZE_USIZE - 1) {
            Some(rounded) => rounded & !(PAGE_SIZE_USIZE - 1),
            None => return Err(MapError::Invalid),
        };

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        // User-range bound: a kernel-range munmap would walk the TTBR1
        // tree and free kernel page descriptors (twin R7-1).
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
        // user (sys_brk above all) for the full zap PLUS scheduler
        // starvation of the holder. The surgery is CHUNKED: each
        // iteration removes at most MUNMAP_CHUNK_BYTES of ONE VMA — that
        // piece's surgery AND its PTE zap still share ONE guard
        // (mm-audit rule: a racing mmap may only land in a
        // FULLY-CONSISTENT hole — VMA gone implies PTEs cleared at every
        // guard drop) — then the guard is dropped, cond_resched()
        // yields, and the tree walk is RE-DERIVED (the tree can change
        // under us between holds). unmap_pages takes no VMA lock (PTE
        // lock only, nested one-way), so the hold is safe.
        //
        // CURSOR (89th session, Face-A closure — twin of the x86_64 fix):
        // the search starts at the CURSOR (the end of the last processed
        // piece), never back at addr_us. Re-deriving from addr_us let a
        // fresh same-mm mmap that legally landed in an already-processed
        // hole be found by the NEXT iteration's search and deleted as an
        // "unprocessed piece" of THIS munmap.
        //
        // Pin-table discipline (vm_file semantics): a split FILE VMA's
        // tail re-pins at its own start (vm_file follows the VMA).
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

            mm.release_unmapped_pages(vs, ve, seg_start, chunk_end);

            let pinned = mm.get_vma_file(vs);
            let _ = vma_mgr.remove(vkey);
            mm.unpin_vma_file(vs);
            if vs < seg_start {
                let mut head = Vma::new(
                    PageVirtAddr::new(vs),
                    PageVirtAddr::new(seg_start),
                    vflags,
                );
                head.set_type(vtype);
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
                    PageVirtAddr::new(chunk_end),
                    PageVirtAddr::new(ve),
                    vflags,
                );
                tail.set_type(vtype);
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
    /// VMAs — madvise(MADV_DONTNEED) semantics (twin contract).
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

    /// Copy address space using Copy-on-Write (twin contract: COW-exempt
    /// VMA ranges inherited writable, everything else downgraded in BOTH
    /// parent and child; one vma_read guard spans the exempt scan, the
    /// PTE copy and the VMA snapshot — R-WRITESTORM-3).
    pub fn fork(mm: &MmStruct) -> Result<MmStruct, MapError> {
        let parent_vma_guard = mm.vma_read();

        let cow_exempt: Vec<(u64, u64)> = parent_vma_guard
            .iter()
            .filter(|vma| {
                vma.flags().contains(rux::mm::vma::VmaFlags::SHARED)
                    || vma.vma_type() == rux::mm::vma::VmaType::Device
            })
            .map(|vma| (vma.start().0 as u64, vma.end().0 as u64))
            .collect();

        // SAFETY: mm.pgd is a valid root PPN for the current address
        // space; the caller (fork) guarantees the parent is fully
        // initialized and consistent.
        // Per-mm section (split mode; riscv64 twin comment): the walk
        // mutates only THIS mm's tables and the child's unpublished ones.
        let _pte_guard = pte_lock_for(mm).lock_irqsave();
        let new_root_ppn = {
            copy_page_table_cow(mm, &cow_exempt).ok_or(MapError::OutOfMemory)?
        };
        drop(_pte_guard);

        // SAFETY: new_root_ppn comes from copy_page_table_cow — a valid,
        // freshly allocated root. space_type and brk are valid by
        // construction from the current address space.
        let new_space = {
            MmStruct::new_shared(
                new_root_ppn,
                mm.space_type(),
                mm.brk(),
            )
        };

        {
            // Snapshot VMA data under the SAME parent read-guard held
            // since before the PTE copy (twin R-WRITESTORM-3).
            let vma_snapshot: Vec<(
                PageVirtAddr,
                PageVirtAddr,
                VmaFlags,
                VmaType,
                i32,
                u64,
                usize,
                VmaOrigin,
            )> = parent_vma_guard
                .iter()
                .map(|vma| {
                    (
                        vma.start(),
                        vma.end(),
                        vma.flags(),
                        vma.vma_type(),
                        vma.file_fd(),
                        vma.file_size(),
                        vma.offset(),
                        vma.origin(),
                    )
                })
                .collect();
            drop(parent_vma_guard);

            if !vma_snapshot.is_empty() {
                let mut new_vma_mgr = new_space.vma_write();
                for (start, end, flags, vma_type, file_fd, file_size, offset, origin) in
                    vma_snapshot
                {
                    let mut new_vma = Vma::new(start, end, flags);
                    new_vma.set_type(vma_type);
                    let mut o = origin;
                    o.mark_forked();
                    new_vma.set_origin(o);
                    new_vma.set_file_fd(file_fd);
                    new_vma.set_file_size(file_size);
                    new_vma.set_offset(offset);
                    // Clone the pinned vm_file so the child's demand
                    // faults read the file after fd close (twin P1).
                    if vma_type == rux::mm::vma::VmaType::FileBacked {
                        if let Some(f) = mm.get_vma_file(start.as_usize()) {
                            new_space.pin_vma_file(start.as_usize(), f);
                        }
                    }
                    // Shared-memory attachments: increment nattch.
                    if vma_type == rux::mm::vma::VmaType::SharedMemory && file_fd >= 0 {
                        rux::api::ipc::shm_attach_vma(file_fd);
                    }
                    let copied_pages = new_vma.page_count() as u64;
                    let _ = new_vma_mgr.add(new_vma);
                    new_space.add_total_vm(copied_pages);
                }
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
        // dup_mm parity: the child that later crashes dumps the auxv of
        // the exec that built its image.
        {
            let auxv = mm.saved_auxv();
            if !auxv.is_empty() {
                new_space.set_saved_auxv(&auxv);
            }
        }

        // RSS: the child's tables map (COW-shared) every page the parent
        // had resident — inherit the count for OOM badness.
        new_space.add_rss(mm.rss());

        Ok(new_space)
    }

