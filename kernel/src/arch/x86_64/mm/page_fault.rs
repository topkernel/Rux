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
use super::mmu_init::{map_page, get_page_table_virt, ROOT_PAGE_TABLE};
use super::mm_ops::{alloc_user_phys_page, is_cow_page, check_pte_permissions, PageTableWalker};
use super::pagetable::*;
use crate::mm::page::PAGE_SIZE as PAGE_SIZE_USIZE;
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Fault access flags (u32 bitset, matching generic callers)
pub struct FaultFlags;
impl FaultFlags {
    /// Read fault
    pub const READ: u32 = 1 << 0;
    /// Write fault
    pub const WRITE: u32 = 1 << 1;
    /// Execute fault (instruction fetch)
    pub const EXEC: u32 = 1 << 2;
    /// User mode access
    pub const USER: u32 = 1 << 3;
    /// Kernel mode access
    pub const KERNEL: u32 = 1 << 4;
}

// ==================== Fault Result ====================

/// Outcome of a handled fault
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Fault fully handled — retry the instruction
    Handled,
    /// Anonymous page materialized — retry
    Fixed,
    /// COW resolved — retry
    CowPending,
    /// Mapping already exists — retry
    AlreadyMapped,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Permission denied (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Bus error — file-backed fault past EOF (SIGBUS to the task)
    BusError,
    /// Kernel exception fixed (via exception table)
    KernelPanic,
}

// ==================== Stack Expansion ====================

/// Try to expand stack when page fault occurs below current stack bottom
///
/// On-demand stack expansion.
fn try_expand_stack(
    addr_space: &AddressSpace,
    fault_addr: VirtAddr,
    flags: u32,
    root_ppn: u64,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;
    use crate::mm::page::PAGE_SIZE as MM_PAGE_SIZE;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    let stack_limit = addr_space.stack_limit();
    let fault_addr_val = fault_addr.as_usize();

    // Check if fault address is within stack expansion range
    if fault_addr_val < stack_limit {
        return MmFaultResult::Segfault;
    }

    // Try to find the stack VMA (with GROWSDOWN flag)
    let vma_mgr_read = addr_space.vma_read();
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
        let mut vma_mgr_write = addr_space.vma_write();
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
    unsafe {
        match vma_type {
            VmaType::Anonymous => {
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::FileBacked | VmaType::SharedMemory => {
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::Device => {
                // Device mapping: don't zero
            }
        }
    }

    // Build page table entry flags (x86: NX set unless the VMA is
    // executable; write adds RW; user pages carry US).
    let mut pte_flags = PageTableEntry::P | PageTableEntry::A | PageTableEntry::D
        | PageTableEntry::NX;
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
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    // Re-check under the lock. The already_mapped walk at entry was
    // lock-free; two threads sharing this mm (CLONE_VM) can both see "not
    // mapped", both allocate+zero, and the second map_page would orphan
    // the first page (refcount 1, mapcount 0, unreclaimable) and silently
    // discard stores landed in it. E8-MM: same device-window-hole rule as
    // the entry walk (see pte_counts_as_mapped) — a restored kernel PTE
    // must not be mistaken for the winner's mapping here, or this path
    // "Handles" forever against it while the user instruction keeps
    // refaulting.
    if unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) }
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
    unsafe {
        map_page(root_ppn, fault_addr, phys_addr, pte_flags);

        // Address-specific TLB flush
        crate::arch::cpu::invlpg(fault_addr.bits());
    }

    // Set up reverse mapping for stack page (VMA lock already dropped,
    // so set fields directly instead of calling page_add_anon_rmap)
    {
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        let page = pfn_to_page_mut(page_pfn);
        if !page.is_null() {
            // SAFETY: page is from pfn_to_page_mut and checked for null.
            // This page was just allocated and is not yet shared, so exclusive access is guaranteed.
            unsafe {
                (*page).set_flag(PageFlag::Anonymous);
                // SwapBacked + LRU_INACTIVE_ANON membership make the page
                // visible to the reclaim engine — vmscan only swaps out
                // pages that sit on LRU_INACTIVE_ANON with SwapBacked set.
                // Without this, anonymous demand faults accumulate as
                // unreclaimable memory and the system OOMs with swap idle.
                (*page).set_flag(PageFlag::SwapBacked);
                (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                (*page).inc_mapcount();
                crate::mm::rmap::page_record_mapping(
                    &*page,
                    addr_space as *const _ as usize,
                    fault_addr.bits() as usize,
                );
                crate::mm::lru::page_add_anon_lru(&*page);
            }
        }
    }
    drop(_pte_guard);

    // RSS accounting: stack page became resident.
    addr_space.add_rss(1);

    MmFaultResult::Handled
}

// ==================== Main Fault Handler ====================

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

/// handle_mm_fault - Handle user mode page fault
///
/// # Arguments
/// - `addr_space`: Address space
/// - `fault_addr`: Virtual address that triggered fault
/// - `flags`: Fault type flags (FaultFlags)
pub fn handle_mm_fault(
    addr_space: &AddressSpace,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    let root_ppn = addr_space.root_ppn();

    // Check if page is already mapped
    // SAFETY: root_ppn is the address space's valid root page table PPN.
    // PageTableWalker::walk only reads page table entries.
    let already_mapped = unsafe {
        PageTableWalker::walk(root_ppn, fault_addr.bits() as u64).map_or(false, |(_ppn, bits)| {
            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
        })
    };

    // If not mapped, check for a swap entry in the PTE (P=0 but non-zero bits)
    if !already_mapped {
        if let Some(entry) = read_pte_raw(root_ppn, fault_addr) {
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
                return handle_swap_fault(addr_space, fault_addr, flags, entry, root_ppn);
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
        if is_write && unsafe { is_cow_page(root_ppn, fault_addr) } {
            crate::pr_debug!("pagefault: cow at {:#x}", fault_addr.bits());
            return MmFaultResult::CowPending;
        }

        // Check if page permissions meet access requirements
        // SAFETY: root_ppn is a valid page table root. check_pte_permissions only reads PTEs.
        if let Some((has_read, has_write, has_exec, pte_is_user)) =
            unsafe { check_pte_permissions(root_ppn, fault_addr) }
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
                unsafe {
                    crate::arch::cpu::invlpg(fault_addr.bits());
                }
                return MmFaultResult::Handled;
            }
        }

        return MmFaultResult::PermissionDenied;
    }

    // Find VMA
    let vma_mgr = addr_space.vma_read();
    let vma = match vma_mgr.find(page_virt_addr) {
        Some(v) => v,
        None => {
            drop(vma_mgr);
            return try_expand_stack(addr_space, fault_addr, flags, root_ppn);
        }
    };

    // Get VMA attributes
    let vma_flags = vma.flags();
    let vma_type = vma.vma_type();
    let vma_file_fd = vma.file_fd();
    let vma_file_size = vma.file_size();
    let vma_offset = vma.offset();

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

    crate::pr_debug!("pagefault: map new page at {:#x}, type={:?}", fault_addr.bits(), vma_type);

    // Release read lock
    drop(vma_mgr);

    // File-backed fault BEYOND the file's end is SIGBUS, not a zero page
    // (Linux filemap_fault: no page at/after EOF can be served for a
    // mapping — LTP mmap13 truncates a mapped file and expects the next
    // touch of the cut page to raise SIGBUS). The effective size is the
    // CURRENT inode size (a truncate after mmap shrinks it), falling
    // back to the snapshot taken at mmap time.
    if vma_type == VmaType::FileBacked && vma_file_fd >= 0 {
        if let Some(aspace) = crate::sched::current().and_then(|t| t.address_space()) {
            if let Some(found_vma) = aspace.vma_read().find(page_virt_addr) {
                let vma_start = found_vma.start().as_usize();
                let file_offset = vma_offset + (page_virt_addr.as_usize() - vma_start);
                let effective_size = addr_space
                    .get_vma_file(vma_start)
                    .and_then(|f| {
                        // SAFETY: inode cell written at open time; read-only.
                        let inode_opt = unsafe { &*f.inode.get() };
                        inode_opt.as_ref().map(|i| i.get_size())
                    })
                    .unwrap_or(vma_file_size) as usize;
                if file_offset >= effective_size {
                    return MmFaultResult::BusError;
                }
            }
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
    // SAFETY: page_ptr is from phys_to_virt() on a freshly allocated physical page.
    // The page is PAGE_SIZE bytes and exclusively owned by this fault handler.
    unsafe {
        match vma_type {
            VmaType::Anonymous => {
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::FileBacked => {
                // Zero-fill the page first (for partial reads and beyond-EOF)
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);

                // Read file data if we have a valid fd
                // (bounds use the CURRENT inode size when the pinned
                // vm_file is available — a file grown by write(2) after
                // the mmap must serve its new pages; the mmap-time
                // snapshot stays only as a fallback).
                if vma_file_fd >= 0 {
                    if let Some(aspace) = crate::sched::current().and_then(|t| t.address_space()) {
                        if let Some(found_vma) = aspace.vma_read().find(page_virt_addr) {
                            let vma_start = found_vma.start().as_usize();
                            let page_offset_in_mapping = page_virt_addr.as_usize() - vma_start;
                            let file_offset = vma_offset + page_offset_in_mapping;

                            // Effective size: CURRENT inode size (grown or
                            // truncated since mmap), mmap snapshot as fallback.
                            // (inode cell written at open time; read-only —
                            // the enclosing unsafe block covers the deref.)
                            let effective_size = aspace
                                .get_vma_file(vma_start)
                                .and_then(|f| {
                                    let inode_opt = &*f.inode.get();
                                    inode_opt.as_ref().map(|i| i.get_size())
                                })
                                .unwrap_or(vma_file_size) as usize;

                            // Read from file if within file bounds
                            if file_offset < effective_size {
                                // Prefer the VMA's pinned file (Linux vm_file):
                                // the mapping fd may already be closed —
                                // resolving by fd number failed silently and
                                // produced zero pages (Ubuntu ld.so).
                                let file = addr_space
                                    .get_vma_file(vma_start)
                                    .or_else(|| crate::fs::get_file_fd(vma_file_fd as usize));
                                if let Some(file) = file {
                                    let saved_pos = file.get_pos();
                                    file.set_pos(file_offset as u64);

                                    let bytes_to_read = core::cmp::min(
                                        PAGE_SIZE_USIZE,
                                        effective_size.saturating_sub(file_offset),
                                    );
                                    let bytes_read = file.read(page_ptr, bytes_to_read);

                                    file.set_pos(saved_pos);

                                    // Zero remaining bytes after file data (partial last page)
                                    if bytes_read > 0 && (bytes_read as usize) < PAGE_SIZE_USIZE {
                                        core::ptr::write_bytes(
                                            page_ptr.add(bytes_read as usize), 0,
                                            PAGE_SIZE_USIZE - bytes_read as usize,
                                        );
                                    }
                                }
                            }
                            // Beyond file size: page stays zero-filled (sparse / hole)
                        }
                    }
                }
            }
            VmaType::Device => {
                // Device mapping: don't zero
            }
            VmaType::SharedMemory => {
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
        }
    }

    // Build page table entry flags (x86: exec is default and non-exec is
    // NX; write adds RW; user pages carry US. A write-only VMA folds to
    // RW — write requires read on x86, mirroring the riscv twin's
    // W-requires-R rule.)
    let mut pte_flags = PageTableEntry::P | PageTableEntry::A | PageTableEntry::D
        | PageTableEntry::NX;
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
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    // Re-check under the lock (same double-fault race as above; E8-MM
    // device-window-hole rule applies).
    if unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) }
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
    // Map page
    // SAFETY: root_ppn is a valid page table root, fault_addr is page-aligned,
    // phys_addr is a freshly allocated physical page, and pte_flags are well-formed.
    unsafe {
        map_page(root_ppn, fault_addr, phys_addr, pte_flags);

        // Address-specific TLB flush
        crate::arch::cpu::invlpg(fault_addr.bits());
    }

    // Set up reverse mapping for the newly mapped page
    {
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        let page = pfn_to_page_mut(page_pfn);
        if !page.is_null() {
            // SAFETY: page is from pfn_to_page_mut and checked for null.
            // The page was just allocated and mapped, so we have exclusive access.
            unsafe {
                match vma_type {
                    VmaType::Anonymous => {
                        (*page).set_flag(PageFlag::Anonymous);
                        // SwapBacked + LRU membership: required for vmscan
                        // to consider this page for swap-out (see the stack
                        // growth path above).
                        (*page).set_flag(PageFlag::SwapBacked);
                        (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        (*page).inc_mapcount();
                        // Multi-mapping bookkeeping: record (mm, vpn) so
                        // try_to_unmap can find re-mapped instances of this
                        // frame.
                        crate::mm::rmap::page_record_mapping(
                            &*page,
                            addr_space as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                        crate::mm::lru::page_add_anon_lru(&*page);
                    }
                    VmaType::SharedMemory => {
                        (*page).set_flag(PageFlag::Anonymous);
                        (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        (*page).inc_mapcount();
                        // Shmem stays off the anon LRU: try_to_unmap only
                        // walks VmaType::Anonymous VMAs, so marking these
                        // SwapBacked would only add reclaim scan churn.
                        crate::mm::rmap::page_record_mapping(
                            &*page,
                            addr_space as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                    }
                    _ => {
                        // File-backed: rmap not wired yet
                    }
                }
            }
        }
    }
    drop(_pte_guard);

    // RSS accounting: this address space just gained a resident page.
    addr_space.add_rss(1);

    MmFaultResult::Handled
}

// ==================== Utility ====================

/// Physical address behind a user virtual address, if mapped.
///
/// Walks the page table to find the PTE for the given virtual address
/// and returns the physical page address (page-aligned).
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    // SAFETY: caller guarantees root_ppn is a live user root.
    unsafe { PageTableWalker::walk(root_ppn, vaddr).map(|(ppn, _)| ppn << 12) }
}

// ==================== Swap-In Support ====================

/// Read the raw PTE value at a virtual address.
///
/// Walks the four-level page table and returns the raw bits of the leaf
/// PTE, even if P=0 (e.g. a swap entry). Returns None if the walk cannot
/// reach the leaf level.
pub(crate) fn read_pte_raw(root_ppn: u64, vaddr: VirtAddr) -> Option<u64> {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    // SAFETY: root_ppn is a valid root page table PPN. get_page_table_virt
    // returns valid kernel-virtual pointers. Only reads PTEs, including the
    // leaf even if P=0.
    unsafe {
        let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
        let pte4 = (*root_table).get(vpn4);
        if !pte4.is_valid() { return None; }

        let table3 = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
        let pte3 = (*table3).get(vpn3);
        if !pte3.is_valid() { return None; }
        // Superpage leaves (identity/device windows): return the leaf
        // itself — never a swap/migration entry, and its PPN must not be
        // followed as a table pointer.
        if pte3.is_leaf() { return Some(pte3.bits()); }

        let table2 = get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
        let pte2 = (*table2).get(vpn2);
        if !pte2.is_valid() { return None; }
        if pte2.is_leaf() { return Some(pte2.bits()); }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1);

        let raw = pte1.bits();
        if raw == 0 { return None; }

        Some(raw)
    }
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
    addr_space: &AddressSpace,
    fault_addr: VirtAddr,
    flags: u32,
    swap_entry: u64,
    root_ppn: u64,
) -> MmFaultResult {
    use crate::mm::swap;
    use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
    use crate::mm::page::VirtAddr as PageVirtAddr;

    // Extract swap type and offset
    let swap_type = swap::swap_entry_type(swap_entry);
    let swap_offset = swap::swap_entry_offset(swap_entry);

    // Find VMA for permission bits
    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());
    let vma_mgr = addr_space.vma_read();
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
        crate::println!("swap: failed to read page from swap (type={}, offset={})", swap_type, swap_offset);
        // R21-6: free the freshly allocated page on the failed read (was a
        // per-error page leak).
        crate::mm::page_alloc::free_page(phys_addr as usize);
        return MmFaultResult::OutOfMemory;
    }

    // Build PTE flags from VMA permissions
    let mut pte_flags = PageTableEntry::P | PageTableEntry::A | PageTableEntry::D
        | PageTableEntry::NX;
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
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    // Re-check under the lock (same double-fault race as the demand
    // paths). The swap-in page is exclusively owned, so freeing on loss is
    // safe; the swap slot is freed by the winner. E8-MM: device-window-
    // hole rule applies here too (see pte_counts_as_mapped).
    if unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) }
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
    unsafe {
        map_page(root_ppn, fault_addr, PhysAddr::new(phys_addr), pte_flags);

        // TLB flush
        crate::arch::cpu::invlpg(fault_addr.bits());
    }

    // Set up rmap for the swapped-in page
    let page_pfn = (phys_addr >> PAGE_SHIFT) as usize;
    let page = pfn_to_page_mut(page_pfn);
    if !page.is_null() {
        // SAFETY: page is checked for null. The page was just allocated for
        // swap-in and is not yet shared, so exclusive access is guaranteed.
        unsafe {
            (*page).set_flag(PageFlag::Anonymous);
            (*page).set_flag(PageFlag::SwapBacked);
            (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
            (*page).inc_mapcount();
            crate::mm::rmap::page_record_mapping(
                &*page,
                addr_space as *const _ as usize,
                fault_addr.bits() as usize,
            );

            // Add back to anon LRU
            crate::mm::lru::page_add_anon_lru(&*page);
        }
    }
    drop(_pte_guard);

    // Free the swap slot (page is back in memory)
    swap::swap_free_slot(swap_type, swap_offset);

    // RSS accounting: the page is resident again.
    addr_space.add_rss(1);

    MmFaultResult::Handled
}

// Keep the static root reachable for diagnostics (interface parity with
// the riscv64 twin's imports).
#[allow(unused)]
fn _root_table_ref() -> usize {
    // SAFETY: address-of only, no dereference.
    unsafe { &raw mut ROOT_PAGE_TABLE as usize }
}
