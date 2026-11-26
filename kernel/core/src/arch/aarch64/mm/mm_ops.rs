//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 memory management operations — MmStruct extensions, user
//! mappings, COW.
//!
//! STAGE-2 PORT of the riscv64 twin (kernel/core/src/arch/riscv64/mm/mm_ops.rs)
//! onto the aarch64 4-level TTBR0 walker. Contracts mirror the twin:
//! INV-COW-1..5 hold verbatim; the divergences are structural and
//! documented at each site:
//! - The user root is a PURE TTBR0 tree — the kernel half lives in the
//!   separate TTBR1 root, so there is no kernel-subtree cloning
//!   (create_user_address_space), no shared-subtree skipping and no
//!   device-window leaf restoring (clear_pte) like the riscv64 twin's
//!   single-root model needs.
//! - Permission bits are Rux SOFTWARE bits (R/W/X/U/A/D mirrors in the
//!   descriptor's upper half, see pagetable.rs); descriptors are built
//!   ONLY through the pagetable.rs constructors.
//! - TLB maintenance goes through asid.rs (tlbi) instead of sfence.vma.
//! - The PCACHE page-cache-alias optimization of the twin is not yet
//!   ported: file faults take the twin's private-copy fallback arm
//!   (functionally correct, one copy per mapper). cow_flags::PCACHE
//!   keeps the bit budget ready for it — mprotect_pte_range and the
//!   PROT_NONE marker re-presentation (page_fault.rs) preserve the bit
//!   exactly like COW, so the port will not have to revisit them
//!   (requirements list in page_fault.rs's module doc).

use core::sync::atomic::Ordering;

extern crate alloc;
use alloc::vec::Vec;

use super::memory_layout::*;
use super::r#unsafe::PtTable;
use super::pagetable::*;
use crate::mm::types::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::types::{MapError, Perm, PageTableType};
use crate::mm::types::{Vma, VmaFlags, VmaOrigin, VmaType};
use crate::mm::{types::MmStruct, hotpath::alloc_pages, types::GfpFlags};

// Re-export AddressSpace for backward compatibility
#[allow(unused_imports)]
pub use crate::mm::types::AddressSpace;

// ==================== Page Table Walker ====================
//
/// Read-only 4-level walker (interface parity with the riscv64
/// `PageTableWalker`): resolves a VA to its leaf `(ppn, pte_bits)`.
///
/// `root_ppn` is the table the walk STARTS at: user VAs walk a TTBR0
/// root, kernel-half VAs walk the TTBR1 (kernel) root — the caller picks
/// the root by address-space ownership the same way the riscv64 twin's
/// callers hand in `mm.pgd`.
///
/// Index shifts descend from the root (L0 = VA[47:39] ... L3 =
/// VA[20:12]): shift for table level `cur` is PAGE_SHIFT + 9*(3-cur).
/// A BLOCK leaf at L0/L1/L2 is offset-adjusted and returned as the 4KB
/// PPN it contains (riscv64 huge-page parity); an L3 PAGE leaf returns
/// its own PPN.
pub struct PageTableWalker;

impl PageTableWalker {
    /// Walk `root_ppn` for `vaddr`; returns the leaf frame PPN (4KB
    /// granule, block leaves adjusted) and the raw descriptor bits.
    ///
    /// # Safety
    /// root_ppn must be a valid page-table root PPN; the walker only
    /// READS table frames (converted through get_page_table_virt).
    pub fn walk(root_ppn: u64, vaddr: u64) -> Option<(u64, u64)> {
        // The typed walk: every frame passes the stale-guard; a garbage
        // root or recycled intermediate returns None like any unmapped
        // address.
        {
            let mut table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
            for cur in 0..4u32 {
                let shift = PAGE_SHIFT + 9 * ((3 - cur) as u64);
                let idx = ((vaddr >> shift) & 0x1FF) as usize;
                let entry = table.get(idx);
                if !entry.is_valid() {
                    return None;
                }
                if entry.is_leaf() || cur == 3 {
                    // Block leaf (L0-L2, 1GB/2MB) or L3 page: fold the
                    // in-page offset so the PPN is 4KB-granular.
                    let page_base = entry.output_addr() + (vaddr & ((1 << shift) - 1));
                    return Some((page_base >> PAGE_SHIFT, entry.bits()));
                }
                table = PtTable::from_phys(entry.ppn() << PAGE_SHIFT)?;
            }
            None
        }
    }

    /// Walk and return the level the leaf was found at (0..=3)
    ///
    /// # Safety
    /// See walk().
    pub fn walk_level(root_ppn: u64, vaddr: u64) -> Option<(u64, u64, u8)> {
        // See walk().
        {
            let mut table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
            for cur in 0..4u32 {
                let shift = PAGE_SHIFT + 9 * ((3 - cur) as u64);
                let idx = ((vaddr >> shift) & 0x1FF) as usize;
                let entry = table.get(idx);
                if !entry.is_valid() {
                    return None;
                }
                if entry.is_leaf() || cur == 3 {
                    let page_base = entry.output_addr() + (vaddr & ((1 << shift) - 1));
                    return Some((page_base >> PAGE_SHIFT, entry.bits(), cur as u8));
                }
                table = PtTable::from_phys(entry.ppn() << PAGE_SHIFT)?;
            }
            None
        }
    }
}

// ==================== MmStruct Extension Methods ====================

impl MmStruct {
    /// Enable this address space (switch TTBR0 + ASID)
    ///
    /// # Safety
    /// pgd must be a valid user root PPN.
    pub fn enable(&self) {
        let satp = Satp::long(self.pgd, 0);
        super::asid::write_satp(satp.0 as usize);
    }

    /// Disable the address space (switch to the kernel root)
    ///
    /// # Safety
    /// Switches TTBR0 to the kernel root with a full flush.
    pub fn disable() {
        let kernel_ppn = super::mmu_init::root_page_table_ppn();
        super::asid::write_satp(super::asid::build_satp(0, kernel_ppn as usize));
    }

    /// Flush the entire TLB
    ///
    /// # Safety
    /// Callers must have repaired the affected translations.
    pub fn flush_tlb() {
        super::asid::flush_tlb_all();
    }

    /// Flush the TLB for one page
    ///
    /// # Safety
    /// See flush_tlb.
    pub fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        super::asid::flush_tlb_page(vaddr.as_usize(), 0);
    }

    // ==================== VMA Operations ====================


    /// Map single page (for lazy mapping/page fault handling)
    pub fn map_single_page(&self, virt_addr: VirtAddr, perm: Perm) -> Result<(), MapError> {
        let phys_addr = alloc_user_phys_page().ok_or(MapError::OutOfMemory)? as usize;
        let flags = perm_to_flags(perm, self.space_type());

        // INV-AE-ZP: fresh exclusive frame from alloc_user_phys_page.
        {
            let ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            // SAFETY: exclusively owned fresh frame in the linear map.
            unsafe {
                core::ptr::write_bytes(ptr.bits() as *mut u8, 0, PAGE_SIZE_USIZE)
 };
            core::sync::atomic::compiler_fence(Ordering::Release);
        }

        map_user_page(
            self.pgd,
            virt_addr,
            PhysAddr::new(phys_addr as u64),
            flags,
        );

        {
            use crate::mm::{hotpath::page_desc, types::PageFlag};
            if let Some(page) = page_desc(phys_addr / (PAGE_SIZE as usize)) {
                page.set_flag(PageFlag::Anonymous);
                // SwapBacked + LRU membership: make the page reclaimable
                // via swap-out (vmscan scans LRU_INACTIVE_ANON only).
                page.set_flag(PageFlag::SwapBacked);
                page.set_index(virt_addr.bits() as usize / (PAGE_SIZE as usize));
                page.inc_mapcount();
                crate::mm::hotpath::page_record_mapping(
                    page,
                    self as *const _ as usize,
                    virt_addr.bits() as usize,
                );
                crate::api::mm::page_add_anon_lru(page);
            }
        }

        Ok(())
    }

    /// Unmap VMA (requires write lock)
    pub fn unmap_vma(&self, start: PageVirtAddr) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();

        let _vma = vma_mgr.find(start).ok_or(MapError::NotMapped)?;
        let _ = vma_mgr.remove(start);
        Ok(())
    }


    /// Unmap physical pages in specified range (PTE teardown under
    /// PTE_MODIFY_LOCK; twin protocol)
    pub fn unmap_pages(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // Serialize against fork's table walk and COW faults (per-mm PTE
        // lock; other mms' writers proceed concurrently).
        let _pte_guard = pte_lock_for(self).lock_irqsave();
        let mut addr = start.as_usize();
        let end = addr + size;

        while addr < end {
            // SAFETY: self.pgd is a valid root PPN and addr is
            // page-aligned inside the range being unmapped.
            let leaf = unsafe { PageTableWalker::walk(self.pgd, addr as u64) };

            let Some((ppn_val, pte_bits)) = leaf else {
                addr += PAGE_SIZE_USIZE;
                continue;
            };

            // A valid leaf WITHOUT the U bit is a kernel translation the
            // user VA range overlaps (defensive: aarch64 user roots never
            // hold one today — the kernel half is TTBR1 — but the guard
            // is free). Not this mm's page: no rmap, no refcount, no PTE
            // rewrite.
            if pte_bits & PageTableEntry::U == 0 {
                addr += PAGE_SIZE_USIZE;
                continue;
            }
            // Device frames (virtio-gpu framebuffer) and other
            // boot-reserved/foreign frames are not RAM pages: never
            // put_page/free them (accounting corruption otherwise).
            {
                let phys = (ppn_val as u64) << 12;
                let fb = crate::console::get_framebuffer_info()
                    .map_or(false, |i| {
                        phys >= i.addr && phys < i.addr + i.size as u64
                    });
                if fb || crate::mm::hotpath::is_foreign_user_frame(phys as usize) {
                    // SAFETY: verified present user leaf.
                    unsafe { self.clear_pte(addr as u64); }
                    addr += PAGE_SIZE_USIZE;
                    continue;
                }
            }
            // Remove reverse mapping before clearing the PTE, then drop
            // this mapping's reference; the last reference frees the page.
            {
                use crate::mm::hotpath::pfn_to_page_mut;
                let page = pfn_to_page_mut(ppn_val as usize);
                if !page.is_null() {
                    // SAFETY: null-checked descriptor of the leaf frame
                    // mapped by this address space at `addr`.
                    unsafe {
                        (*page).rmap_alt_remove(
                            self as *const _ as usize,
                            addr / PAGE_SIZE_USIZE,
                        );
                        if (*page).is_mapped() {
                            crate::mm::hotpath::page_remove_rmap(&*page);
                        }
                        (*page).put_page_site(crate::debug::driftwatch::S_UNMAP_RV);
                    }
                    // PCACHE mapping pin drop (kept for parity with the
                    // twin's filemap port; no aarch64 PCACHE producers
                    // yet, so this is dormant).
                    if pte_bits & cow_flags::PCACHE != 0 {
                        release_cache_mapping_pin(ppn_val as usize);
                    }
                }
            }
            // SAFETY: verified present user leaf.
            unsafe {
                self.clear_pte(addr as u64);
            }
            // RSS accounting: one resident page leaves this address space.
            self.sub_rss(1);

            addr += PAGE_SIZE_USIZE;
        }

        // Full local flush + remote shootdown (twin protocol: permission
        // and mapping changes on a live mm require all-PE invalidation).
        // SAFETY: translations were repaired above.
        unsafe {
            super::asid::flush_tlb_all();
        }
        crate::arch::ipi::flush_tlb_others();

        Ok(())
    }

    /// Teardown of ONE present user leaf at `va`, for callers that
    /// already hold PTE_MODIFY_LOCK (mremap's page-move copy; twin
    /// semantics): kernel-only leaves and device frames are left alone,
    /// a normal user leaf gets rmap removal, the put_page release and
    /// its PTE cleared. Returns true if a leaf was torn down.
    ///
    /// # Safety
    /// PTE_MODIFY_LOCK must be held.
    pub fn unmap_one_user_leaf_locked(&self, va: usize) -> bool {
        // Read-only typed walk.
        let Some((ppn_val, pte_bits)) = PageTableWalker::walk(self.pgd, va as u64) else {
            return false;
        };
        if pte_bits & PageTableEntry::U == 0 {
            return false; // kernel translation overlapped in the walk
        }
        // Device/foreign frames: clear the PTE only.
        {
            let phys = (ppn_val as u64) << 12;
            let fb = crate::console::get_framebuffer_info()
                .map_or(false, |i| phys >= i.addr && phys < i.addr + i.size as u64);
            if fb || crate::mm::hotpath::is_foreign_user_frame(phys as usize) {
                self.clear_pte(va as u64);
                return true;
            }
        }
        use crate::mm::hotpath::page_desc;
        // Descriptor of the leaf frame mapped at `va` by this address
        // space (INV-FRAME-1).
        if let Some(page) = page_desc(ppn_val as usize) {
            page.rmap_alt_remove(self as *const _ as usize, va / PAGE_SIZE_USIZE);
            if page.is_mapped() {
                crate::mm::hotpath::page_remove_rmap(page);
            }
            page.put_page_site(crate::debug::driftwatch::S_UNMAP_RV);
            if pte_bits & cow_flags::PCACHE != 0 {
                release_cache_mapping_pin(ppn_val as usize);
            }
        }
        self.clear_pte(va as u64);
        self.sub_rss(1);
        true
    }

    /// Clear the L3 leaf descriptor at `va` (write an empty entry).
    ///
    /// aarch64 divergence: the riscv64 twin RESTORES a kernel
    /// device-window PTE here (user roots clone the kernel's low-half
    /// MMIO megapages); aarch64 user roots are pure TTBR0 trees with no
    /// cloned kernel translations, so the plain 0 clear is complete.
    ///
    /// # Safety
    /// Caller holds PTE_MODIFY_LOCK; `va` is a user address of this mm.
    fn clear_pte(&self, virt: u64) {
        // Walk-only helper on a valid root (typed handle from
        // user_l3_table_for).
        if let Some(l3) = super::mmu_init::user_l3_table_for(self.pgd, virt) {
            let idx = ((virt >> PAGE_SHIFT) & 0x1FF) as usize;
            l3.set(idx, PageTableEntry::new());
        }
    }


}

/// Convert permission to page table flags (Rux software-bit mask; the
/// hardware descriptor is derived at install time — see pagetable.rs)
pub fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
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

// ==================== User page mapping ====================

/// Install ONE 4KB user page into a user root (L3 PAGE descriptor built
/// by the pagetable.rs constructor from the software flag mask), with a
/// per-page TLB invalidation so single-page callers (demand faults,
/// mremap, ashmem/shm) see the new translation on retry — riscv64
/// twin's map_page contract. Returns false when an intermediate table
/// allocation failed (failible map path) or a non-table entry blocks the
/// walk (user trees never host blocks; a foreign entry is corruption).
///
/// Takes NO PTE lock (callers hold it, twin protocol).
///
/// # Safety
/// user_root_ppn must be a valid user root; the caller holds the PTE
/// section; phys is an owned frame.
pub fn map_user_page(
    user_root_ppn: u64,
    user_virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> bool {
    let ok = map_user_page_noflush(user_root_ppn, user_virt, phys, flags);
    if ok {
        // The translation was just (re)written; invalidate any stale
        // entry before the first access.
        super::asid::flush_tlb_page(user_virt.bits() as usize, 0);
    }
    ok
}

/// map_user_page without the TLB invalidation (batch primitive; region
/// callers flush once after their loop).
///
/// # Safety
/// See map_user_page.
pub fn map_user_page_noflush(
    user_root_ppn: u64,
    user_virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> bool {
    // Walk/build under the caller's PTE section; tables come from
    // alloc_page_table (zeroed), the leaf from the constructor.
    {
        let mut table_phys = user_root_ppn << PAGE_SHIFT;
        for cur in 0..3u32 {
            let shift = PAGE_SHIFT + 9 * (3 - cur) as u64;
            let idx = ((user_virt.bits() >> shift) & 0x1FF) as usize;
            let Some(table) = PtTable::from_phys(table_phys) else { return false };
            let entry = table.get(idx);
            if entry.is_valid() && entry.is_table() {
                table_phys = entry.ppn() << PAGE_SHIFT;
                continue;
            }
            if entry.is_valid() {
                // Block leaf in the way: user trees never install blocks
                // (the walker only writes L3 pages), so this is a foreign
                // or corrupt entry — refuse rather than overwrite.
                return false;
            }
            let Some(np) = super::mmu_init::alloc_page_table() else {
                static PTFAIL: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if PTFAIL.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "map_user_page: PT alloc failed (L{} ) at va {:#x}",
                        cur,
                        user_virt.bits()
                    );
                }
                return false;
            };
            table.set(idx, PageTableEntry::new_table(np >> PAGE_SHIFT));
            table_phys = np;
        }
        let Some(l3) = PtTable::from_phys(table_phys) else { return false };
        let idx = ((user_virt.bits() >> PAGE_SHIFT) & 0x1FF) as usize;
        l3.set(idx, PageTableEntry::new_page_user_flags(phys.ppn(), flags));
        true
    }
}

/// Map a user region page-by-page (virt→phys identity stride), one full
/// TLB flush at the end. Argument order matches the riscv64 twin ABI
/// (root, virt, phys, size, flags) used by the generic tree.
///
/// # Safety
/// See map_user_page.
pub fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) -> bool {
    // Serialize against fork's table walk (PTE lock coverage).
    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
    let Some(virt_end_val) = virt_start.checked_add(size) else {
        panic!("map_user_region: virt_start + size overflow");
    };

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);

    let mut virt = virt_start_addr.floor();
    let end = VirtAddr::new(virt_end_val).ceil();

    let mut ok = true;
    while virt.bits() < end.bits() {
        let offset = virt.bits() - virt_start_addr.bits();
        let phys = PhysAddr::new(phys_start_addr.bits() + offset);
        // Batch primitive inside the lock; flush once below.
        if !map_user_page_noflush(user_root_ppn, virt, phys, flags) {
            ok = false;
        }
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
    // Batch of fresh translations; one flush publishes them all.
    super::asid::flush_tlb_all();
    ok
}

// ==================== Allocation / registry ====================

/// Allocate a physical page for user memory (riscv64 twin semantics)
pub fn alloc_user_phys_page() -> Option<u64> {
    let phys = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys != 0 {
        Some(phys as u64)
    } else {
        None
    }
}

/// Create a fresh user address space (empty TTBR0 root).
///
/// aarch64 divergence from the riscv64 twin: NO kernel-mapping clone —
/// the kernel half lives in TTBR1 (a separate root register), so the
/// user root starts (and stays) a pure user tree. This also removes the
/// twin's shared-kernel-subtree hazards entirely (teardown skips,
/// device-window restores, MMIO re-cloning).
pub fn create_user_address_space() -> Option<u64> {
    // The root frame comes from the staged table allocator (zeroed,
    // ledger-stamped in the Late stage).
    let root_phys = super::mmu_init::alloc_page_table()?;
    Some(root_phys >> PAGE_SHIFT)
}

// ==================== Global mm context + PTE lock ====================

/// Global mm context id (debug parity)
pub static MM_CTX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Global PTE modify lock (riscv64 twin: serializes fork walks against
/// COW faults and mprotect/munmap table surgery)
pub static PTE_MODIFY_LOCK: crate::mm::types::GlobalPteLock =
    crate::mm::types::GlobalPteLock::new();

/// Per-mm leaf-PTE section hook (INV-LOCK-7): split mode on all arches
/// since the 2026-10-31 perf review (per-mm spin + global epoch SHARED;
/// see mm/mm_struct.rs `PteLock::lock`).
#[inline]
pub fn pte_lock_for(
    mm: &crate::mm::types::MmStruct,
) -> &crate::mm::types::PteLock {
    // Split mode on all arches (perf review 2026-10-31): per-mm leaf-PTE
    // spin; global-mode users keep the epoch exclusive. See
    // docs/architecture/lock-ordering.md.
    mm.pte_lock()
}

static PTE_GLOBAL_HANDLE: crate::mm::types::PteLock =
    crate::mm::types::PteLock::new();

// ==================== Leaf primitives for shared mm code (FD-1) ====================
//
// mm/rmap.rs (try_to_unmap_inner) and mm/compact.rs (remap_page) used to
// carry Sv39 manual descents of their own — root indexed with bits 38:30
// — which on this 4-level arch read the WRONG L0 slot, failed
// is_valid(), and silently no-oped (swap-out unmap / compaction
// migration reclaimed nothing). The walk shape (L0 → L1 → L2 → L3, bits
// 47:39..20:12) and the descriptor encoding are arch facts and live
// here; the shared mm code calls only these three primitives.

/// Permission/flag bits of a 4K L3 descriptor — everything except the
/// output-address field (bits 47:12): valid/type, AttrIndx, AP, SH, AF,
/// nG, PXN/UXN and the Rux software permission bits. This is the exact
/// bit set [`leaf_pte_install`] puts back.
#[inline]
pub fn pte_flag_bits(bits: u64) -> u64 {
    bits & !0x0000_FFFF_FFFF_F000u64
}

/// Default flag bits for an anonymous user page — compaction's fallback
/// when the pre-unmap walk could not recover the live PTE. Valid PAGE
/// descriptor, normal cacheable, inner-shareable, AF, nG, EL0 RW
/// (software V|R|W|U|A|D), execute-never — the new_page_user shape with
/// X dropped.
pub const PTE_FLAGS_ANON_DEFAULT: u64 = PageTableEntry::V
    | PageTableEntry::TYPE_PAGE
    | PageTableEntry::ATTR_NORMAL
    | PageTableEntry::SH_IS
    | PageTableEntry::AF
    | PageTableEntry::NG
    | PageTableEntry::AP_EL0
    | PageTableEntry::PXN
    | PageTableEntry::UXN
    | PageTableEntry::R
    | PageTableEntry::W
    | PageTableEntry::U
    | PageTableEntry::A
    | PageTableEntry::D;

/// Replace the 4K L3 descriptor at `vaddr` with raw `new_bits` (0 =
/// unmap; a swap or migration entry — a non-valid descriptor may carry
/// arbitrary software bits, the fault path decodes them), under this
/// mm's PTE lock. The leaf must still be valid and map `expect_pfn`
/// (the R7-C3 re-validation: the caller's lock-free walk may have raced
/// a concurrent leaf writer). Returns true when the leaf was replaced;
/// the caller owns the TLB flush.
///
/// A BLOCK leaf at L0-L2 has no 4K slot to replace — returns false
/// (anonymous reclaim targets 4K mappings only).
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub fn leaf_pte_replace(
    mm: &crate::mm::types::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    expect_pfn: u64,
    new_bits: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    let idx = |shift: u64| ((vaddr >> shift) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let l0 = root_table.get(idx(39));
    if !l0.is_valid() || l0.is_leaf() {
        return false;
    }
    let Some(t1) = PtTable::from_phys(l0.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let l1 = t1.get(idx(30));
    if !l1.is_valid() || l1.is_leaf() {
        return false;
    }
    let Some(t2) = PtTable::from_phys(l1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let l2 = t2.get(idx(21));
    if !l2.is_valid() || l2.is_leaf() {
        return false;
    }
    let Some(t3) = PtTable::from_phys(l2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let l3 = t3.get(idx(12));
    if !l3.is_valid() || l3.ppn() != expect_pfn {
        return false;
    }
    t3.set(idx(12), PageTableEntry::from_bits(new_bits));
    true
}

/// Rebuild a VALID 4K L3 descriptor mapping `pfn` at `vaddr` from saved
/// flag bits ([`pte_flag_bits`] shape), under this mm's PTE lock. The
/// leaf slot may currently hold a non-valid marker (the compaction
/// migration window) — only the intermediate levels must exist. The
/// valid bit is OR-ed in unconditionally. Returns true when installed;
/// the caller owns the TLB flush.
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub fn leaf_pte_install(
    mm: &crate::mm::types::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    pfn: u64,
    flags: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    let idx = |shift: u64| ((vaddr >> shift) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let l0 = root_table.get(idx(39));
    if !l0.is_valid() || l0.is_leaf() {
        return false;
    }
    let Some(t1) = PtTable::from_phys(l0.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let l1 = t1.get(idx(30));
    if !l1.is_valid() || l1.is_leaf() {
        return false;
    }
    let Some(t2) = PtTable::from_phys(l1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let l2 = t2.get(idx(21));
    if !l2.is_valid() || l2.is_leaf() {
        return false;
    }
    let Some(t3) = PtTable::from_phys(l2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let new_bits = (pfn << 12) | pte_flag_bits(flags) | PageTableEntry::V;
    t3.set(idx(12), PageTableEntry::from_bits(new_bits));
    true
}

// ==================== Bulk alloc-map helpers ====================

/// Allocate and map anonymous user memory (sys_brk path; riscv64 twin):
/// power-of-two buddy chunks capped at MAX_ORDER, zeroed BEFORE mapping
/// (leak-window fix), preserving present leaves (stale brk growth
/// windows), freeing block excess immediately.
///
/// # Safety
/// user_root_ppn must be a valid user root.
pub fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    if size == 0 {
        return None;
    }
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let mut first_phys = 0u64;
    let mut mapped_pages = 0usize;

    while mapped_pages < page_count {
        let remain = page_count - mapped_pages;
        let order = if remain == 1 {
            0
        } else {
            (remain.next_power_of_two().trailing_zeros() as usize)
                .min(crate::mm::types::MAX_ORDER)
        };
        let block_pages = 1usize << order;
        // block_pages >= remain whenever remain <= 2^MAX_ORDER, so the
        // last chunk covers the rest exactly.
        let chunk_pages = remain.min(block_pages);
        let chunk_bytes = chunk_pages * PAGE_SIZE as usize;

        let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);
        if phys_addr == 0 {
            // All-or-nothing: report failure; sys_brk leaves the break
            // unchanged (mapped-so-far chunks are zero anonymous pages a
            // retry re-maps over).
            return None;
        }

        // Zero BEFORE mapping (twin leak-window fix: the frames are
        // reachable through the linear map regardless).
        // SAFETY: exclusively owned fresh block (INV-AE-ZP).
        unsafe {
            let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(
                virt_addr_ptr.bits() as *mut u8,
                0,
                block_pages * PAGE_SIZE as usize,
            );
        }

        // PRESERVE PRESENT LEAVES (twin: do_brk_flags only maps ABSENT
        // pages): a stale leaf in the brk growth window must not be
        // re-mapped with the fresh zero block.
        {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for i in 0..chunk_pages {
                let va = virt_addr + ((mapped_pages + i) * PAGE_SIZE as usize) as u64;
                let present = PageTableWalker::walk(user_root_ppn, va)
                    .map_or(false, |(_p, bits)| bits & PageTableEntry::V != 0);
                if present {
                    crate::mm::hotpath::free_pages(
                        ((base_pfn + i) as usize) << PAGE_SHIFT,
                        0,
                    );
                    continue;
                }
                map_user_page(
                    user_root_ppn,
                    VirtAddr::new(va),
                    PhysAddr::new(((base_pfn + i) as u64) << PAGE_SHIFT),
                    flags,
                );
            }
        }

        // Free the rounded-up excess immediately (twin R7-C5: unmapped
        // excess pages have no PTE, so no teardown path would free them).
        if block_pages > chunk_pages {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for pfn in (base_pfn + chunk_pages)..(base_pfn + block_pages) {
                crate::mm::hotpath::free_pages((pfn << PAGE_SHIFT) as usize, 0);
            }
        }

        if mapped_pages == 0 {
            first_phys = phys_addr as u64;
        }
        mapped_pages += chunk_pages;
    }

    Some(first_phys)
}

/// Allocate and wire a user page-table region (exec image/stack path;
/// riscv64 twin): BINARY-DECOMPOSITION chunks — each iteration takes the
/// largest power-of-two block that FITS the remaining request (floor),
/// zeroed before mapping, no excess to free. The returned phys base is
/// the FIRST block only; callers that write the whole range must walk
/// the page tables.
///
/// # Safety
/// See alloc_and_map_user_memory.
pub fn alloc_and_map_to_user_table(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    if size == 0 {
        return None;
    }
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let user_flags = flags | PageTableEntry::U;

    let mut first_phys = 0u64;
    let mut mapped_pages = 0usize;
    while mapped_pages < page_count {
        let remain = page_count - mapped_pages;
        // floor(log2(remain)), capped at MAX_ORDER; block_pages <=
        // remain always, so chunk == block and no excess exists.
        let order = ((usize::BITS - 1 - remain.leading_zeros()) as usize)
            .min(crate::mm::types::MAX_ORDER);
        let block_pages = 1usize << order;
        let chunk_pages = remain.min(block_pages);
        let chunk_bytes = chunk_pages * PAGE_SIZE as usize;

        let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);
        if phys_addr == 0 {
            return None; // all-or-nothing; the RAII guard frees partials
        }

        // Zero BEFORE mapping (twin leak-window fix).
        // SAFETY: exclusively owned fresh block.
        unsafe {
            let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, chunk_bytes);
        }

        map_user_region(
            user_root_ppn,
            virt_addr + (mapped_pages * PAGE_SIZE as usize) as u64,
            phys_addr as u64,
            chunk_bytes as u64,
            user_flags,
        );

        if mapped_pages == 0 {
            first_phys = phys_addr as u64;
        }
        mapped_pages += chunk_pages;
    }

    Some(first_phys)
}

// ==================== COW support ====================

/// COW software bits — descriptor upper-half software space (bit budget
/// documented in pagetable.rs; these never reach hardware)
pub mod cow_flags {
    use super::super::pagetable::PageTableEntry;

    /// COW marker: mapping is shared read-only until the fault copies it
    pub const COW: u64 = PageTableEntry::SW_COW;
    /// Page-cache-backed mapping (filemap fault family — reserved for
    /// the twin's PCACHE port; dormant on aarch64 today)
    pub const PCACHE: u64 = PageTableEntry::SW_PCACHE;

    /// Whether these bits describe a shared page-cache mapping
    pub fn pte_is_shared_cache(bits: u64) -> bool {
        bits & PCACHE != 0
    }
}

/// Drop the page-cache pin owned by ONE mapping of a cache-backed frame
/// (twin contract; dormant on aarch64 until the filemap port lands).
pub(super) fn release_cache_mapping_pin(ppn: usize) {
    use crate::mm::hotpath::page_desc;
    let Some(page) = page_desc(ppn) else { return };
    // Descriptor of a live frame that was mapped until this unmap;
    // mapping/index are only cleared by the cache's own release path,
    // which cannot run while a pin (held until this call) exists.
    let key = page.mapping() as u64;
    let index = page.index() as u64;
    if key != 0 {
        crate::fs::hotpath::get_page_cache().put_by_key(key, index);
    }
}

/// Copy the parent's user tree into a fresh child root with COW marking
/// (INV-COW-1..5). Every valid user leaf takes one page reference (one
/// per sharer); WRITABLE non-exempt leaves are downgraded to RO+COW in
/// BOTH parent and child. Foreign (boot-reserved / kernel-heap /
/// framebuffer) frames are copied as-is with no refcount — the owning
/// subsystem releases them. The walk runs under the caller's PTE
/// section (fork's PTE_MODIFY_LOCK).
///
/// aarch64 divergence: the child root is a pure TTBR0 tree — there are
/// no kernel PGD entries to share (the twin copies VPN2 >=
/// KERNEL_PGD_START verbatim).
///
/// # Safety
/// Parent root valid; PTE_MODIFY_LOCK held.
pub fn copy_page_table_cow(
    parent: &crate::mm::types::MmStruct,
    cow_exempt: &[(u64, u64)],
) -> Option<u64> {
    use crate::mm::hotpath::page_desc;

    let parent_root_ppn = parent.root_ppn();
    if parent_root_ppn == 0 {
        return None;
    }

    // Framebuffer frames are device-shared memory and must NEVER be
    // COW-marked — the physical-range exemption survives VMA-state loss
    // (twin fbterm-freeze root cause).
    let fb_frame_range: Option<(u64, u64)> =
        crate::console::get_framebuffer_info()
            .map(|i| (i.addr >> 12, (i.addr + i.size as u64 + 0xFFF) >> 12));

    let child_root_phys = super::mmu_init::alloc_page_table()?;
    let child_root_ppn = child_root_phys >> PAGE_SHIFT;

    // Structural liveness: the caller holds the PTE section for the walk;
    // the stale-frame guard adds recycled-frame containment.
    let parent_root = PtTable::from_phys_live(parent_root_ppn << PAGE_SHIFT)?;
    let child_root = PtTable::from_phys_live(child_root_phys)?;

    for l0 in 0..512usize {
        let e0 = parent_root.get(l0);
        if !e0.is_valid() {
            continue;
        }
        if !e0.is_table() {
            // L0 block (512GB): not a user mapping the walker ever
            // installs; copy verbatim without refcount.
            child_root.set(l0, e0);
            continue;
        }

        let child_t1_phys = super::mmu_init::alloc_page_table()?;
        child_root.set(l0, PageTableEntry::new_table(child_t1_phys >> PAGE_SHIFT));
        let pt1 = PtTable::from_phys_live(e0.ppn() << PAGE_SHIFT)?;
        let ct1 = PtTable::from_phys_live(child_t1_phys)?;

        for l1 in 0..512usize {
            let e1 = pt1.get(l1);
            if !e1.is_valid() {
                continue;
            }
            if e1.is_leaf() {
                // 1GB block leaf: refcount-and-share (twin gigapage arm).
                cow_share_block(e1);
                ct1.set(l1, e1);
                continue;
            }
            if !e1.is_table() {
                continue;
            }

            let child_t2_phys = super::mmu_init::alloc_page_table()?;
            ct1.set(l1, PageTableEntry::new_table(child_t2_phys >> PAGE_SHIFT));
            let pt2 = PtTable::from_phys_live(e1.ppn() << PAGE_SHIFT)?;
            let ct2 = PtTable::from_phys_live(child_t2_phys)?;

            for l2 in 0..512usize {
                let e2 = pt2.get(l2);
                if !e2.is_valid() {
                    continue;
                }
                if e2.is_leaf() {
                    cow_share_block(e2);
                    ct2.set(l2, e2);
                    continue;
                }
                if !e2.is_table() {
                    continue;
                }

                let child_t3_phys = super::mmu_init::alloc_page_table()?;
                ct2.set(
                    l2,
                    PageTableEntry::new_table(child_t3_phys >> PAGE_SHIFT),
                );
                let pt3 = PtTable::from_phys_live(e2.ppn() << PAGE_SHIFT)?;
                let ct3 = PtTable::from_phys_live(child_t3_phys)?;

                for l3 in 0..512usize {
                    // The walk runs under fork's PTE_MODIFY_LOCK — no
                    // per-leaf locking here (the lock is not reentrant).
                    let e3 = pt3.get(l3);
                    if !e3.is_valid() {
                        continue;
                    }

                    let leaf_va = ((l0 as u64) << 39)
                        | ((l1 as u64) << 30)
                        | ((l2 as u64) << 21)
                        | ((l3 as u64) << 12);
                    let leaf_phys = e3.output_addr();

                    let exempt = cow_exempt
                        .iter()
                        .any(|(s, e)| leaf_va >= *s && leaf_va < *e)
                        || fb_frame_range.map_or(false, |(lo, hi)| {
                            (leaf_phys >> 12) >= lo && (leaf_phys >> 12) < hi
                        });
                    let foreign = crate::mm::hotpath::is_foreign_user_frame(
                        leaf_phys as usize,
                    );

                    let new_entry = if e3.is_user() && !foreign {
                        page_desc((leaf_phys >> PAGE_SHIFT) as usize).map_or(e3, |page| {
                            // One reference per sharer (writable and
                            // read-only alike — twin protocol).
                            page.get_page();
                            page.inc_mapcount();

                            if e3.is_writable() && !exempt {
                                // INV-COW-5: downgrade BOTH parent and
                                // child to RO+COW. The software mask
                                // keeps every permission bit, drops W,
                                // adds COW; rebuild_user_leaf rederives
                                // the hardware attrs (AP_RO).
                                page.set_flag(crate::mm::types::PageFlag::Cow);
                                let sw = (e3.bits()
                                    & PageTableEntry::SW_SOFTWARE_MASK)
                                    & !PageTableEntry::W
                                    | cow_flags::COW;
                                let cow_e =
                                    PageTableEntry::rebuild_user_leaf(e3.bits(), sw);
                                pt3.set(l3, cow_e);
                                cow_e
                            } else {
                                e3
                            }
                        })
                    } else {
                        e3
                    };

                    ct3.set(l3, new_entry);
                }
            }
        }
    }

    // SMP coherence (twin protocol): the walk DOWNGRADED the parent's
    // writable PTEs to RO+COW; every PE's stale writable entries must
    // die or a parent store issued elsewhere bypasses COW into the now
    // shared frame (the twin's lost-write family).
    crate::arch::ipi::flush_tlb_others();
    // Permissions were repaired above.
    super::asid::flush_tlb_all();

    Some(child_root_ppn)
}

/// Refcount-and-share one BLOCK leaf inherited by fork (never COW-marked
/// — the user walker never installs writable blocks; refcount keeps the
/// teardown put balanced).
///
/// # Safety
/// Descriptor belongs to a mapped frame.
fn cow_share_block(entry: PageTableEntry) {
    use crate::mm::hotpath::page_desc;
    if !entry.is_user() {
        return;
    }
    let phys = entry.output_addr();
    if phys == 0
        || crate::mm::hotpath::is_foreign_user_frame(phys as usize)
        || crate::console::get_framebuffer_info().map_or(false, |i| {
            phys >= i.addr && phys < i.addr + i.size as u64
        })
    {
        return;
    }
    if let Some(page) = page_desc((phys >> PAGE_SHIFT) as usize) {
        page.get_page();
        page.inc_mapcount();
    }
}

/// Outcome of a COW fault resolution attempt (twin tri-state: a changed
/// PTE is a Retry, never a kill — the fake-OOM layer-2 family).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowFaultResult {
    /// COW resolved: PTE is writable again (exclusive fast path, or a
    /// private copy was installed and mapped)
    Resolved,
    /// The PTE changed under us between the lock-free COW check and the
    /// locked re-walk — re-execute the faulting instruction
    Retry,
    /// Allocating the private copy failed — genuine out-of-memory
    OutOfMemory,
}

/// Handle copy-on-write page fault (INV-COW-3/4; twin contract):
/// speculative private-copy allocation BEFORE the PTE section (S7 twin —
/// the allocator slowpath must never run inside the epoch), locked
/// re-walk, exclusive fast path (refcount == 1 → restore W in place),
/// copy path (allocate → copy → install → put old). `mm` is the owning
/// address space of `root_ppn`.
///
/// # Safety
/// mm/root_ppn/fault_addr describe the faulting mapping of the current task.
pub fn handle_cow_fault(
    mm: &crate::mm::types::MmStruct,
    root_ppn: u64,
    fault_addr: VirtAddr,
) -> CowFaultResult {
    use crate::mm::hotpath::page_desc;

    // Speculative pre-lock allocation (twin S7-oom01wedge).
    let spec_phys = match alloc_user_phys_page() {
        Some(p) => p,
        None => return CowFaultResult::OutOfMemory,
    };
    struct SpecPage(Option<u64>);
    impl Drop for SpecPage {
        fn drop(&mut self) {
            if let Some(p) = self.0.take() {
                crate::mm::hotpath::free_pages(p as usize, 0);
            }
        }
    }
    let mut spec = SpecPage(Some(spec_phys));

    let _pte_guard = pte_lock_for(mm).lock_irqsave();

    let virt_addr = fault_addr.bits();

    // Locked re-walk: the full 4-level descent (a missing/changed upper
    // level after the lock-free COW check is a Retry, not an error).
    let Some((old_ppn, old_bits)) = PageTableWalker::walk(root_ppn, virt_addr) else {
        return CowFaultResult::Retry;
    };
    if old_bits & cow_flags::COW == 0 {
        // COW bit gone at re-check time: a sibling sharing this mm broke
        // the COW first. Re-execute, do NOT kill (twin fake-OOM note).
        return CowFaultResult::Retry;
    }

    let old_page = page_desc(old_ppn as usize);
    let refcount = match old_page {
        Some(p) => p.refcount(),
        None => 1,
    };

    // Exclusive owner: restore W in place (INV-COW-3). The software mask
    // drops COW/PCACHE and adds W|R (twin R7-A3: W implies R).
    if refcount <= 1 {
        // PCACHE leaf reaching the exclusive arm means the cache entry
        // is gone; the pin drop is a no-op then (twin note).
        let pcache = old_bits & cow_flags::PCACHE;
        let sw = (old_bits & PageTableEntry::SW_SOFTWARE_MASK)
            & !(cow_flags::COW | cow_flags::PCACHE)
            | PageTableEntry::W
            | PageTableEntry::R;
        if pcache != 0 {
            release_cache_mapping_pin(old_ppn as usize);
        }
        // Clear the descriptor flag too (twin R7-C9: "COW set with
        // refcount 1" is an invariant violation).
        if let Some(p) = old_page {
            p.clear_flag(crate::mm::types::PageFlag::Cow);
        }
        // Present user leaf; rebuild preserves its output address.
        write_user_leaf(root_ppn, virt_addr, PageTableEntry::rebuild_user_leaf(old_bits, sw));
        // Fast-path PTE replacement must be visible to every PE that
        // may hold the old RO entry (twin shootdown discipline).
        crate::arch::ipi::flush_tlb_others();
        super::asid::flush_tlb_page(virt_addr as usize, 0);
        return CowFaultResult::Resolved;
    }

    // Shared: install the private copy (INV-COW-4). The page came from
    // the speculative pre-lock alloc; sole consumer, take() cannot fail.
    let new_phys = match spec.0.take() {
        Some(p) => p,
        None => return CowFaultResult::OutOfMemory,
    };
    let new_ppn = new_phys >> PAGE_SHIFT;

    // Copy while the old page is still pinned (refcount >= 2).
    super::r#unsafe::copy_page(old_ppn << PAGE_SHIFT, new_phys);

    // New leaf: old permissions, W restored, no COW/PCACHE (the copy is
    // exclusively owned by this mapping).
    let sw = (old_bits & PageTableEntry::SW_SOFTWARE_MASK)
        & !(cow_flags::COW | cow_flags::PCACHE)
        | PageTableEntry::W
        | PageTableEntry::R;
    let new_pte = PageTableEntry::new_page_user_flags(new_ppn, sw);

    // Install the new PTE before dropping our share of the old page.
    // Verified present leaf slot; fresh owned frame.
    write_user_leaf(root_ppn, virt_addr, new_pte);
    // Twin shootdown discipline: the old frame → private copy swap
    // must not leave a stale translation on any PE.
    crate::arch::ipi::flush_tlb_others();
    super::asid::flush_tlb_page(virt_addr as usize, 0);

    // Release our share of the old page: refcount AND mapcount (the
    // mapping moved to the copy; a leaked mapcount corrupts reclaim).
    if let Some(p) = old_page {
        p.put_page_site(crate::debug::driftwatch::S_COW_RV);
        p.dec_mapcount();
        p.clear_flag(crate::mm::types::PageFlag::Cow);
    }

    // Old PCACHE pin follows the mapping (twin ordering; dormant).
    if old_bits & cow_flags::PCACHE != 0 {
        release_cache_mapping_pin(old_ppn as usize);
    }

    // Reverse mapping + RSS for the new private page.
    {
        use crate::mm::types::PageFlag;
        if let Some(new_page) = page_desc(new_ppn as usize) {
            // Exclusively owned fresh frame.
            new_page.set_flag(PageFlag::Anonymous);
            new_page.set_flag(PageFlag::SwapBacked);
            new_page.set_index(virt_addr as usize / (PAGE_SIZE as usize));
            new_page.inc_mapcount();
            crate::mm::hotpath::page_record_mapping(
                new_page,
                mm as *const _ as usize,
                virt_addr as usize,
            );
            crate::api::mm::page_add_anon_lru(new_page);
        }
        mm.add_rss(1);
    }

    CowFaultResult::Resolved
}

/// Overwrite ONE present L3 user leaf with `entry` (no walk allocation;
/// the caller verified the leaf exists under the PTE lock).
///
/// # Safety
/// PTE_MODIFY_LOCK held; the leaf at `va` is present.
fn write_user_leaf(root_ppn: u64, va: u64, entry: PageTableEntry) {
    // Present-tables-only descent on a valid root.
    let Some(l3) = super::mmu_init::user_l3_table_for(root_ppn, va) else {
        return;
    };
    let idx = ((va >> PAGE_SHIFT) & 0x1FF) as usize;
    l3.set(idx, entry);
}

/// Check if the leaf at `addr` carries the COW marker
///
/// # Safety
/// root_ppn valid; read-only walk.
pub fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    PageTableWalker::walk(root_ppn, addr.bits())
        .map_or(false, |(_ppn, bits)| bits & cow_flags::COW != 0)
}

/// Leaf permissions as (readable, writable, executable, user) from the
/// software mirror bits
///
/// # Safety
/// root_ppn valid; read-only walk.
pub fn check_pte_permissions(
    root_ppn: u64,
    addr: VirtAddr,
) -> Option<(bool, bool, bool, bool)> {
    PageTableWalker::walk(root_ppn, addr.bits()).map(|(_, bits)| {
        (
            bits & PageTableEntry::R != 0,
            bits & PageTableEntry::W != 0,
            bits & PageTableEntry::X != 0,
            bits & PageTableEntry::U != 0,
        )
    })
}

// ==================== mprotect + PROT_NONE markers ====================

/// R-WRITESTORM-2 twin: is this leaf a PROT_NONE marker installed by
/// mprotect(prot==0)? Software mirror V|U with no R/W/X (the descriptor
/// also denies EL0 in hardware — see rebuild_user_leaf_protnone).
pub fn is_protnone_marker(bits: u64) -> bool {
    bits & PageTableEntry::V != 0
        && bits & PageTableEntry::U != 0
        && bits & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X) == 0
}

/// Rewrite the permissions of every PRESENT user leaf in
/// [start_virt, start_virt + num_pages*4K) — the mprotect arm
/// (x86_64 `mprotect_pte_range` twin on the aarch64 4-level walker).
///
/// - The output address (PPN) always survives; COW/PCACHE software
///   bits ride along (COW defers W to the copy fault; PCACHE is a
///   mapping-ownership pin — dropping it would leak the pin).
/// - prot == 0 installs the PROT_NONE MARKER: valid descriptor,
///   software V|U with no R/W/X, hardware denies EL0 entirely. The
///   marker is advisory PTE state — a later fault under a VMA that
///   grants the access re-presents the leaf (page_fault.rs).
/// - User trees only carry L3 page leaves; table/invalid slots are
///   skipped (the range was validated against the VMA list by the
///   caller, so unmapped middles mean "not populated yet" — the
///   demand-fill path will materialize them with the VMA's perms).
///
/// TLB: per-page `tlbi aside1e1is`; the caller holds the PTE section.
///
/// # Safety
/// root_ppn must be the caller's live user page-table root; caller
/// holds the PTE modify section (PTE_MODIFY_LOCK).
pub fn mprotect_pte_range(root_ppn: u64, start_virt: u64, num_pages: u64, prot: u32) {
    // Software base mask. W requires R (hardware AP_RO covers the W=0
    // half; the software mirror keeps the same fold the fault path uses).
    let mut base = PageTableEntry::V | PageTableEntry::U
        | PageTableEntry::A | PageTableEntry::D;
    if prot & 0x1 != 0 {
        base |= PageTableEntry::R;
    }
    if prot & 0x2 != 0 {
        base |= PageTableEntry::R | PageTableEntry::W;
    }
    if prot & 0x4 != 0 {
        base |= PageTableEntry::X;
    }

    for i in 0..num_pages {
        let va = start_virt.wrapping_add(i * PAGE_SIZE);

        // SAFETY: root_ppn is the live user root; the helper only walks
        // table descriptors (no leaf writes at upper levels).
        let Some(l3) = super::mmu_init::user_l3_table_for(root_ppn, va) else {
            continue;
        };
        let idx = ((va >> PAGE_SHIFT) & 0x1FF) as usize;
        // SAFETY: l3 comes from the walker; idx is masked to 9 bits.
        let raw = l3.get(idx).bits();
        // Empty or non-leaf slots: nothing to rewrite (see doc).
        if raw & PageTableEntry::V == 0 || raw & PageTableEntry::TYPE_MASK != PageTableEntry::TYPE_PAGE {
            continue;
        }
        // Foreign/kernel-frame leaves carry no U bit in the mirror —
        // leave them alone (a TTBR0 tree should not hold any, but a
        // stray boot-reserved leaf must not be user-rewritten).
        if raw & PageTableEntry::U == 0 {
            continue;
        }

        let cow_bit = raw & cow_flags::COW;
        let pcache_bit = raw & cow_flags::PCACHE;

        let new_pte = if prot & 0x7 == 0 {
            // PROT_NONE marker (constructor preserves COW/PCACHE itself)
            PageTableEntry::rebuild_user_leaf_protnone(raw)
        } else {
            let mut flags = base;
            if cow_bit != 0 {
                flags |= cow_bit;
                flags &= !PageTableEntry::W; // W deferred to the COW fault
            }
            if pcache_bit != 0 {
                flags |= pcache_bit;
                // A PRIVATE cache leaf never gains W in place: the frame
                // is shared with the page cache, so a W-upgrade diverts
                // the first write through COW (writenotify; twins). A
                // SHARED cache leaf keeps W — stores must land in the
                // cache frame (PCACHE is dormant on aarch64 today, but
                // the ownership rule rides the bit).
                if flags & PageTableEntry::W != 0 && !cow_flags::pte_is_shared_cache(raw) {
                    flags |= cow_flags::COW;
                    flags &= !PageTableEntry::W;
                }
            }
            PageTableEntry::rebuild_user_leaf(raw, flags)
        };

        // SAFETY: same slot the raw bits came from, under the caller's
        // PTE section.
        l3.set(idx, new_pte);
        // SAFETY: this translation changed; drop any cached walk.
        unsafe { super::asid::flush_tlb_page(va as usize, 0) };
    }
}
