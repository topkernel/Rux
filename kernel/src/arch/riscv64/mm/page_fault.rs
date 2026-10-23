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
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    // R7-C3: re-check under the lock. The already_mapped walk at entry was
    // lock-free; two threads sharing this mm (CLONE_VM) can both see "not
    // mapped", both allocate+zero (+read for file VMAs), and the second
    // map_page would orphan the first page (refcount 1, mapcount 0,
    // unreclaimable) and silently discard stores landed in it.
    // E8-MM: same device-window-hole rule as the entry walk (see
    // pte_counts_as_mapped) — a restored kernel PTE must not be mistaken
    // for the winner's mapping here, or this path "Handles" forever
    // against it while the user instruction keeps refaulting.
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

    // RSS accounting (review 4.14): stack page became resident.
    addr_space.add_rss(1);

    MmFaultResult::Handled
}

// ==================== filemap_fault (page-cache mapping, riscv64 port) ====================

/// Pin the page-cache frame for one page-shaped file offset, populating
/// the cache on a miss through the ext4 read machinery (the fs-side
/// readahead/fault-around detector warms the surrounding window in the
/// same device round trips a read_at would have used). x86_64 twin (the
/// session-23 fake-OOM fix); the riscv64 MAP_PRIVATE arm adopts the same
/// protocol so file demand faults stop allocating one unreclaimable
/// private copy per fault (the GNOME-image fake-OOM family).
///
/// Returns the PINNED physical address for direct mapping. The pin (plus
/// the descriptor `get_page` the map path takes) is owned by the PTE for
/// its mapping lifetime — see `cow_flags::PCACHE`.
fn filemap_pin_cache_page(
    file: &crate::fs::file::File,
    file_offset: usize,
    effective_size: usize,
    hole_as_zero: bool,
) -> Option<u64> {
    if file_offset >= effective_size {
        return None;
    }
    if !crate::fs::ext4::file::is_ext4_file(file) {
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
    // back to a private zero copy — mapper incoherence, nothing persists:
    // insert an explicit ZERO page into the cache and alias THAT (x86_64
    // twin). The flush path allocates the on-disk block at msync/fsync
    // time. Private arms keep the private-copy fill.
    let populated =
        crate::fs::ext4::file::ext4_fault_populate_cache(file, file_offset as u64);
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
    // R-WRITESTORM-2 (x86_64 twin): a V|U leaf with NO R/W/X bits is a
    // PROT_NONE marker from mprotect(prot==0) (Sv39 keeps V set). Counting
    // it as "already mapped" sent the fault into the permission check,
    // which sees no R/W/X and kills the task without consulting the
    // covering VMA. Markers now fall through to the VMA layer, which
    // re-presents the leaf when the VMA grants the access.
    let already_mapped = unsafe {
        PageTableWalker::walk(root_ppn, fault_addr.bits() as u64).map_or(false, |(_ppn, bits)| {
            !(pte_counts_as_mapped(bits, fault_addr.bits() as u64)
              && (bits & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X)) == 0
              && (bits & PageTableEntry::V) != 0)
        })
    };

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
    // sc181: bounds for the anonymous fault-around window (see the tail
    // and anon_faultaround_forward).
    let vma_start = vma.start().as_usize();
    let vma_end = vma.end().as_usize();

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
                let pcache_bit = raw & super::mm_ops::cow_flags::PCACHE;
                if pcache_bit != 0 {
                    flags |= pcache_bit;
                    if flags & PageTableEntry::W != 0
                        && !super::mm_ops::cow_flags::pte_is_shared_cache(raw)
                    {
                        flags |= 1 << 8; // force COW
                        flags &= !PageTableEntry::W;
                    }
                }
                let _pte_guard =
                    crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
                if let Some(now) = read_pte_raw(root_ppn, page_base) {
                    if is_protnone_marker(now) {
                        let new_pte = (now & !0x3FF) | flags;
                        write_pte_raw(root_ppn, page_base, new_pte);
                        // SAFETY: sfence.vma for this address; kernel ctx.
                        unsafe {
                            let va = page_base.bits();
                            core::arch::asm!(
                                "fence",
                                "sfence.vma {0}, zero",
                                "fence",
                                in(reg) va,
                                options(nostack, preserves_flags)
                            );
                        }
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
            let file = addr_space
                .get_vma_file(
                    addr_space
                        .vma_read()
                        .find(page_virt_addr)
                        .map(|v| v.start().as_usize())
                        .unwrap_or(0),
                )
                .or_else(|| {
                    if vma_file_fd >= 0 {
                        unsafe { crate::fs::get_file_fd(vma_file_fd as usize) }
                    } else {
                        None
                    }
                });
            let vma_start = addr_space
                .vma_read()
                .find(page_virt_addr)
                .map(|v| v.start().as_usize());
            match (file, vma_start) {
                (Some(f), Some(vs)) => {
                    let file_offset = vma_offset + (page_virt_addr.as_usize() - vs);
                    if crate::fs::ext4::file::is_ext4_file(f.as_ref()) {
                        // Effective size: CURRENT inode size (a file grown
                        // by write(2) after the mmap must serve its new
                        // pages), the mmap-time snapshot as fallback —
                        // same rule as the private arm.
                        let effective_size = unsafe { &*f.inode.get() }
                            .as_ref()
                            .map(|i| i.get_size())
                            .unwrap_or(vma_file_size) as usize;
                        match filemap_pin_cache_page(f.as_ref(), file_offset, effective_size, true)
                        {
                            Some(phys) => {
                                // Conservative v1 dirty mark for a W
                                // mapping (x86_64 twin): the store may
                                // never come; flushing an unwritten page
                                // is harmless I/O.
                                if vma_flags.is_writable() {
                                    if let Some(inode) =
                                        unsafe { &*f.inode.get() }.as_ref()
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
                _ => (None, false),
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
        if let Some(aspace) = crate::sched::current().and_then(|t| t.address_space()) {
            // R73: scope the VMA read guard to the lookup itself; the file
            // pin table and the populate read below stay OUTSIDE it (the
            // pin may do file I/O through the ext4 machinery — never hold
            // a guard across it; same shape as the fill arm below and the
            // x86_64 twin).
            let vma_start_opt = {
                let vmas = aspace.vma_read();
                vmas.find(page_virt_addr).map(|v| v.start().as_usize())
            };
            if let Some(vs) = vma_start_opt {
                let file = aspace.get_vma_file(vs);
                // Effective size: CURRENT inode size (a file grown by
                // write(2) after the mmap must serve its new pages), the
                // mmap-time snapshot as fallback — same rule as the fill
                // arm below.
                let effective_size = file
                    .as_ref()
                    .and_then(|f| {
                        // SAFETY: inode cell written at open time; read-only.
                        let inode_opt = unsafe { &*f.inode.get() };
                        inode_opt.as_ref().map(|i| i.get_size())
                    })
                    .unwrap_or(vma_file_size) as usize;
                if let Some(f) = file {
                    let file_offset = vma_offset + (page_virt_addr.as_usize() - vs);
                    filemap_pin_cache_page(&f, file_offset, effective_size, false)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
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
                // snapshot stays only as a fallback: LTP mmapstress04
                // mapped a 1-page file, grew it to 384 pages, and read
                // stale zeros through the snapshot.)
                if vma_file_fd >= 0 {
                    if let Some(aspace) = crate::sched::current().and_then(|t| t.address_space()) {
                        // R73: scope the VMA read guard to the lookup
                        // itself. The guard is a spinlock rwlock read
                        // (preempt-disable while held); keeping it alive
                        // across file.read_at() below meant every
                        // demand-fault file read ran with preempt_count
                        // != 0 — the EXT4_BIG_LOCK waiter then refused
                        // its sleep path (lock_fair can_sleep) and
                        // pure-spinned, pinching all 4 CPUs while the
                        // lock holder waited for block I/O (GNOME
                        // r72-fire2 wedge: 4 CPUs in lock_fair, gs-b
                        // starved, screen frozen at M4).
                        let vma_start_opt = {
                            let vmas = aspace.vma_read();
                            vmas.find(page_virt_addr).map(|v| v.start().as_usize())
                        };
                        if let Some(vma_start) = vma_start_opt {
                            let page_offset_in_mapping = page_virt_addr.as_usize() - vma_start;
                            let file_offset = vma_offset + page_offset_in_mapping;

                            // Effective size: CURRENT inode size (grown or
                            // truncated since mmap), mmap snapshot as
                            // fallback.
                            let effective_size = aspace
                                .get_vma_file(vma_start)
                                .and_then(|f| {
                                    // SAFETY: inode cell written at open time; read-only.
                                    let inode_opt = unsafe { &*f.inode.get() };
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
                                    .or_else(|| unsafe { crate::fs::get_file_fd(vma_file_fd as usize) });
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
                                    let bytes_read = unsafe {
                                        file.read_at(
                                            file_offset as u64,
                                            page_ptr,
                                            bytes_to_read,
                                        )
                                    };

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
        pte_flags |= super::mm_ops::cow_flags::COW;
        pte_flags |= super::mm_ops::cow_flags::PCACHE;
    }
    if shared_pcache {
        // SHARED cache leaf (session 25): W stays granted IN PLACE —
        // stores land directly in the cache frame (mapper coherence).
        // The PCACHE bit records the PTE-owned pin (released at unmap).
        // Deliberately NO COW bit: PCACHE-without-COW is exactly how the
        // mprotect/marker-represent sites recognize a shared leaf and
        // keep granting W in place (a COW divert here would detach this
        // mapping from the file's shared state — see pte_is_shared_cache).
        pte_flags |= super::mm_ops::cow_flags::PCACHE;
    }

    // Map page under the PTE-modify lock and keep it held across the rmap
    // setup: a concurrent fork's copy_page_table_cow walk landing between
    // the map and the rmap/refcount update takes a reference the rmap
    // never sees (round 6 MED — demand-fault PTE lock).
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    // R7-C3: re-check under the lock. The already_mapped walk at entry was
    // lock-free; two threads sharing this mm (CLONE_VM) can both see "not
    // mapped", both allocate+zero (+read for file VMAs), and the second
    // map_page would orphan the first page (refcount 1, mapcount 0,
    // unreclaimable) and silently discard stores landed in it.
    // E8-MM: same device-window-hole rule as the entry walk (see
    // pte_counts_as_mapped) — a restored kernel PTE must not be mistaken
    // for the winner's mapping here, or this path "Handles" forever
    // against it while the user instruction keeps refaulting.
    if unsafe { PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) }
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
            super::mm_ops::release_cache_mapping_pin(
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
    // Map page
    // SAFETY: root_ppn is a valid page table root, fault_addr is page-aligned,
    // phys_addr is a freshly allocated physical page, and pte_flags are well-formed.
    unsafe {
        if !map_page(root_ppn, fault_addr, phys_addr, pte_flags) {
            // PT allocation failure (fake-OOM family): release the data
            // page, deliver OOM (trap layer SIGKILLs the user task). A
            // private pin or SHARED PCACHE pin is returned via put_by_key;
            // only the LEGACY alias's pin is permanent (its frame stays
            // cache-owned).
            if private_cache_phys.is_some() || shared_pcache {
                super::mm_ops::release_cache_mapping_pin(
                    (phys_addr.bits() >> PAGE_SHIFT) as usize,
                );
            } else if shared_alias_phys.is_none() {
                crate::mm::page_alloc::free_page(phys_addr.bits() as usize);
            }
            return MmFaultResult::OutOfMemory;
        }

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
                // MAP_SHARED page-cache alias: the frame is owned by the
                // page cache (and possibly other mappers) — take one page
                // reference for THIS PTE mapping so the teardown walkers'
                // put_page() stays balanced, and skip the private-fill
                // text-stamp machinery (stamps assume an exclusive
                // file-fill page; a second mapper would trip FILL-DOUBLE
                // forensics, and the stamp adjudication must never decide
                // the fate of a cache-owned frame).
                if shared_alias_phys.is_some() {
                    (*page).get_page();
                    (*page).inc_mapcount();
                    if !shared_pcache {
                        // Legacy non-ext4 alias only. A PCACHE leaf takes
                        // NO rmap record: the descriptor's mapping/index
                        // are the page cache's eviction keys and rmap
                        // state would corrupt eviction (same discipline
                        // as the private cache arm; this retires the
                        // session-24 "unconditional page_record_mapping
                        // on a cache frame" hazard for ext4).
                        crate::mm::rmap::page_record_mapping(
                            unsafe { &*page },
                            addr_space as *const _ as usize,
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
                    (*page).get_page();
                    (*page).inc_mapcount();
                } else {
                match vma_type {
                    VmaType::Anonymous => {
                        (*page).set_flag(PageFlag::Anonymous);
                        // SwapBacked + LRU membership: required for vmscan
                        // to consider this page for swap-out (see the stack
                        // growth path above).
                        (*page).set_flag(PageFlag::SwapBacked);
                        (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        (*page).inc_mapcount();
                        // Multi-mapping bookkeeping (review 4.10): record
                        // (mm, vpn) so try_to_unmap can find re-mapped
                        // instances of this frame.
                        crate::mm::rmap::page_record_mapping(
                            unsafe { &*page },
                            addr_space as *const _ as usize,
                            fault_addr.bits() as usize,
                        );
                        crate::mm::lru::page_add_anon_lru(unsafe { &*page });
                    }
                    VmaType::SharedMemory => {
                        (*page).set_flag(PageFlag::Anonymous);
                        (*page).set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                        (*page).inc_mapcount();
                        // Shmem stays off the anon LRU: try_to_unmap only
                        // walks VmaType::Anonymous VMAs, so marking these
                        // SwapBacked would only add reclaim scan churn.
                        crate::mm::rmap::page_record_mapping(
                            unsafe { &*page },
                            addr_space as *const _ as usize,
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
                            (*page).set_flag(PageFlag::Anonymous);
                            (*page).set_flag(PageFlag::SwapBacked);
                            (*page).set_index(
                                fault_addr.bits() as usize / (PAGE_SIZE as usize),
                            );
                            (*page).inc_mapcount();
                            crate::mm::rmap::page_record_mapping(
                                unsafe { &*page },
                                addr_space as *const _ as usize,
                                fault_addr.bits() as usize,
                            );
                            crate::mm::lru::page_add_anon_lru(unsafe { &*page });
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
    }
    drop(_pte_guard);

    // RSS accounting (review 4.14): this address space just gained a
    // resident page.
    addr_space.add_rss(1);

    // sc181: forward fault-around for large anonymous VMAs (write faults
    // only — see anon_faultaround_forward).
    if vma_type == VmaType::Anonymous
        && is_write
        && !vma_flags.is_shared()
        && vma_end - vma_start >= ANON_FAULTAROUND_MIN_VMA
    {
        anon_faultaround_forward(
            addr_space,
            root_ppn,
            (fault_addr.as_usize() | 0xFFF) + 1,
            vma_end,
            pte_flags,
        );
    }

    MmFaultResult::Handled
}

// ==================== Anonymous Fault-Around (sc181) ====================

/// sc181: forward fault-around window (pages) for anonymous write faults.
const ANON_FAULTAROUND_PAGES: usize = 32;

/// sc181: minimum VMA size that gets fault-around (small stacks/brk keep
/// exact one-page semantics).
const ANON_FAULTAROUND_MIN_VMA: usize = 1024 * 1024;

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
    addr_space: &AddressSpace,
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
        if unsafe { PageTableWalker::walk(root_ppn, va as u64) }
            .map_or(false, |(_p, bits)| pte_counts_as_mapped(bits, va as u64))
        {
            return;
        }
        let Some(phys) = alloc_user_phys_page() else {
            return; // pressure: the window is best-effort
        };
        // SAFETY: freshly allocated page, exclusively owned here.
        let page_ptr = phys_to_virt(PhysAddr::new(phys)).bits() as *mut u8;
        unsafe { core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE) };

        let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
        // R7-C3 twin: re-check under the lock — a CLONE_VM sibling may
        // have installed the page between the lock-free walk and here.
        if unsafe { PageTableWalker::walk(root_ppn, va as u64) }
            .map_or(false, |(_p, bits)| pte_counts_as_mapped(bits, va as u64))
        {
            drop(_pte_guard);
            crate::mm::page_alloc::free_page(phys as usize);
            return;
        }
        // SAFETY: root_ppn valid, va page-aligned, phys freshly allocated.
        if unsafe { !map_page(root_ppn, VirtAddr::new(va as u64), PhysAddr::new(phys), pte_flags) }
        {
            drop(_pte_guard);
            crate::mm::page_alloc::free_page(phys as usize);
            return; // page-table allocation trouble — stop the window
        }
        // Address-specific TLB flush (same discipline as the single-page
        // path).
        unsafe {
            core::arch::asm!(
                "fence",
                "sfence.vma {0}, zero",
                "fence",
                in(reg) va,
                options(nostack, preserves_flags)
            );
        }
        // Bookkeeping: the Anonymous arm of the single-page path.
        {
            use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
            let page_pfn = (phys >> PAGE_SHIFT) as usize;
            let page = pfn_to_page_mut(page_pfn);
            if !page.is_null() {
                // SAFETY: page from pfn_to_page_mut, null-checked; the
                // frame is exclusively owned until the PTE install above.
                unsafe {
                    (*page).set_flag(PageFlag::Anonymous);
                    (*page).set_flag(PageFlag::SwapBacked);
                    (*page).set_index(va / PAGE_SIZE_USIZE);
                    (*page).inc_mapcount();
                    crate::mm::rmap::page_record_mapping(
                        unsafe { &*page },
                        addr_space as *const _ as usize,
                        va,
                    );
                    crate::mm::lru::page_add_anon_lru(unsafe { &*page });
                }
            }
        }
        drop(_pte_guard);
        addr_space.add_rss(1);
        va += PAGE_SIZE_USIZE;
    }
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
        // Superpage leaves: compute the 4KB frame inside the leaf (PERF:
        // MMIO windows map as 2MB L1 leaves; never follow a leaf PPN as a
        // table pointer).
        if pte2.is_leaf() {
            // 1GB leaf: PPN[2] (bits 53:28) is the 1GB frame number.
            let ppn2 = (pte2.bits() >> 28) & 0x3FF_FFFF;
            return Some((ppn2 << 30) | (vaddr & 0x3FFF_FFFF & !0xFFF));
        }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1 as usize);
        if !pte1.is_valid() { return None; }
        if pte1.is_leaf() {
            return Some((pte1.ppn_for_2mb_page() << 21) | (vaddr & 0x1F_FFFF & !0xFFF));
        }

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

/// R-WRITESTORM-2: write one 4K leaf PTE (raw bits), same 3-level walk as
/// read_pte_raw. Caller must hold PTE_MODIFY_LOCK and have resolved the
/// leaf via read_pte_raw first (upper levels exist).
pub(crate) fn write_pte_raw(root_ppn: u64, vaddr: VirtAddr, bits: u64) {
    use super::PAGE_SHIFT;
    let vpn2 = ((vaddr.bits() >> 30) & 0x1FF) as usize;
    let vpn1 = ((vaddr.bits() >> 21) & 0x1FF) as usize;
    let vpn0 = ((vaddr.bits() >> 12) & 0x1FF) as usize;
    // SAFETY: same walk as read_pte_raw; the leaf exists (the caller just
    // read it), so intermediate tables are present. Only the leaf changes.
    unsafe {
        let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() { return; }
        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() || pte1.is_leaf() { return; }
        let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
        (*table0).set(vpn0, PageTableEntry::from_bits(bits));
    }
}

pub(crate) fn read_pte_raw(root_ppn: u64, vaddr: VirtAddr) -> Option<u64> {
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
        // Superpage leaves (PERF: 2MB MMIO L1 leaves / 1GB L2 framebuffer):
        // return the leaf itself — never a swap/migration entry, and its
        // PPN must not be followed as a table pointer.
        if pte2.is_leaf() { return Some(pte2.bits()); }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1 as usize);
        if !pte1.is_valid() { return None; }
        if pte1.is_leaf() { return Some(pte1.bits()); }

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
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    // R7-C3: re-check under the lock (same double-fault race as the demand
    // paths). The swap-in page is exclusively owned, so freeing on loss is
    // safe; the swap slot is freed by the winner.
    // E8-MM: device-window-hole rule applies here too (see
    // pte_counts_as_mapped).
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
    unsafe {
        if !map_page(root_ppn, fault_addr, PhysAddr::new(phys_addr), pte_flags) {
            // PT allocation failure (fake-OOM family): free the swap-in
            // page, deliver OOM.
            crate::mm::page_alloc::free_page(phys_addr as usize);
            return MmFaultResult::OutOfMemory;
        }

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

    // RSS accounting (review 4.14): the page is resident again.
    addr_space.add_rss(1);

    MmFaultResult::Handled
}
