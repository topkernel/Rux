//! 内存管理模块

pub mod buddy_allocator;
pub mod allocator;
pub mod page;
pub mod vma;
pub mod pagemap;

pub use buddy_allocator::*;
pub use page::*;
pub use vma::*;
pub use pagemap::*;

/// 页大小 (4KB)
pub const PAGE_SIZE: usize = 4096;

/// 物理内存大小 (bytes)
pub const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB

/// 内核虚拟地址空间基址
pub const KERNEL_VIRT_BASE: usize = 0xffff_0000_0000_0000;

/// 用户空间地址范围
pub const USER_VIRT_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_VIRT_TOP: usize = 0x0000_0000_7fff_ffff;

/// 重新导出init_heap函数
pub use allocator::init_heap;
