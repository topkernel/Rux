//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 memory layout — constants, address types, kernel mapping.
//!
//! Layout (4-level paging, 48-bit VA):
//!
//! ```text
//! 0x0000_0000_0000_0000 .. 0x0000_7fff_ffff_f000   user space (TASK_SIZE, 128TB)
//! 0xffff_8880_0000_0000 .. +RAM                     linear (direct) map, PAGE_OFFSET
//! 0xffff_c900_0000_0000 .. 0xffff_ea00_0000_0000    vmalloc / ioremap space
//! 0xffff_ea00_0000_0000 .. +64GB                     vmemmap (struct page array)
//! 0xffff_ffff_ff60_0000 .. 0xffff_ffff_ffff_0000     fixmap (16MB)
//! 0xffff_ffff_8020_0000 ..                            kernel image (linked)
//! ```
//!
//! The kernel image is linked at VMA 0xffffffff80200000 with LMA 0x200000;
//! VMA - 0xffffffff80000000 == LMA holds for the whole image (same trick as
//! __START_KERNEL_map on x86 Linux), which is what the boot stub's 2MB
//! higher-half pages assume.

// ==================== Page geometry ====================

pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u64 = 12;
pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;
pub const VA_BITS: u64 = 48;
pub const VA_MASK: u64 = (1 << VA_BITS) - 1;

pub const PTRS_PER_PTE: u64 = 512;
pub const PTRS_PER_PMD: u64 = 512;
pub const PTRS_PER_PUD: u64 = 512;
pub const PTRS_PER_PGD: u64 = 512;

/// 4-level shifts: PGD→PUD→PMD→PTE
pub const PGDIR_SHIFT: u64 = 39;   // PGD maps 512GB
pub const PUD_SHIFT: u64 = 30;     // PUD maps 1GB
pub const PMD_SHIFT: u64 = 21;     // PMD maps 2MB

pub const PGDIR_SIZE: u64 = 1 << PGDIR_SHIFT;
pub const PUD_SIZE: u64 = 1 << PUD_SHIFT;
pub const PMD_SIZE: u64 = 1 << PMD_SHIFT;

// ==================== Address space split ====================

/// User space limit: top of the lower canonical half minus one page
pub const TASK_SIZE: usize = 0x0000_7fff_ffff_f000;
pub const USER_PTRS_PER_PGD: usize = 256;                 // half of PML4
pub const KERNEL_PGD_START: usize = USER_PTRS_PER_PGD;    // PML4[256..]

/// Linear (direct) mapping base — Linux PAGE_OFFSET
pub const PAGE_OFFSET: usize = 0xffff_8880_0000_0000;
/// Higher-half kernel image base (VMA base whose LMA is 0)
pub const KERNEL_LINK_BASE: usize = 0xffff_ffff_8000_0000;
/// Kernel image link address (first byte of .text)
pub const KERNEL_LINK_ADDR: usize = KERNEL_LINK_BASE + 0x20_0000;

pub const KERN_VIRT_SIZE: usize = 128 * 1024 * 1024 * 1024 * 1024; // top half

pub const VMALLOC_SIZE: usize = 32 * 1024 * 1024 * 1024 * 1024;    // 32TB
pub const VMALLOC_END: usize = 0xffff_ea00_0000_0000;              // vmemmap start
pub const VMALLOC_START: usize = VMALLOC_END - VMALLOC_SIZE;

pub const VMEMMAP_SIZE: usize = 4 * 1024 * 1024 * 1024;            // 4GB
pub const VMEMMAP_END: usize = VMALLOC_END;
pub const VMEMMAP_START: usize = VMEMMAP_END - VMEMMAP_SIZE;

// ==================== Physical layout ====================

/// x86 RAM starts at physical 0
pub const PHYS_MEMORY_BASE: usize = 0;
/// VA - PA offset of the linear map
pub const VA_PA_OFFSET: usize = PAGE_OFFSET - PHYS_MEMORY_BASE;

/// Kernel physical load address (LMA of .text)
pub const KERNEL_ENTRY: u64 = 0x20_0000;
/// Kernel image physical size budget (link-time end clamps the truth)
pub const KERNEL_SIZE: u64 = 0x40_0000; // 64MB

/// Kernel heap physical start (after the 64MB image window)
pub const HEAP_START: u64 = 0x40_0000;
pub const SLAB_START_DEFAULT: u64 = 0x80_0000;

// ==================== Platform devices (q35) ====================

/// COM1 — port I/O (0x3f8), not MMIO; informational only
pub const UART_BASE: u64 = 0x3f8;
/// q35 MMCONFIG window (PCI ECAM), 256MB
pub const PCIE_ECAM_BASE: u64 = 0xb000_0000;
pub const PCIE_ECAM_SIZE: u64 = 0x1000_0000;
/// q35 PCI MMIO hole (32-bit BARs live here)
pub const PCI_MMIO_BASE: u64 = 0xe000_0000;
/// IOAPIC (also fixed by MADT on q35)
pub const IOAPIC_BASE: u64 = 0xfec0_0000;
/// Local APIC
pub const LAPIC_BASE: u64 = 0xfee0_0000;

/// Is this address inside the linear mapping?
pub const fn is_linear_mapping(virt: usize) -> bool {
    virt >= PAGE_OFFSET && virt < PAGE_OFFSET + (1usize << 46)
}

// ==================== Kernel mapping ====================

/// Runtime kernel mapping information (populated by mm::init)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct KernelMapping {
    pub virt_addr: usize,
    pub virt_offset: usize,
    pub phys_addr: usize,
    pub size: usize,
    pub va_pa_offset: usize,
    pub va_kernel_pa_offset: usize,
    pub page_offset: usize,
}

/// Global kernel mapping structure (Rust-defined on x86; boot.S keeps the
/// bootstrap tables, Rust builds the real ones)
#[used]
#[link_section = ".data"]
pub static mut KERNEL_MAP: KernelMapping = KernelMapping {
    virt_addr: KERNEL_LINK_ADDR,
    virt_offset: 0,
    phys_addr: 0x20_0000,
    size: 0,
    va_pa_offset: VA_PA_OFFSET,
    va_kernel_pa_offset: 0xffff_ffff_8000_0000,
    page_offset: PAGE_OFFSET,
};

/// Physical RAM base (multiboot may refine)
#[used]
#[link_section = ".data"]
pub static mut PHYS_RAM_BASE: usize = PHYS_MEMORY_BASE;

// ==================== mmap constants ====================

/// mmap protection flags (Linux ABI — arch-independent values)
pub mod prot {
    pub const PROT_READ: u32 = 0x1;
    pub const PROT_WRITE: u32 = 0x2;
    pub const PROT_EXEC: u32 = 0x4;
    pub const PROT_NONE: u32 = 0x0;
    pub const PROT_MASK: u32 = 0x7;

    pub const MAP_SHARED: u32 = 0x01;
    pub const MAP_PRIVATE: u32 = 0x02;
    pub const MAP_TYPE_MASK: u32 = 0x0f;
    pub const MAP_FIXED: u32 = 0x10;
    pub const MAP_ANONYMOUS: u32 = 0x20;
    pub const MAP_STACK: u32 = 0x20000;
    pub const MAP_FIXED_NOREPLACE: u32 = 0x100000;
    pub const MAP_HUGETLB: u32 = 0x40000;
    pub const MAP_LOCKED: u32 = 0x2000;
    pub const MAP_NORESERVE: u32 = 0x4000;
    pub const MAP_POPULATE: u32 = 0x8000;
    pub const MAP_NODUMP: u32 = 0x10000;

    pub const EINVAL: i64 = -22;
    pub const ENOMEM: i64 = -12;
    pub const EACCES: i64 = -13;
    pub const EFAULT: i64 = -14;
    pub const ENOSPC: i64 = -28;
    pub const ENODEV: i64 = -19;
    pub const EBADF: i64 = -9;
    pub const EOPNOTSUPP: i64 = -95;
    pub const EEXIST: i64 = -17;
}

/// User-space layout constants
pub mod user_addr {
    pub const USER_START: usize = 0x0000_0000;
    pub const USER_END: usize = super::TASK_SIZE;
    pub const TASK_SIZE: usize = super::TASK_SIZE;

    pub const TASK_UNMAPPED_BASE: usize = TASK_SIZE / 3;
    pub const MMAP_LEGACY_BASE: usize = TASK_SIZE / 3;
    pub const MMAP_START: usize = TASK_SIZE - (64 * 1024 * 1024 * 1024); // 64GB below top
    pub const MMAP_END: usize = TASK_SIZE;

    pub const BRK_DEFAULT: usize = 0x2000_0000;  // 512MB
    pub const BRK_MAX: usize = TASK_UNMAPPED_BASE;

    pub const STACK_TOP: usize = TASK_SIZE - (super::PAGE_SIZE as usize);
    pub const STACK_MAX_SIZE: usize = 8 * 1024 * 1024;  // 8MB
    pub const STACK_MIN_SIZE: usize = 1 * 1024 * 1024;  // 1MB

    pub const HEAP_START: usize = BRK_DEFAULT;
    pub const HEAP_MAX_SIZE: usize = BRK_MAX - BRK_DEFAULT;

    pub const PAGE_ZERO_SIZE: usize = 4 * 1024;
    pub const MIN_MAP_ADDR: usize = PAGE_ZERO_SIZE;
}

/// mmap error codes (arch-interface module name; values are the Linux ABI)
pub mod mmap_error {
    pub const EINVAL: i64 = -22;
    pub const ENOMEM: i64 = -12;
    pub const EACCES: i64 = -13;
    pub const EFAULT: i64 = -14;
    pub const ENOSPC: i64 = -28;
    pub const ENODEV: i64 = -19;
    pub const EBADF: i64 = -9;
    pub const EOPNOTSUPP: i64 = -95;
    pub const EEXIST: i64 = -17;
}

// ==================== Address newtypes ====================

/// Virtual address
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    pub const fn new(addr: u64) -> Self {
        VirtAddr(addr)
    }
    pub const fn bits(&self) -> u64 {
        self.0
    }
    pub fn is_aligned(&self) -> bool {
        (self.0 & PAGE_OFFSET_MASK) == 0
    }
    pub fn floor(&self) -> Self {
        VirtAddr(self.0 & !PAGE_OFFSET_MASK)
    }
    pub fn ceil(&self) -> Self {
        if (self.0 & PAGE_OFFSET_MASK) == 0 {
            *self
        } else {
            VirtAddr((self.0 + PAGE_SIZE) & !PAGE_OFFSET_MASK)
        }
    }
    pub fn page_offset(&self) -> u64 {
        self.0 & PAGE_OFFSET_MASK
    }
    /// Page-table index at the given level (0 = PTE ... 3 = PML4)
    pub fn vpn(&self, level: u8) -> u64 {
        (self.0 >> (PAGE_SHIFT + 9 * level as u64)) & 0x1ff
    }
    /// Named 4-level indices
    pub const fn pgd_index(&self) -> u64 {
        (self.0 >> PGDIR_SHIFT) & 0x1ff
    }
    pub const fn pud_index(&self) -> u64 {
        (self.0 >> PUD_SHIFT) & 0x1ff
    }
    pub const fn pmd_index(&self) -> u64 {
        (self.0 >> PMD_SHIFT) & 0x1ff
    }
    pub const fn pte_index(&self) -> u64 {
        (self.0 >> PAGE_SHIFT) & 0x1ff
    }
    pub fn as_u64(&self) -> u64 {
        self.0
    }
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

/// Physical address
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    pub const fn new(addr: u64) -> Self {
        PhysAddr(addr)
    }
    pub const fn bits(&self) -> u64 {
        self.0
    }
    pub fn is_aligned(&self) -> bool {
        (self.0 & PAGE_OFFSET_MASK) == 0
    }
    pub fn floor(&self) -> Self {
        PhysAddr(self.0 & !PAGE_OFFSET_MASK)
    }
    pub fn ceil(&self) -> Self {
        if (self.0 & PAGE_OFFSET_MASK) == 0 {
            *self
        } else {
            PhysAddr((self.0 + PAGE_SIZE) & !PAGE_OFFSET_MASK)
        }
    }
    pub fn ppn(&self) -> u64 {
        self.0 >> PAGE_SHIFT
    }
}

/// Physical → virtual via the linear map
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    VirtAddr(phys.0 + VA_PA_OFFSET as u64)
}

/// Virtual → physical via the linear map (only valid in the linear region)
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    PhysAddr(virt.0 - VA_PA_OFFSET as u64)
}
