//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 内存管理模块

pub mod buddy_allocator;
pub mod allocator;
pub mod page;
pub mod page_desc;
pub mod vma;
pub mod pagemap;
pub mod slab;
pub mod pcp;

pub use page::*;
pub use page_desc::{Page, PageFlag, PageFlags, PageType};

pub const PAGE_SIZE: usize = 4096;

pub const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB

pub const KERNEL_VIRT_BASE: usize = 0xffff_0000_0000_0000;

pub const USER_VIRT_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_VIRT_TOP: usize = 0x0000_0000_7fff_ffff;

pub use allocator::init_heap;
pub use page_desc::{init_mem_map, mem_map, pfn_to_page, pfn_to_page_mut, page_to_pfn};
pub use slab::{kmalloc, kfree, kzalloc, init_slab, slab_stats};
pub use pcp::{
    init_percpu_pages, alloc_page_pcp, free_page_pcp,
    alloc_kernel_page, alloc_user_page, free_kernel_page, free_user_page,
    pcp_stats, MigrateType, GFP_KERNEL, GFP_USER,
};
