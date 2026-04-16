//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Page Fault Handling
//!
//! This module contains:
//! - Fault type flags
//! - Page fault handling result types
//! - handle_mm_fault() implementation
//! - Stack expansion support

use core::arch::asm;

use super::memory_layout::*;
use super::mmu_init::{map_page, get_page_table_virt, ROOT_PAGE_TABLE};
use super::mm_ops::{alloc_user_phys_page, is_cow_page, check_pte_permissions, PageTableWalker};
use super::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Page fault type flags
pub struct FaultFlags;

impl FaultFlags {
    /// Read fault
    pub const READ: u32 = 0x01;
    /// Write fault
    pub const WRITE: u32 = 0x02;
    /// Execute fault (instruction fetch)
    pub const EXEC: u32 = 0x04;
    /// User mode access
    pub const USER: u32 = 0x08;
    /// Kernel mode access
    pub const KERNEL: u32 = 0x10;
}

// ==================== Fault Result ====================

/// Page fault handling result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Handled successfully, can retry instruction
    Handled,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Permission denied (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Already mapped (no handling needed)
    AlreadyMapped,
    /// COW pending (handled by handle_cow_fault)
    CowPending,
    /// Kernel exception fixed (via exception table)
    Fixed,
    /// Unfixable kernel exception
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
    use crate::mm::vma::VmaFlags;

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

    // Build page table entry flags
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U; // User page

    if vma_flags.is_readable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }

    // Map page
    // SAFETY: root_ppn is a valid page table root, fault_addr is page-aligned,
    // phys_addr is a freshly allocated physical page, and pte_flags are well-formed.
    unsafe {
        map_page(root_ppn, fault_addr, phys_addr, pte_flags);

        // Address-specific TLB flush
        let vaddr = fault_addr.bits();
        core::arch::asm!(
            "fence",
            "sfence.vma {0}, zero",
            "fence",
            in(reg) vaddr,
            options(nostack, preserves_flags)
        );
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
                (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                (*page).inc_mapcount();
            }
        }
    }

    MmFaultResult::Handled
}

// ==================== Main Fault Handler ====================

/// handle_mm_fault - Handle user mode page fault
///
/// # Arguments
/// - `addr_space`: Address space
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
        PageTableWalker::walk(root_ppn, fault_addr.bits() as u64).is_some()
    };

    // If not mapped, check for a swap entry in the PTE (V=0 but non-zero bits)
    if !already_mapped {
        if let Some(swap_entry) = read_pte_raw(root_ppn, fault_addr) {
            if crate::mm::swap::is_swap_entry(swap_entry) {
                crate::pr_debug!("pagefault: swap-in at {:#x}", fault_addr.bits());
                return handle_swap_fault(addr_space, fault_addr, flags, swap_entry, root_ppn);
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
            unsafe { check_pte_permissions(root_ppn, fault_addr) } {
            // Verify permissions
            let perm_ok = (!is_write || has_write)
                && (!is_read || has_read)
                && (!is_exec || has_exec)
                && (!is_user || pte_is_user);

            if perm_ok {
                // Permissions correct, flush TLB
                // SAFETY: sfence.vma with a specific virtual address invalidates TLB entries
                // for that address. The fence instructions ensure memory ordering.
                unsafe {
                    let vaddr = fault_addr.bits();
                    core::arch::asm!(
                        "fence",
                        "sfence.vma {0}, zero",
                        "fence",
                        in(reg) vaddr,
                        options(nostack, preserves_flags)
                    );
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

    crate::pr_debug!("pagefault: map new page at {:#x}, type={:?}", fault_addr.bits(), vma_type);

    // Release read lock
    drop(vma_mgr);

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
                if vma_file_fd >= 0 {
                    if let Some(aspace) = crate::sched::current().and_then(|t| t.address_space()) {
                        if let Some(found_vma) = aspace.vma_read().find(page_virt_addr) {
                            let vma_start = found_vma.start().as_usize();
                            let page_offset_in_mapping = page_virt_addr.as_usize() - vma_start;
                            let file_offset = vma_offset + page_offset_in_mapping;

                            // Read from file if within file bounds
                            if file_offset < vma_file_size as usize {
                                if let Some(file) = crate::fs::get_file_fd(vma_file_fd as usize) {
                                    let saved_pos = file.get_pos();
                                    file.set_pos(file_offset as u64);

                                    let bytes_to_read = core::cmp::min(
                                        PAGE_SIZE_USIZE,
                                        (vma_file_size as usize).saturating_sub(file_offset),
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

    // Build page table entry flags
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U; // User page

    if vma_flags.is_readable() {
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

    // Map page
    // SAFETY: root_ppn is a valid page table root, fault_addr is page-aligned,
    // phys_addr is a freshly allocated physical page, and pte_flags are well-formed.
    unsafe {
        map_page(root_ppn, fault_addr, phys_addr, pte_flags);

        // Address-specific TLB flush
        let vaddr = fault_addr.bits();
        core::arch::asm!(
            "fence",
            "sfence.vma {0}, zero",
            "fence",
            in(reg) vaddr,
            options(nostack, preserves_flags)
        );
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
                    VmaType::Anonymous | VmaType::SharedMemory => {
                        (*page).set_flag(PageFlag::Anonymous);
                        (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        (*page).inc_mapcount();
                    }
                    _ => {
                        // File-backed: rmap not wired yet
                    }
                }
            }
        }
    }

    MmFaultResult::Handled
}

// ==================== Utility ====================

/// Get the physical address mapped at a user virtual address.
///
/// Walks the page table to find the PTE for the given virtual address
/// and returns the physical page address (page-aligned).
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    use super::PAGE_SHIFT;

    let vpn2 = (vaddr >> 30) & 0x1FF;
    let vpn1 = (vaddr >> 21) & 0x1FF;
    let vpn0 = (vaddr >> 12) & 0x1FF;

    // SAFETY: root_ppn is a valid root page table PPN. get_page_table_virt returns
    // a valid kernel-virtual pointer to each page table level. Only reads PTEs.
    unsafe {
        let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
        let pte2 = (*root_table).get(vpn2 as usize);
        if !pte2.is_valid() { return None; }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1 as usize);
        if !pte1.is_valid() { return None; }

        let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
        let pte0 = (*table0).get(vpn0 as usize);
        if !pte0.is_valid() { return None; }

        Some((pte0.ppn() as u64) << PAGE_SHIFT)
    }
}

// ==================== Swap-In Support ====================

/// Read the raw PTE value at a virtual address.
///
/// Walks the three-level page table and returns the raw bits of the
/// leaf PTE, even if V=0 (e.g. a swap entry).  Returns None if the
/// page table walk cannot reach the leaf level.
fn read_pte_raw(root_ppn: u64, vaddr: VirtAddr) -> Option<u64> {
    use super::PAGE_SHIFT;

    let vpn2 = (vaddr.bits() >> 30) & 0x1FF;
    let vpn1 = (vaddr.bits() >> 21) & 0x1FF;
    let vpn0 = (vaddr.bits() >> 12) & 0x1FF;

    // SAFETY: root_ppn is a valid root page table PPN. get_page_table_virt returns
    // valid kernel-virtual pointers. Only reads PTEs, including the leaf even if V=0.
    unsafe {
        let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
        let pte2 = (*root_table).get(vpn2 as usize);
        if !pte2.is_valid() { return None; }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1 as usize);
        if !pte1.is_valid() { return None; }

        let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
        let pte0 = (*table0).get(vpn0 as usize);

        let raw = pte0.bits();
        if raw == 0 { return None; }

        Some(raw)
    }
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

    // Read page contents from swap device
    if swap::swap_read_page(swap_type, swap_offset, phys_addr as usize).is_err() {
        crate::println!("swap: failed to read page from swap (type={}, offset={})", swap_type, swap_offset);
        return MmFaultResult::OutOfMemory;
    }

    // Build PTE flags from VMA permissions
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U;

    if vma_flags.is_readable() {
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
    unsafe {
        map_page(root_ppn, fault_addr, PhysAddr::new(phys_addr), pte_flags);

        // TLB flush
        let vaddr = fault_addr.bits();
        core::arch::asm!(
            "fence",
            "sfence.vma {0}, zero",
            "fence",
            in(reg) vaddr,
            options(nostack, preserves_flags)
        );
    }

    // Set up rmap for the swapped-in page
    let page_pfn = (phys_addr >> super::PAGE_SHIFT) as usize;
    let page = pfn_to_page_mut(page_pfn);
    if !page.is_null() {
        // SAFETY: page is checked for null. The page was just allocated for swap-in
        // and is not yet shared, so exclusive access is guaranteed.
        unsafe {
            (*page).set_flag(PageFlag::Anonymous);
            (*page).set_flag(PageFlag::SwapBacked);
            (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
            (*page).inc_mapcount();

            // Add back to anon LRU
            crate::mm::lru::page_add_anon_lru(&*page);
        }
    }

    // Free the swap slot (page is back in memory)
    swap::swap_free_slot(swap_type, swap_offset);

    MmFaultResult::Handled
}
