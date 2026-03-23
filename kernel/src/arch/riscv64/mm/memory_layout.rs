//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Sv39 Memory Layout Constants and Address Types
//!
//! This module contains:
//! - Memory layout constants (PAGE_OFFSET, VA_BITS, etc.)
//! - Kernel mapping structure
//! - Virtual and physical address types
//! - mmap constants and user space address definitions

use core::arch::asm;

// ==================== Page Size Constants ====================

pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u64 = 12;
pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;
pub const VA_BITS: u64 = 39;
pub const VA_MASK: u64 = (1 << VA_BITS) - 1;

// ==================== Sv39 Page Table Constants ====================

/// Number of entries per page table level
pub const PTRS_PER_PTE: u64 = 512;
pub const PTRS_PER_PMD: u64 = 512;
pub const PTRS_PER_PUD: u64 = 512;
pub const PTRS_PER_PGD: u64 = 512;

/// Size of each page table level mapping
pub const PGDIR_SHIFT: u64 = 30;  // PGD maps 1GB
pub const PUD_SHIFT: u64 = 30;    // PUD maps 1GB (same as PGD for 3-level)
pub const PMD_SHIFT: u64 = 21;    // PMD maps 2MB

pub const PGDIR_SIZE: u64 = 1 << PGDIR_SHIFT;  // 1GB
pub const PMD_SIZE: u64 = 1 << PMD_SHIFT;      // 2MB

/// TASK_SIZE - Maximum user space address
/// Linux: PGDIR_SIZE * PTRS_PER_PGD / 2 = 1GB * 512 / 2 = 256GB
pub const TASK_SIZE: usize = (PGDIR_SIZE * PTRS_PER_PGD / 2) as usize;

/// USER_PTRS_PER_PGD - Number of PGD entries for user space
/// Linux: USER_PTRS_PER_PGD = TASK_SIZE / PGDIR_SIZE = 256GB / 1GB = 256
/// User space: VPN2 0-255
pub const USER_PTRS_PER_PGD: usize = TASK_SIZE / (PGDIR_SIZE as usize);

/// KERNEL_PGD_START - First PGD entry index for kernel space
/// Linux: Kernel entries start at USER_PTRS_PER_PGD
/// Kernel space: VPN2 256-511
pub const KERNEL_PGD_START: usize = USER_PTRS_PER_PGD;

// ==================== Linux Sv39 Virtual Memory Layout ====================
//
// Sv39 uses 39-bit virtual addresses:
// - User space:   0x00000000_00000000 - 0x0000003f_ffffffff (256GB)
// - Kernel space: 0xffffffc0_00000000 - 0xffffffff_ffffffff (256GB)
//
// Linux kernel virtual address layout (from pgtable.h):
// - KERN_VIRT_SIZE = (PTRS_PER_PGD / 2 * PGDIR_SIZE) / 2 = 128GB
// - VMALLOC_SIZE = KERN_VIRT_SIZE / 2 = 64GB
// - VMEMMAP_SIZE = BIT(VA_BITS - PAGE_SHIFT - 1 + STRUCT_PAGE_SHIFT) = 4GB

/// PAGE_OFFSET - Start of kernel linear mapping region
/// Linux Sv39: 0xffffffd600000000 (from page.h: PAGE_OFFSET_L3)
pub const PAGE_OFFSET: usize = 0xffffffd600000000;

/// KERN_VIRT_SIZE - Half of kernel address space for direct mapping
/// Linux: (PTRS_PER_PGD / 2 * PGDIR_SIZE) / 2 = (256 * 1GB) / 2 = 128GB
pub const KERN_VIRT_SIZE: usize = ((PTRS_PER_PGD / 2) as usize * (PGDIR_SIZE as usize)) / 2;

/// VMALLOC region size (half of KERN_VIRT_SIZE)
/// Linux: KERN_VIRT_SIZE >> 1 = 64GB
pub const VMALLOC_SIZE: usize = KERN_VIRT_SIZE / 2;
pub const VMALLOC_END: usize = PAGE_OFFSET;
pub const VMALLOC_START: usize = PAGE_OFFSET - VMALLOC_SIZE;

/// vmemmap region size
/// Linux: BIT(VA_BITS - PAGE_SHIFT - 1 + STRUCT_PAGE_MAX_SHIFT)
/// For Sv39: BIT(39 - 12 - 1 + 6) = BIT(32) = 4GB
pub const VMEMMAP_SIZE: usize = 4 * 1024 * 1024 * 1024;  // 4GB
pub const VMEMMAP_END: usize = VMALLOC_START;
pub const VMEMMAP_START: usize = VMALLOC_START - VMEMMAP_SIZE;

/// Kernel image mapping region (high address for Sv39)
/// Linux: ADDRESS_SPACE_END - 2GB + 1 = 0xffffffff_80000000
pub const KERNEL_LINK_ADDR: usize = 0xffffffff80000000;

// ==================== Physical <-> Virtual Address Conversion ====================
//
// Linux uses linear mapping for physical memory access:
// - va_pa_offset = PAGE_OFFSET - phys_ram_base
// - phys_to_virt(phys) = phys + va_pa_offset
// - virt_to_phys(virt) = virt - va_pa_offset

/// Physical memory base address (QEMU virt platform)
pub const PHYS_MEMORY_BASE: usize = 0x80000000;

/// VA-PA offset for linear mapping
/// Linux: kernel_map.va_pa_offset = PAGE_OFFSET - phys_ram_base
pub const VA_PA_OFFSET: usize = PAGE_OFFSET - PHYS_MEMORY_BASE;

/// Check if address is in linear mapping region
/// Linux: is_linear_mapping(x) = (x >= PAGE_OFFSET && x < PAGE_OFFSET + KERN_VIRT_SIZE)
#[inline]
pub const fn is_linear_mapping(virt: usize) -> bool {
    virt >= PAGE_OFFSET && virt < PAGE_OFFSET + KERN_VIRT_SIZE
}

// ==================== Physical Memory Layout (QEMU virt platform) ====================

/// Kernel entry point (after OpenSBI)
pub const KERNEL_ENTRY: u64 = 0x80200000;

/// Default kernel size estimate (8MB)
pub const KERNEL_SIZE: u64 = 0x800000;

/// Heap start address (after kernel)
pub const HEAP_START: u64 = 0x80A00000;

/// Slab start address (after heap)
/// Note: Actual address depends on KERNEL_HEAP_SIZE config
pub const SLAB_START_DEFAULT: u64 = HEAP_START + (32 * 1024 * 1024);  // 32MB after heap start

// ==================== Device Addresses (QEMU virt platform) ====================

/// UART base address
pub const UART_BASE: u64 = 0x10000000;

/// VirtIO MMIO base address
pub const VIRTIO_MMIO_BASE: u64 = 0x10001000;

/// PLIC base address
pub const PLIC_BASE: u64 = 0x0c000000;

/// CLINT base address
pub const CLINT_BASE: u64 = 0x02000000;

/// DTB area address
pub const DTB_BASE: u64 = 0xbfe00000;

/// PCIe ECAM base address
pub const PCIE_ECAM_BASE: u64 = 0x30000000;

/// PCI MMIO base address
pub const PCI_MMIO_BASE: u64 = 0x40000000;

// ==================== Linux-style Kernel Mapping ====================

/// Runtime kernel mapping information (Linux-compatible)
///
/// This structure is populated at boot time based on actual memory layout.
/// It mirrors Linux's kernel_mapping structure from arch/riscv/include/asm/page.h
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct KernelMapping {
    /// Kernel virtual address (linked address)
    /// Linux: kernel_map.virt_addr = KERNEL_LINK_ADDR
    pub virt_addr: usize,

    /// KASLR offset (0 if KASLR disabled)
    /// Linux: kernel_map.virt_offset
    pub virt_offset: usize,

    /// Kernel physical load address
    /// Linux: kernel_map.phys_addr = &_start
    pub phys_addr: usize,

    /// Kernel image size
    /// Linux: kernel_map.size = &_end - &_start
    pub size: usize,

    /// VA-PA offset for linear mapping
    /// Linux: kernel_map.va_pa_offset = PAGE_OFFSET - phys_ram_base
    /// Used for phys_to_virt/virt_to_phys conversions
    pub va_pa_offset: usize,

    /// VA-PA offset for kernel mapping
    /// Linux: kernel_map.va_kernel_pa_offset = virt_addr - phys_addr
    /// Used for kernel text/data address conversion
    pub va_kernel_pa_offset: usize,

    /// PAGE_OFFSET value (runtime determined for Sv39/Sv48/Sv57)
    /// Linux: kernel_map.page_offset
    pub page_offset: usize,
}

/// Global kernel mapping structure
/// Defined in boot.S, initialized before rust_main is called
extern "C" {
    pub static mut KERNEL_MAP: KernelMapping;
}

/// Physical RAM base address (runtime determined from device tree)
/// Linux: phys_ram_base
#[used]
#[link_section = ".data"]
pub static mut PHYS_RAM_BASE: usize = PHYS_MEMORY_BASE;

// ==================== mmap Constant definitions ====================

/// mmap protection flags (prot)
pub mod prot {
    /// Page readable
    pub const PROT_READ: u32 = 0x1;
    /// Page writable
    pub const PROT_WRITE: u32 = 0x2;
    /// Page executable
    pub const PROT_EXEC: u32 = 0x4;
    /// Page not accessible
    pub const PROT_NONE: u32 = 0x0;
    /// Protection flags mask
    pub const PROT_MASK: u32 = 0x7;
}

/// mmap mapping flags (flags)
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

/// mmap error codes
pub mod mmap_error {
    /// Invalid parameter
    pub const EINVAL: i64 = -22;
    /// Out of memory
    pub const ENOMEM: i64 = -12;
    /// Permission denied
    pub const EACCES: i64 = -13;
    /// Address not mapped
    pub const EFAULT: i64 = -14;
    /// Device has no space
    pub const ENOSPC: i64 = -28;
    /// Unsupported operation
    pub const ENODEV: i64 = -19;
    /// Bad file descriptor
    pub const EBADF: i64 = -9;
}

/// User space address range (Linux RISC-V Sv39 compatible)
///
/// Linux Sv39 Address Space Layout:
/// - User space: 0x0000000000000000 ~ 0x0000003FFFFFFFFF (256GB)
/// - Kernel space: 0xFFFFFFD600000000 ~ 0xFFFFFFFFFFFFFFFF (high canonical)
pub mod user_addr {
    /// User space start address (Linux: 0, but first page unmapped)
    pub const USER_START: usize = 0x0000_0000;

    /// User space end address = TASK_SIZE = 256GB for Sv39
    pub const USER_END: usize = super::TASK_SIZE;

    /// TASK_SIZE - maximum user address (256GB)
    pub const TASK_SIZE: usize = super::TASK_SIZE;

    /// TASK_UNMAPPED_BASE - mmap area start (Linux: TASK_SIZE / 3)
    /// For Sv39: 256GB / 3 ≈ 85GB = 0x1555555555
    pub const TASK_UNMAPPED_BASE: usize = super::TASK_SIZE / 3;

    /// mmap legacy base (for legacy mmap layout, bottom-up)
    pub const MMAP_LEGACY_BASE: usize = super::TASK_SIZE / 3;

    /// mmap area start address (top-down from TASK_SIZE)
    /// Modern Linux uses top-down mmap by default
    pub const MMAP_START: usize = super::TASK_SIZE - (64 * 1024 * 1024 * 1024); // 64GB below TASK_SIZE

    /// mmap area end address
    pub const MMAP_END: usize = super::TASK_SIZE;

    /// brk default start address
    /// We use a higher address to avoid conflict with UART (0x10000000)
    pub const BRK_DEFAULT: usize = 0x2000_0000;  // 512MB

    /// brk maximum address (end of heap area)
    pub const BRK_MAX: usize = TASK_UNMAPPED_BASE;

    /// Stack base (grows down from TASK_SIZE - PAGE_SIZE)
    pub const STACK_TOP: usize = super::TASK_SIZE - (super::PAGE_SIZE as usize);

    /// Stack maximum size (Linux default: 8MB)
    pub const STACK_MAX_SIZE: usize = 8 * 1024 * 1024;  // 8MB

    /// Stack minimum size (1MB)
    pub const STACK_MIN_SIZE: usize = 1 * 1024 * 1024;  // 1MB

    /// Heap start address (for compatibility, same as BRK_DEFAULT)
    pub const HEAP_START: usize = BRK_DEFAULT;

    /// Heap maximum size
    pub const HEAP_MAX_SIZE: usize = BRK_MAX - BRK_DEFAULT;

    /// First page size (null pointer guard)
    pub const PAGE_ZERO_SIZE: usize = 4 * 1024;  // 4KB null page

    /// Minimum address for user mappings (skip null page)
    pub const MIN_MAP_ADDR: usize = PAGE_ZERO_SIZE;
}

// ==================== Address Types ====================

/// Virtual address type with Sv39 sign extension
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    /// Create virtual address with proper Sv39 sign extension
    ///
    /// Sv39 uses 39-bit virtual addresses with sign extension:
    /// - If bit 38 = 0: bits 63-39 must be 0 (user space)
    /// - If bit 38 = 1: bits 63-39 must be 1 (kernel space)
    #[inline]
    pub const fn new(addr: u64) -> Self {
        let bit38 = (addr >> 38) & 1;
        if bit38 == 1 {
            // Kernel address: sign extend bit 38 to bits 63-39
            Self(addr | 0xFFFFFFC0_00000000)
        } else {
            // User address: clear bits 63-39
            Self(addr & 0x0000007F_FFFFFFFF)
        }
    }

    /// Get raw value
    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }

    /// Page alignment check
    #[inline]
    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_OFFSET_MASK == 0
    }

    /// Page floor
    #[inline]
    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    /// Page ceiling
    #[inline]
    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK)
    }

    /// Page offset
    #[inline]
    pub fn page_offset(&self) -> u64 {
        self.0 & PAGE_OFFSET_MASK
    }

    /// Calculate virtual page number for given level
    #[inline]
    pub fn vpn(&self, level: u8) -> u64 {
        (self.0 >> (PAGE_SHIFT + 9 * level as u64)) & 0x1FF
    }

    /// Get u64 value
    #[inline]
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    /// Get usize value
    #[inline]
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

/// Physical address type
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Create physical address
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// Get raw value
    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }

    /// Page alignment check
    #[inline]
    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_OFFSET_MASK == 0
    }

    /// Page floor
    #[inline]
    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    /// Page ceiling
    #[inline]
    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK)
    }

    /// Calculate physical page number (PPN)
    #[inline]
    pub fn ppn(&self) -> u64 {
        self.0 >> PAGE_SHIFT
    }
}

/// Convert physical address to kernel virtual address (Linux-style PAGE_OFFSET mapping)
///
/// Linux uses: virt = phys + va_pa_offset, where va_pa_offset = PAGE_OFFSET - phys_ram_base
#[inline]
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    let va_pa_offset = unsafe { KERNEL_MAP.va_pa_offset };
    VirtAddr::new(phys.0.wrapping_add(va_pa_offset as u64))
}

/// Convert kernel virtual address to physical address (Linux-style PAGE_OFFSET mapping)
///
/// Linux uses: phys = virt - va_pa_offset, where va_pa_offset = PAGE_OFFSET - phys_ram_base
#[inline]
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    let addr = virt.0;

    // Check if this is a linear mapping address (PAGE_OFFSET region)
    if is_linear_mapping(addr as usize) {
        let va_pa_offset = unsafe { KERNEL_MAP.va_pa_offset };
        PhysAddr::new(addr - va_pa_offset as u64)
    } else if addr >= KERNEL_ENTRY && addr < 0x90000000 {
        // Legacy identity mapping (for transition period)
        PhysAddr::new(addr)
    } else {
        // User virtual address or other: return as-is
        PhysAddr::new(addr)
    }
}
