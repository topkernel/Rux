//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 page fault handling — stage-2 port of the riscv64 twin.
//!
//! handle_mm_fault drives the VMA layer + the 4-level user walker:
//! demand fill for anonymous/file/shmem VMAs, COW detection, stack
//! expansion, PROT_NONE marker re-presentation (mprotect(prot==0)
//! installed markers re-present under a granting VMA), and the twin's
//! locking protocol (map + rmap setup under one PTE section, R7-C3
//! re-check against CLONE_VM siblings).
//!
//! Still dormant from the twin, with the precise port requirements:
//!
//! - swap-in (swap entries in PTEs, migration entries): no producers
//!   exist on aarch64 — no swap-out path and no swap configuration,
//!   so there is nothing to swap IN yet. Porting means carrying the
//!   twin's V=0-nonzero-bits leaf decode here plus the mm/swap entry
//!   encoding in the walker; it stays dead code until a swap-out
//!   producer (kswapd/vmscan port) exists. Documented, not
//!   implemented.
//!
//! - the MAP_SHARED/MAP_PRIVATE page-cache alias (riscv64's PCACHE
//!   filemap_fault port): file faults take the twin's private-copy
//!   fallback arm (one private copy per mapper, functionally correct).
//!   A real port is a subsystem, not a patch: mapping page-cache frames
//!   directly needs the pin/refcount protocol across fork's
//!   copy_page_table_cow, unmap's release_cache_mapping_pin,
//!   writenotify-style W-diversion for private mappings, and shared
//!   writeback (dirty tracking + msync). cow_flags::PCACHE keeps the
//!   bit budget reserved, and the mprotect/marker-re-present paths
//!   written this stage preserve the bit like COW — so a later port
//!   does not have to touch them again. Deferred.
//!
//! - anonymous fault-around (riscv64 sc181): the twin installs a
//!   forward window of up to 32 zero pages on write faults into VMAs
//!   >= 1MB (cmdline `faaround=0` opt-out). The port is small in
//!   isolation, but it changes RSS/residency accounting on EVERY anon
//!   write fault in large VMAs, and aarch64 has no perf-gate tooling
//!   ported yet (no pbench/syscost/census parity) — an optimization
//!   whose effect cannot be measured here violates the "if it is not
//!   measured, add the DFX tool that measures it" rule, and an
//!   untestable accounting change risks mincore-class regressions
//!   sight unseen. Deferred until the aarch64 DFX tools land; the
//!   twin's anon_faultaround_forward is the reference shape.

use super::memory_layout::*;
use super::mm_ops::{
    alloc_user_phys_page, check_pte_permissions, is_cow_page,
    map_user_page, pte_lock_for, PageTableWalker, PTE_MODIFY_LOCK,
};
use super::pagetable::*;
use crate::mm::page::PAGE_SIZE as PAGE_SIZE_USIZE;
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Fault access type flags (interface parity with the other backends)
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
    /// Bus error — file-backed fault past EOF (SIGBUS to the task)
    BusError,
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

/// Try to expand stack when a page fault occurs below the current stack
/// bottom (on-demand GROWSDOWN expansion; twin contract)
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

    if fault_addr_val < stack_limit {
        return MmFaultResult::Segfault;
    }

    // Find the stack VMA (GROWSDOWN flag)
    let vma_mgr_read = addr_space.vma_read();
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
        let mut vma_mgr_write = addr_space.vma_write();
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
    unsafe {
        match vma_type {
            VmaType::Anonymous
            | VmaType::FileBacked
            | VmaType::SharedMemory => {
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::Device => {
                // Device mapping: don't zero
            }
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
    let _pte_guard = pte_lock_for(addr_space).lock_irqsave();
    // SAFETY: read-only walk under the lock.
    if unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits()) }.is_some() {
        drop(_pte_guard);
        // Lost the race: free our exclusively-owned page; the next entry
        // sees the winner's mapping (Handled, not AlreadyMapped — the
        // trap layer maps AlreadyMapped to SIGSEGV).
        crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
        return MmFaultResult::Handled;
    }
    // SAFETY: root valid, page-aligned address, owned zeroed page.
    unsafe {
        if !map_user_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure: free the data page, deliver OOM
            // (trap layer SIGKILLs the user task; twin policy).
            crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
            return MmFaultResult::OutOfMemory;
        }
    }

    // Reverse mapping for the stack page (VMA lock already dropped).
    {
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        let page = pfn_to_page_mut(page_pfn);
        if !page.is_null() {
            // SAFETY: null-checked; exclusively owned until the install.
            unsafe {
                (*page).set_flag(PageFlag::Anonymous);
                // SwapBacked + LRU membership: reclaim visibility.
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

    addr_space.add_rss(1);

    MmFaultResult::Handled
}

// ==================== PROT_NONE marker re-presentation ====================

/// R-WRITESTORM-2 twin: a fault landed on a PROT_NONE marker leaf
/// (mprotect(prot==0)'s hardware-denied descriptor). The covering VMA
/// is the authority: if it grants the access, re-present the leaf with
/// the VMA's permissions (PPN + COW/PCACHE software bits preserved, W
/// deferred through COW per INV-COW-1); otherwise the access is a real
/// protection violation.
fn protnone_represent(
    addr_space: &AddressSpace,
    root_ppn: u64,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;

    let is_write = flags & FaultFlags::WRITE != 0;
    let is_exec = flags & FaultFlags::EXEC != 0;
    let is_read = flags & FaultFlags::READ != 0;

    // VMA lookup + access grant (riscv64 twin's verify order)
    let vma_mgr = addr_space.vma_read();
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
    let _pte_guard = pte_lock_for(addr_space).lock_irqsave();
    // SAFETY: read-only walk on the mm's root.
    let Some((_ppn, now)) = (unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits()) }) else {
        return MmFaultResult::PermissionDenied;
    };
    if !super::mm_ops::is_protnone_marker(now) {
        // Someone re-presented/unmapped it first: the fresh state rules.
        return MmFaultResult::Handled;
    }
    let cow_bit = now & super::mm_ops::cow_flags::COW;
    let pcache_bit = now & super::mm_ops::cow_flags::PCACHE;
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
            && !super::mm_ops::cow_flags::pte_is_shared_cache(now)
        {
            sw |= super::mm_ops::cow_flags::COW;
            sw &= !PageTableEntry::W;
        }
    }

    // Write the L3 leaf: rebuild preserves the PPN (OUTPUT_ADDR field).
    // SAFETY: root_ppn is this mm's live root; marker re-checked under
    // the section.
    if let Some(l3) = (unsafe { super::mmu_init::user_l3_table_for(root_ppn, fault_addr.bits()) }) {
        let idx = ((fault_addr.bits() >> PAGE_SHIFT) & 0x1FF) as usize;
        // SAFETY: l3 from the walker; idx masked to 9 bits.
        unsafe { (*l3).set(idx, PageTableEntry::rebuild_user_leaf(now, sw)) };
        // SAFETY: this translation changed; drop any cached walk.
        unsafe { super::asid::flush_tlb_page(fault_addr.bits() as usize, 0) };
        return MmFaultResult::Handled;
    }
    MmFaultResult::PermissionDenied
}

// ==================== Main Fault Handler ====================

/// handle_mm_fault - handle a user-mode page fault (twin contract):
/// 1. Find the VMA to validate the address and permissions
/// 2. Check whether the page is already mapped (COW / permission arms)
/// 3. If unmapped, allocate a new page (zero anonymous / file read /
///    shared-memory zero)
/// 4. Map it with the VMA's permissions under the PTE section, with the
///    R7-C3 re-check
/// 5. Set up rmap/LRU/RSS bookkeeping
pub fn handle_mm_fault(
    addr_space: &AddressSpace,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;

    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());
    let root_ppn = addr_space.root_ppn();

    // Already-mapped check (lock-free; the PROT_NONE marker arm runs in
    // the branch below — markers count as mapped leaves).
    // SAFETY: read-only walk on the mm's root.
    let already_mapped = unsafe {
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
        if let Some((_ppn, raw)) = unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits()) } {
            if super::mm_ops::is_protnone_marker(raw) {
                return protnone_represent(addr_space, root_ppn, fault_addr, flags);
            }
        }

        // COW check first (write fault on an RO+COW leaf)
        // SAFETY: read-only walk.
        if is_write && unsafe { is_cow_page(root_ppn, fault_addr) } {
            crate::pr_debug!("pagefault: cow at {:#x}", fault_addr.bits());
            return MmFaultResult::CowPending;
        }

        // Permission check against the leaf's software bits
        // SAFETY: read-only walk.
        if let Some((has_read, has_write, has_exec, pte_is_user)) =
            unsafe { check_pte_permissions(root_ppn, fault_addr) }
        {
            let perm_ok = (!is_write || has_write)
                && (!is_read || has_read)
                && (!is_exec || has_exec)
                && (!is_user || pte_is_user);
            if perm_ok {
                // Permissions fine (e.g. AF-clear style refault): flush
                // and retry.
                // SAFETY: translation unchanged; drop any stale entry.
                unsafe { super::asid::flush_tlb_page(fault_addr.bits() as usize, 0) };
                return MmFaultResult::Handled;
            }
        }
        return MmFaultResult::PermissionDenied;
    }

    // Find the VMA
    let vma_mgr = addr_space.vma_read();
    let vma = match vma_mgr.find(page_virt_addr) {
        Some(v) => v,
        None => {
            drop(vma_mgr);
            return try_expand_stack(addr_space, fault_addr, flags, root_ppn);
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
        addr_space.get_vma_file(vma_start)
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
            if let Some(live) = crate::fs::devfs::devfs_reg_current_size(f.as_ref()) {
                return Some(live as usize);
            }
            // SAFETY: inode cell written at open time; read-only.
            let inode_opt = unsafe { &*f.inode.get() };
            inode_opt.as_ref().map(|i| i.get_size() as usize)
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
    // SAFETY: exclusively owned fresh page inside the linear map.
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;
    unsafe {
        match vma_type {
            VmaType::Anonymous | VmaType::SharedMemory => {
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::FileBacked => {
                // Zero-fill first (partial reads / tail holes), then read
                // the file data. Bounds use the CURRENT inode size (a
                // file grown by write(2) after mmap must serve its new
                // pages — twin mmapstress04 note); positional read_at so
                // the shared file position is never touched (twin R71).
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
                if vma_file_fd >= 0 && file_offset < effective_size {
                    // Prefer the VMA's pinned vm_file (the mapping fd may
                    // already be closed — twin ld.so note).
                    let file = vm_file.clone().or_else(|| {
                        if vma_file_fd >= 0 {
                            // SAFETY: fd-table lookup.
                            unsafe { crate::fs::get_file_fd(vma_file_fd as usize) }
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
                        let bytes_read = unsafe {
                            file.read_at(
                                file_offset as u64,
                                page_ptr,
                                bytes_to_read,
                            )
                        };
                        if bytes_read > 0 && (bytes_read as usize) < PAGE_SIZE_USIZE {
                            core::ptr::write_bytes(
                                page_ptr.add(bytes_read as usize),
                                0,
                                PAGE_SIZE_USIZE - bytes_read as usize,
                            );
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
    let _pte_guard = pte_lock_for(addr_space).lock_irqsave();
    // SAFETY: read-only walk under the lock.
    if unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits()) }.is_some() {
        drop(_pte_guard);
        // Lost the race: free our exclusively-owned page; retry sees the
        // winner's mapping (Handled — never AlreadyMapped, which the trap
        // layer maps to SIGSEGV).
        crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
        return MmFaultResult::Handled;
    }
    // SAFETY: valid root, page-aligned fault address, owned filled page.
    unsafe {
        if !map_user_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure: release the data page, deliver OOM.
            crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
            return MmFaultResult::OutOfMemory;
        }
    }

    // Reverse mapping + LRU for the newly mapped page (twin arms).
    {
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
        let page_pfn = (phys_addr.bits() >> PAGE_SHIFT) as usize;
        let page = pfn_to_page_mut(page_pfn);
        if !page.is_null() {
            // SAFETY: null-checked; frame exclusively owned until the
            // install above.
            unsafe {
                match vma_type {
                    VmaType::Anonymous => {
                        (*page).set_flag(PageFlag::Anonymous);
                        (*page).set_flag(PageFlag::SwapBacked);
                        (*page).set_index(
                            fault_addr.bits() as usize / (PAGE_SIZE as usize),
                        );
                        (*page).inc_mapcount();
                        crate::mm::rmap::page_record_mapping(
                            &*page,
                            addr_space as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                        crate::mm::lru::page_add_anon_lru(&*page);
                    }
                    VmaType::SharedMemory => {
                        (*page).set_flag(PageFlag::Anonymous);
                        (*page).set_index(
                            fault_addr.bits() as usize / (PAGE_SIZE as usize),
                        );
                        (*page).inc_mapcount();
                        // Shmem stays off the anon LRU (twin note:
                        // try_to_unmap only walks Anonymous VMAs).
                        crate::mm::rmap::page_record_mapping(
                            &*page,
                            addr_space as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                    }
                    _ => {
                        // Private file copy (the twin's do_cow_fault
                        // parity arm): an exclusive copy IS an anonymous
                        // page from reclaim's point of view. Device
                        // frames keep no state.
                        if matches!(vma_type, VmaType::FileBacked) {
                            (*page).set_flag(PageFlag::Anonymous);
                            (*page).set_flag(PageFlag::SwapBacked);
                            (*page).set_index(
                                fault_addr.bits() as usize / (PAGE_SIZE as usize),
                            );
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
            }
        }
    }
    drop(_pte_guard);

    // RSS accounting: this address space gained a resident page.
    addr_space.add_rss(1);

    MmFaultResult::Handled
}

// ==================== Utility ====================

/// Get the physical address mapped at a user virtual address
/// (diagnostics; 4-level user-table walk, block leaves folded)
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    // SAFETY: read-only walk on a valid root.
    unsafe { PageTableWalker::walk(root_ppn, vaddr).map(|(ppn, _)| ppn << PAGE_SHIFT) }
}
