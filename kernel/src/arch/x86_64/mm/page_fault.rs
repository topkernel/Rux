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
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
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
unsafe fn fill_file_page(
    file: &crate::fs::file::File,
    file_offset: usize,
    page_ptr: *mut u8,
    effective_size: usize,
) {
    let will_full_read =
        file_offset < effective_size && effective_size - file_offset >= PAGE_SIZE_USIZE;
    if !will_full_read {
        core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
    }
    if file_offset >= effective_size {
        return; // within VMA but past EOF: stays zero (sparse/hole)
    }
    let bytes_to_read = core::cmp::min(PAGE_SIZE_USIZE, effective_size - file_offset);
    let bytes_read = file.read_at(file_offset as u64, page_ptr, bytes_to_read);
    let good = if bytes_read > 0 { bytes_read as usize } else { 0 };
    if good < PAGE_SIZE_USIZE {
        core::ptr::write_bytes(page_ptr.add(good), 0, PAGE_SIZE_USIZE - good);
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
fn filemap_pin_cache_page(
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
unsafe fn filemap_bookkeep_mapped_cache_page(phys: u64) {
    use crate::mm::page_desc::pfn_to_page_mut;
    let page = pfn_to_page_mut((phys >> PAGE_SHIFT) as usize);
    if page.is_null() {
        return;
    }
    // SAFETY: descriptor of a live, pinned cache frame.
    unsafe {
        (*page).get_page();
        (*page).inc_mapcount();
    }
}

/// Release a fill-phase page that lost the map race (or whose map failed):
/// a pinned cache frame returns its pin; a private fill copy is freed.
fn release_fill_page(phys: u64, from_cache: bool) {
    if from_cache {
        super::mm_ops::release_cache_mapping_pin((phys >> PAGE_SHIFT) as usize);
    } else {
        crate::mm::page_alloc::free_page(phys as usize);
    }
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
    // R-WRITESTORM-2: a PRESENT USER leaf counts as mapped. A PROT_NONE
    // marker (P=0, U=1, PPN preserved) does NOT — counting it sent the
    // fault into the already-mapped permission check, which reads P=0 as
    // "no leaf" (check_pte_permissions -> None) and killed the task with
    // PermissionDenied without ever consulting the covering VMA (the oh
    // service-kill family). Markers now fall through to the VMA layer,
    // which re-presents the leaf when the VMA grants the access.
    let already_mapped = unsafe {
        PageTableWalker::walk(root_ppn, fault_addr.bits() as u64).map_or(false, |(_ppn, bits)| {
            (bits & PageTableEntry::P) != 0
                && (bits & PageTableEntry::U) != 0
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
            let now = crate::drivers::timer::get_jiffies();
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
        addr_space.get_vma_file(vma_start)
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
            if let Some(live) = crate::fs::devfs::devfs_reg_current_size(f.as_ref()) {
                return Some(live);
            }
            // SAFETY: inode cell written at open time; read-only.
            let inode_opt = unsafe { &*f.inode.get() };
            inode_opt.as_ref().map(|i| i.get_size())
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
        let page_base = VirtAddr::new(fault_addr.bits() & !(PAGE_SIZE as u64 - 1));
        if let Some(raw) = read_pte_raw(root_ppn, page_base) {
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
                let cow_bit =
                    raw & super::mm_ops::cow_flags::COW;
                let pcache_bit = raw
                    & (super::mm_ops::cow_flags::PCACHE
                        | super::mm_ops::cow_flags::PCACHE_SHARED);
                if cow_bit != 0 {
                    flags |= cow_bit;
                    flags &= !PageTableEntry::W;
                }
                if pcache_bit != 0 {
                    flags |= pcache_bit;
                    if flags & PageTableEntry::W != 0
                        && pcache_bit & super::mm_ops::cow_flags::PCACHE_SHARED == 0
                    {
                        // Private cache-backed leaf: never W in place —
                        // divert the first write through COW (see
                        // mprotect_pte_range). SHARED cache leaves keep W:
                        // their stores must land in the cache frame
                        // (session 25).
                        flags |= super::mm_ops::cow_flags::COW;
                        flags &= !PageTableEntry::W;
                    }
                }
                let _pte_guard =
                    crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
                // Re-read under the lock: a racing mprotect / COW fault /
                // swap-in may have replaced the leaf since the lock-free
                // read above; only rewrite a still-marker leaf.
                if let Some(now) = read_pte_raw(root_ppn, page_base) {
                    if is_protnone_marker(now) {
                        let new_bits =
                            (now & PageTableEntry::PHYS_MASK_PUBLIC) | flags;
                        write_pte_raw(root_ppn, page_base, new_bits);
                        // SAFETY: invlpg on a user VA from kernel context.
                        unsafe { crate::arch::cpu::invlpg(page_base.bits()); }
                        return MmFaultResult::Handled;
                    }
                    // Leaf changed under us: fall through; the demand
                    // paths below (or the retry) act on the new state.
                }
            }
        }
    }

    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::Steptrace) {
        crate::pr_debug!("pagefault: map new page at {:#x}, type={:?}", fault_addr.bits(), vma_type);
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
            if file_offset >= effective_size {
                crate::pr_err!(
                    "pagefault: SIGBUS forensics: addr {:#x} vma_start {:#x} vma_off {:#x} file_off {:#x} eff_size {:#x} mmap_snap_size {:#x} fd {} pid {} path {}",
                    page_virt_addr.as_usize(),
                    vma_start,
                    vma_offset,
                    file_offset,
                    effective_size,
                    vma_file_size as usize,
                    vma_file_fd,
                    unsafe { crate::process::current_pid() },
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
                if let Some(shared_phys) = vm_file
                    .as_ref()
                    .and_then(|f| {
                        let ops = f.get_ops()?;
                        if !core::ptr::eq(
                            ops as *const _,
                            &crate::fs::devfs::DEVFS_REG_FILE_OPS as *const _,
                        ) {
                            return None;
                        }
                        // Page-granular offset within the file.
                        crate::fs::devfs::shared_fault_file_page(
                            f.as_ref(),
                            (file_offset / (PAGE_SIZE as usize))
                                * (PAGE_SIZE as usize),
                        )
                    })
                {
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
                        let _sp_guard =
                            crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
                        if unsafe {
                            PageTableWalker::walk(root_ppn, fault_addr.bits() as u64)
                        }
                        .map_or(false, |(_ppn, bits)| {
                            pte_counts_as_mapped(bits, fault_addr.bits() as u64)
                        })
                        {
                            return MmFaultResult::Handled;
                        }
                        // SAFETY: root_ppn valid; fault_addr page-aligned;
                        // sp_phys is the shared file page; flags built above.
                        unsafe {
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
                            use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
                            let page =
                                pfn_to_page_mut((shared_phys >> PAGE_SHIFT) as usize);
                            if !page.is_null() {
                                // SAFETY: page descriptor of a live frame;
                                // the backing holds the base reference, so
                                // get_page cannot race a 0→1 resurrection.
                                unsafe {
                                    (*page).get_page();
                                    (*page).set_flag(PageFlag::Anonymous);
                                    (*page).inc_mapcount();
                                    crate::mm::rmap::page_record_mapping(
                                        &*page,
                                        addr_space as *const _ as usize,
                                        fault_addr.bits() as usize,
                                    );
                                }
                            }
                        }
                        addr_space.add_rss(1);
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
                    .or_else(|| unsafe { crate::fs::get_file_fd(vma_file_fd as usize) });
                if let Some(file) = file
                    .filter(|f| crate::fs::ext4::file::is_ext4_file(f.as_ref()))
                {
                    if let Some(cache_phys) = filemap_pin_cache_page(
                        file.as_ref(),
                        file_offset,
                        effective_size,
                        true,
                    ) {
                        // file_offset < effective_size held (SIGBUS gate).
                        let mut sp_flags = PageTableEntry::P
                            | PageTableEntry::A
                            | PageTableEntry::D
                            | PageTableEntry::NX
                            | PageTableEntry::U
                            | super::mm_ops::cow_flags::PCACHE;
                        let want_w = vma_flags.is_writable();
                        if want_w {
                            sp_flags |=
                                PageTableEntry::W | super::mm_ops::cow_flags::PCACHE_SHARED;
                        }
                        if vma_flags.is_executable() {
                            sp_flags &= !PageTableEntry::NX;
                        }
                        let _sp_guard =
                            crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
                        if unsafe {
                            PageTableWalker::walk(root_ppn, fault_addr.bits() as u64)
                        }
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
                        unsafe {
                            if !map_page(root_ppn, fault_addr, PhysAddr::new(cache_phys), sp_flags)
                            {
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
                        if want_w {
                            // Conservative v1 dirty mark, AFTER the mapping
                            // is installed (a failed/raced attempt leaves no
                            // phantom dirty): a writable mapping MAY write;
                            // flushing an unwritten page is harmless I/O.
                            // Cleared only when no mapping holds a pin
                            // anymore (flush_done). pte -> inodes order,
                            // same as the release sites.
                            if let Some(inode) = unsafe { &*file.inode.get() }.as_ref() {
                                crate::fs::page_cache::get_page_cache().mark_dirty(
                                    inode.fs_id,
                                    inode.ino as u64,
                                    (file_offset / PAGE_SIZE_USIZE) as u64,
                                );
                            }
                        }
                        addr_space.add_rss(1);
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
            unsafe { crate::fs::get_file_fd(vma_file_fd as usize) }
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
            let n_pages =
                (win_end - fault_page + PAGE_SIZE_USIZE - 1) / PAGE_SIZE_USIZE;

            // Fill phase — NO locks held (R73: read_at may block on I/O;
            // the vma guard was dropped above and the pte lock must not
            // serialize I/O across mms). Cache pages are PINNED here
            // (pin released by the map-phase losers below, or owned by
            // the PTE via the PCACHE bit on success).
            let mut phys: [u64; FAULT_AROUND_PAGES] = [0; FAULT_AROUND_PAGES];
            let mut from_cache = [false; FAULT_AROUND_PAGES];
            let mut filled = 0usize;
            let cacheable = crate::fs::ext4::file::is_ext4_file(file.as_ref());
            for i in 0..n_pages {
                let off_i = vma_offset + (fault_page - vma_start) + i * PAGE_SIZE_USIZE;
                if cacheable {
                    if let Some(p) =
                        filemap_pin_cache_page(file.as_ref(), off_i, effective_size, false)
                    {
                        phys[filled] = p;
                        from_cache[filled] = true;
                        filled += 1;
                        continue;
                    }
                    // Hole or populate failure — the private zero-filled
                    // copy below (holes never enter the page cache).
                }
                // Partial window on allocator failure is fine (Linux
                // behaves the same); zero filled pages means the single
                // page path below reports OutOfMemory.
                let Some(pa) = alloc_user_phys_page() else { break };
                let page_ptr = phys_to_virt(PhysAddr::new(pa)).bits() as *mut u8;
                // SAFETY: page_ptr is phys_to_virt of a freshly
                // allocated, exclusively owned page; fill_file_page only
                // writes within its PAGE_SIZE bounds.
                unsafe { fill_file_page(file.as_ref(), off_i, page_ptr, effective_size) };
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
                let mut pte_flags = PageTableEntry::P | PageTableEntry::A
                    | PageTableEntry::D | PageTableEntry::NX | PageTableEntry::U;
                if vma_flags.is_executable() {
                    pte_flags &= !PageTableEntry::NX;
                }
                let cow_bit = if vma_flags.is_writable() {
                    super::mm_ops::cow_flags::COW
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
                let vma_mgr2 = addr_space.vma_read();
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
                let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
                let mut mapped = 0u64;
                let mut fault_pg_failed = false;
                for i in 0..filled {
                    let va_i = VirtAddr::new((fault_page + i * PAGE_SIZE_USIZE) as u64);
                    // Re-check per page (E8-MM device-window-hole rule).
                    // A page that lost a race (concurrent CLONE_VM fault
                    // or a racing mprotect marker) releases its pin/copy
                    // and is skipped.
                    if unsafe { PageTableWalker::walk(root_ppn, va_i.bits() as u64) }
                        .map_or(false, |(_ppn, bits)| {
                            pte_counts_as_mapped(bits, va_i.bits() as u64)
                        })
                    {
                        release_fill_page(phys[i], from_cache[i]);
                        continue;
                    }
                    let flags_i = if from_cache[i] {
                        pte_flags | cow_bit | super::mm_ops::cow_flags::PCACHE
                    } else if vma_flags.is_writable() {
                        pte_flags | PageTableEntry::W
                    } else {
                        pte_flags
                    };
                    // SAFETY: root_ppn is a valid root, va_i is
                    // page-aligned and VMA-verified, phys[i] is a freshly
                    // filled private page or a pinned cache frame, flags_i
                    // built from the verified VMA's permissions.
                    unsafe {
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
                        unsafe { filemap_bookkeep_mapped_cache_page(phys[i]) };
                    } else {
                        // Private fill copy: an exclusive anonymous page
                        // (the session-22 "rmap not wired yet" gap —
                        // without flags these were the census-invisible
                        // unreclaimable copies).
                        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
                        let page = pfn_to_page_mut((phys[i] >> PAGE_SHIFT) as usize);
                        if !page.is_null() {
                            // SAFETY: freshly allocated, exclusively owned.
                            unsafe {
                                (*page).set_flag(PageFlag::Anonymous);
                                (*page).set_flag(PageFlag::SwapBacked);
                                (*page).set_index(va_i.bits() as usize / PAGE_SIZE_USIZE);
                                (*page).inc_mapcount();
                                crate::mm::rmap::page_record_mapping(
                                    &*page,
                                    addr_space as *const _ as usize,
                                    va_i.bits() as usize,
                                );
                                crate::mm::lru::page_add_anon_lru(&*page);
                            }
                        }
                    }
                    mapped += 1;
                }
                drop(_pte_guard);
                drop(vma_mgr2);

                if mapped > 0 {
                    addr_space.add_rss(mapped);
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
                    core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
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
                            let bytes_read = file.read_at(
                                file_offset as u64,
                                page_ptr,
                                bytes_to_read,
                            );

                            if will_full_read && bytes_read == 0 {
                                // Read failed outright on a page we did
                                // not pre-zero: provide the zero base
                                // the pre-zero path would have.
                                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
                            }
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
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
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
                    VmaType::FileBacked => {
                        // Private file copy (write-fault COW fill on a
                        // private VMA — do_cow_fault parity; also the
                        // non-ext4/hole fallbacks). Session 22: this arm
                        // used to leave the page with NO descriptor state
                        // ("rmap not wired yet") — census-invisible and
                        // unreclaimable, the fake-OOM drainer's
                        // accounting face. An exclusive copy IS an
                        // anonymous page from reclaim's point of view.
                        (*page).set_flag(PageFlag::Anonymous);
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

/// R-WRITESTORM-2: write one 4K leaf PTE (raw bits), following the same
/// 4-level walk as read_pte_raw. Caller must hold PTE_MODIFY_LOCK and
/// only call it for addresses whose walk read_pte_raw already resolved
/// (the upper levels exist).
pub(crate) fn write_pte_raw(root_ppn: u64, vaddr: VirtAddr, bits: u64) {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;
    // SAFETY: same walk as read_pte_raw; the leaf exists (caller read it
    // via read_pte_raw on the same root/addr), so the intermediate
    // tables are present. Only the leaf is replaced.
    unsafe {
        let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
        let pte4 = (*root_table).get(vpn4);
        if !pte4.is_valid() { return; }
        let table3 = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
        let pte3 = (*table3).get(vpn3);
        if !pte3.is_valid() || pte3.is_leaf() { return; }
        let table2 = get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
        let pte2 = (*table2).get(vpn2);
        if !pte2.is_valid() || pte2.is_leaf() { return; }
        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        (*table1).set(vpn1, super::pagetable::PageTableEntry::from_bits(bits));
    }
}

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
    let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
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
