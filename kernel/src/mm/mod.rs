//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Memory Management Module

pub mod buddy_allocator;
pub mod allocator;
pub mod layout;
pub mod page;
pub mod page_desc;
pub mod vma;
pub mod pagemap;
pub mod slab;
pub mod meminfo;
pub mod mm_struct;
pub mod memblock;
pub mod zone;
pub mod pglist;
pub mod page_alloc;
pub mod rmap;
pub mod lru;
pub mod vmscan;
pub mod kswapd;
pub mod oom_kill;
pub mod hugepage;
pub mod vmemmap;
pub mod swap;
pub mod compact;
pub mod vdso;

/// x86_64 bring-up stubs for the MmStruct arch-method family — remove
/// when arch/x86_64/mm/mm_ops.rs provides the real implementations.
#[cfg(feature = "x86_64")]
pub mod x86_64_stubs;

// pcp.rs (per-CPU pages) REMOVED (wave-6, review 4.6 / batch-4 S3): the
// module had zero live call sites on the alloc/free paths — every
// allocation went straight to the zone lock — while carrying a
// `&'static mut` per-CPU array and a next_free-based linkage that would
// have collided with the zone allocator's OnFreelist discipline had it
// ever been wired. Deleting it (rather than minimally wiring the free
// side) keeps the audit surface small; revisit with a proper
// pageset+list_bulk design if zone-lock contention shows up.

pub use page::*;
pub use page_desc::{Page, PageFlag, PageFlags, PageType, copy_page_contents};
pub use mm_struct::{MmStruct, MmFlags, AddressSpace};
pub use layout::{
    kernel_layout_init, kernel_layout, is_kernel_layout_initialized,
    KernelMemoryLayout,
    phys_memory_base, phys_memory_size,
    kernel_start, kernel_end,
    heap_start, heap_end, heap_size,
    slab_start, slab_end, slab_size,
    user_phys_start, user_phys_end, user_phys_size,
    frame_alloc_start, frame_alloc_size,
    print_kernel_layout,
};

pub const PAGE_SIZE: usize = 4096;

/// mmap mapping flags (Linux generic ABI — identical on all supported
/// architectures). riscv64 re-exports its arch module (the values are the
/// ABI); x86_64's pinned arch tree carries no such module, so the
/// constants are defined here.
#[cfg(feature = "riscv64")]
pub use crate::arch::mm::map;

#[cfg(feature = "x86_64")]
pub mod map {
    /// Shared mapping
    pub const MAP_SHARED: u32 = 0x01;
    /// Private copy-on-write mapping
    pub const MAP_PRIVATE: u32 = 0x02;
    /// Mapping type mask
    pub const MAP_TYPE_MASK: u32 = 0x0f;
    /// Fixed address mapping
    pub const MAP_FIXED: u32 = 0x10;
    /// Anonymous mapping (not file-based)
    pub const MAP_ANONYMOUS: u32 = 0x20;
    /// Stack mapping (grows down)
    pub const MAP_STACK: u32 = 0x20000;
    /// Fixed but allows relocation
    pub const MAP_FIXED_NOREPLACE: u32 = 0x100000;
    /// Fill with huge pages
    pub const MAP_HUGETLB: u32 = 0x40000;
    /// Lock pages
    pub const MAP_LOCKED: u32 = 0x2000;
    /// No swap space reservation
    pub const MAP_NORESERVE: u32 = 0x4000;
    /// Fill (align)
    pub const MAP_POPULATE: u32 = 0x8000;
    /// No core dump
    pub const MAP_NODUMP: u32 = 0x10000;
}

/// Allocate fresh physical memory and map it into a user page table.
///
/// Returns the physical address of the FIRST page (callers that write the
/// whole range must walk the page tables — chunks are not contiguous).
/// riscv64 delegates to the arch implementation (chunked buddy blocks);
/// x86_64 walks page-at-a-time through the arch interface until the
/// backend grows its own chunked allocator.
///
/// # Safety
/// `user_ppn` must be a live user page-table root; `virt_addr` must be
/// page-aligned and unmapped in it.
pub unsafe fn alloc_and_map_user_table(
    user_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    #[cfg(feature = "riscv64")]
    {
        // SAFETY: caller guarantees a valid root (see fn doc).
        unsafe { crate::arch::mm::alloc_and_map_to_user_table(user_ppn, virt_addr, size, flags) }
    }
    #[cfg(feature = "x86_64")]
    {
        // SAFETY: caller guarantees a valid root (see fn doc).
        unsafe { alloc_and_map_user_pages_portable(user_ppn, virt_addr, size, flags) }
    }
}

/// Same contract as [`alloc_and_map_user_table`], for anonymous heap
/// growth (sys_brk).
///
/// # Safety
/// Same as [`alloc_and_map_user_table`].
pub unsafe fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    #[cfg(feature = "riscv64")]
    {
        // SAFETY: caller guarantees a valid root (see fn doc).
        unsafe { crate::arch::mm::alloc_and_map_user_memory(user_root_ppn, virt_addr, size, flags) }
    }
    #[cfg(feature = "x86_64")]
    {
        // SAFETY: caller guarantees a valid root (see fn doc).
        unsafe { alloc_and_map_user_pages_portable(user_root_ppn, virt_addr, size, flags) }
    }
}

/// Page-at-a-time alloc+map used by the x86_64 backend paths.
#[cfg(feature = "x86_64")]
unsafe fn alloc_and_map_user_pages_portable(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    use crate::arch::mm::{map_user_page, PageTableEntry, PhysAddr, VirtAddr};

    if size == 0 {
        return None;
    }
    let page_size = PAGE_SIZE as u64;
    let page_count = ((size + page_size - 1) / page_size) as usize;
    let user_flags = flags | PageTableEntry::U;

    let mut first_phys: Option<u64> = None;
    for i in 0..page_count {
        let phys = crate::arch::mm::alloc_user_phys_page()?;
        let virt = VirtAddr::new(virt_addr + i as u64 * page_size);
        // SAFETY: fresh root, freshly allocated page, page-aligned virt.
        unsafe { map_user_page(user_root_ppn, virt, PhysAddr::new(phys), user_flags) };
        if first_phys.is_none() {
            first_phys = Some(phys);
        }
    }
    first_phys
}

/// Activate a page-table root on the current CPU.
///
/// Bridges the per-arch context-switch primitives: riscv64 drives
/// `context::switch_mm(ppn, asid)` (satp + ASID-scoped sfence); x86_64
/// writes CR3 through the interface-parity `mm::write_satp` (no PCID
/// yet, so the ASID argument is unused).
///
/// # Safety
/// `root_ppn` must be a live page-table root; caller must ensure the
/// kernel half is mapped in the target root.
#[inline]
pub unsafe fn switch_address_space(root_ppn: u64, asid: u16) {
    #[cfg(feature = "riscv64")]
    crate::arch::context::switch_mm(root_ppn, asid);
    #[cfg(feature = "x86_64")]
    {
        let _ = asid;
        // SAFETY: caller guarantees a valid root (see fn doc).
        crate::arch::mm::write_satp(root_ppn << 12);
    }
}

/// Invalidate one user page mapping in the local TLB (no cross-CPU
/// shootdown — callers issue that separately via arch::ipi when needed).
///
/// Bridges the per-arch `flush_tlb_page` signatures: riscv64 takes
/// `(vaddr, asid)` (asid 0 = all address spaces), x86_64 takes the
/// address alone (PCID-less invlpg).
#[inline]
pub fn flush_tlb_page_local(addr: u64) {
    #[cfg(feature = "riscv64")]
    crate::arch::mm::flush_tlb_page(addr as usize, 0);
    #[cfg(feature = "x86_64")]
    crate::arch::mm::flush_tlb_page(addr);
}

// Use physical memory size from config (Kernel.toml: memory.physical_memory)
// This allows runtime configuration instead of hardcoding
pub const PHYS_MEMORY_SIZE: usize = crate::config::PHYS_MEMORY_SIZE;

pub const KERNEL_VIRT_BASE: usize = 0xffff_0000_0000_0000;

pub const USER_VIRT_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_VIRT_TOP: usize = 0x0000_0000_7fff_ffff;

pub use allocator::init_heap;
pub use page_desc::{init_mem_map, pfn_to_page, pfn_to_page_mut, page_to_pfn, pfn_valid, phys_valid};
pub use slab::{kmalloc, kfree, kzalloc, init_slab, slab_stats, slab_region, is_slab_initialized};
pub use meminfo::{
    get_memory_info, print_memory_info, get_memory_summary,
    is_memory_low, should_trigger_oom, MemoryInfo, MemorySummary,
};
pub use buddy_allocator::buddy_stats;
pub use page_desc::page_desc_stats;
pub use memblock::{
    memblock_init, memblock_add, memblock_reserve, memblock_reserve_nomap,
    memblock_get_available_region, memblock_total_memory, memblock_available_memory,
    memblock_is_reserved, memblock_find_in_range, memblock_dump, memblock, memblock_mut,
    MemBlock, MemBlockRegion, MemBlockFlags, MemBlockType,
};
pub use zone::{
    ZoneType, Zone, ZoneStats, GfpFlags, MAX_ORDER,
    WMARK_MIN, WMARK_LOW, WMARK_HIGH,
    pfn_to_phys, phys_to_pfn, print_zone_info,
};
pub use pglist::{
    PglistData, NodeStats, MAX_NR_ZONES, MAX_NUMNODES,
    LRU_INACTIVE_ANON, LRU_ACTIVE_ANON, LRU_INACTIVE_FILE, LRU_ACTIVE_FILE,
    LRU_UNEVICTABLE, NR_LRU_LISTS, DEF_PRIORITY,
    init_node_data, node_data, node_data_mut, first_online_node, first_online_node_mut,
    num_online_nodes, select_zone, select_zone_mut, print_buddyinfo, print_zoneinfo,
};
pub use page_alloc::{
    alloc_pages, alloc_page, get_zeroed_page, free_pages, free_page,
    virt_to_page, virt_to_pfn, page_to_phys, page_to_virt,
    __get_free_pages, __get_free_page, __get_zeroed_page, __free_pages, __free_page,
    init_zone_system,
};
pub use rmap::{
    AnonVma, AnonVmaChain,
    page_add_anon_rmap, page_add_file_rmap, page_remove_rmap,
    page_mapped, page_get_mappings, page_referenced, page_clear_referenced,
    try_to_unmap, try_to_unmap_with_swap, rmap_stats, RmapStats,
};
pub use hugepage::{
    HugePageType, HugePageStats,
    PAGE_SHIFT, PMD_SHIFT, PGDIR_SHIFT, PMD_SIZE, PGDIR_SIZE,
    HPAGE_PMD_NR, HPAGE_PGD_NR, HPAGE_PMD_ORDER, HPAGE_PGD_ORDER,
    alloc_hugepage, free_hugepage, alloc_hugepage_pmd, free_hugepage_pmd,
    hugepage_stats, is_pmd_aligned, is_pgd_aligned,
    pmd_align_down, pmd_align_up, pgd_align_down, pgd_align_up,
    vm_flags as huge_vm_flags, pte_flags as huge_pte_flags,
    is_huge_pte, print_hugepage_info,
};
