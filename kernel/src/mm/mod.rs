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
pub mod pcp;
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

pub use page::*;
pub use page_desc::{Page, PageFlag, PageFlags, PageType};
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

// Use physical memory size from config (Kernel.toml: memory.physical_memory)
// This allows runtime configuration instead of hardcoding
pub const PHYS_MEMORY_SIZE: usize = crate::config::PHYS_MEMORY_SIZE;

pub const KERNEL_VIRT_BASE: usize = 0xffff_0000_0000_0000;

pub const USER_VIRT_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_VIRT_TOP: usize = 0x0000_0000_7fff_ffff;

pub use allocator::init_heap;
pub use page_desc::{init_mem_map, mem_map, pfn_to_page, pfn_to_page_mut, page_to_pfn, pfn_valid, phys_valid};
pub use slab::{kmalloc, kfree, kzalloc, init_slab, slab_stats};
pub use pcp::{
    init_percpu_pages, alloc_page_pcp, free_page_pcp,
    alloc_kernel_page, alloc_user_page, free_kernel_page, free_user_page,
    pcp_stats, MigrateType, GFP_KERNEL, GFP_USER,
};
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
    BuddyAllocator, BuddyStats, init_kernel_buddy, buddy_alloc, buddy_free,
    __get_free_pages, __get_free_page, __get_zeroed_page, __free_pages, __free_page,
    init_zone_system,
};
pub use rmap::{
    AnonVma, AnonVmaChain,
    page_add_anon_rmap, page_add_file_rmap, page_remove_rmap,
    page_mapped, page_get_mappings, page_referenced, page_clear_referenced,
    try_to_unmap, rmap_stats, RmapStats,
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
