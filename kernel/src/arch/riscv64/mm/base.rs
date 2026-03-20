//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V Sv39 virtual memory management
//!
//! RISC-V Sv39 paging specification:
//! - 3-level page table (512 PTE/level)
//! - 39-bit virtual address (512GB)
//! - 4KB page size
//! - Page table entry: 10-bit PPN + 10-bit flags
//!

use crate::mm::{alloc_pages, free_pages, GfpFlags};
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use spin::RwLock;

// ==================== Constant definitions ====================

pub const PAGE_SIZE: u64 = 4096;

pub const PAGE_SHIFT: u64 = 12;

pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;

pub const VA_BITS: u64 = 39;

pub const VA_MASK: u64 = (1 << VA_BITS) - 1;

// ==================== Sv39 Address Space Layout (Linux-compatible) ====================

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

// ==================== Linux Sv39 Virtual Memory Layout ====================
//
// Sv39 uses 39-bit virtual addresses:
// - User space:   0x00000000_00000000 - 0x0000003f_ffffffff (256GB)
// - Kernel space: 0xffffffc0_00000000 - 0xffffffff_ffffffff (256GB)
//
// Linux kernel virtual address layout (from pgtable.h):
// - KERN_VIRT_SIZE = (PTRS_PER_PGD / 2 * PGDIR_SIZE) / 2 = 128GB
// - VMALLOC_SIZE = KERN_VIRT_SIZE / 2 = 64GB
// - VMEMMAP_SIZE = BIT(VA_BITS - PAGE_SHIFT - 1 + STRUCT_PAGE_SHIFT) = 64GB
//
// Address layout (high to low):
// 0xffffffff_ffffffff  - End of address space
// ...
// 0xffffffd8_00000000  - PAGE_OFFSET (linear mapping start)
// 0xffffffd0_00000000  - VMALLOC_END (= PAGE_OFFSET)
// 0xffffffc8_00000000  - VMALLOC_START (= PAGE_OFFSET - VMALLOC_SIZE)
// 0xffffffc0_00000000  - VMEMMAP_END (= VMALLOC_START)
// 0xffffffb8_00000000  - VMEMMAP_START (= VMALLOC_START - VMEMMAP_SIZE)

/// PAGE_OFFSET - Start of kernel linear mapping region
/// Linux Sv39: 0xffffffd800000000 (from page.h: PAGE_OFFSET_L3)
pub const PAGE_OFFSET: usize = 0xffffffd800000000;

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
//
// For Rux (phys_ram_base = 0x80000000):
// - va_pa_offset = 0xffffffd800000000 - 0x80000000 = 0xffffffd000000000

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
///
/// User space layout (within 256GB):
/// - 0x0000000000000000 ~ 0x0000000000000FFF: Null page (unmapped)
/// - 0x0000000000001000 ~ : Code/Data segments (ELF loaded)
/// - brk area follows ELF segments
/// - mmap area: TASK_UNMAPPED_BASE = TASK_SIZE / 3 (~85GB)
/// - Stack: grows down from TASK_SIZE (256GB)
pub mod user_addr {
    /// User space start address (Linux: 0, but first page unmapped)
    pub const USER_START: usize = 0x0000_0000;

    /// User space end address = TASK_SIZE = 256GB for Sv39
    /// This is the maximum address user space can access
    pub const USER_END: usize = super::TASK_SIZE;

    /// TASK_SIZE - maximum user address (256GB)
    pub const TASK_SIZE: usize = super::TASK_SIZE;

    /// TASK_UNMAPPED_BASE - mmap area start (Linux: TASK_SIZE / 3)
    /// For Sv39: 256GB / 3 ≈ 85GB = 0x1555555555
    /// This is where mmap starts allocating by default (top-down)
    pub const TASK_UNMAPPED_BASE: usize = super::TASK_SIZE / 3;

    /// mmap legacy base (for legacy mmap layout, bottom-up)
    /// Linux uses TASK_UNMAPPED_BASE for bottom-up mmap
    pub const MMAP_LEGACY_BASE: usize = super::TASK_SIZE / 3;

    /// mmap area start address (top-down from TASK_SIZE)
    /// Modern Linux uses top-down mmap by default
    /// Starting from high addresses, going down
    pub const MMAP_START: usize = super::TASK_SIZE - (64 * 1024 * 1024 * 1024); // 64GB below TASK_SIZE

    /// mmap area end address
    pub const MMAP_END: usize = super::TASK_SIZE;

    /// brk default start address
    /// Linux: brk starts after loaded ELF segments, typically around 0x10000-0x10000000
    /// We use a higher address to avoid conflict with UART (0x10000000)
    /// Note: musl libc's mallocng uses brk(0) return as mmap hint
    pub const BRK_DEFAULT: usize = 0x2000_0000;  // 512MB - well above UART and away from device region

    /// brk maximum address (end of heap area)
    /// Should be below mmap area
    pub const BRK_MAX: usize = TASK_UNMAPPED_BASE;

    /// Stack base (grows down from TASK_SIZE - PAGE_SIZE)
    /// Linux: stack starts at TASK_SIZE - PAGE_SIZE (last valid user address)
    /// Note: TASK_SIZE is the first kernel address, not the last user address
    pub const STACK_TOP: usize = super::TASK_SIZE - (super::PAGE_SIZE as usize);

    /// Stack maximum size (Linux default: 8MB, ulimit configurable)
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

// ==================== Address types ====================

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
        // Sv39 sign extension: if bit 38 is set, extend to all upper bits
        // This ensures canonical address form
        let bit38 = (addr >> 38) & 1;
        if bit38 == 1 {
            // Kernel address: sign extend bit 38 to bits 63-39
            Self(addr | 0xFFFFFFC0_00000000)
        } else {
            // User address: clear bits 63-39
            Self(addr & 0x0000007F_FFFFFFFF)
        }
    }

    /// Get value
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

    /// Calculate page number
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Create physical address
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// Get value
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

// ==================== Page table entry ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// V (Valid) - bit 0
    pub const V: u64 = 1 << 0;
    /// R (Read) - bit 1
    pub const R: u64 = 1 << 1;
    /// W (Write) - bit 2
    pub const W: u64 = 1 << 2;
    /// X (Execute) - bit 3
    pub const X: u64 = 1 << 3;
    /// U (User) - bit 4
    pub const U: u64 = 1 << 4;
    /// G (Global) - bit 5
    pub const G: u64 = 1 << 5;
    /// A (Accessed) - bit 6
    pub const A: u64 = 1 << 6;
    /// D (Dirty) - bit 7
    pub const D: u64 = 1 << 7;

    /// Create empty page table entry
    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    /// Create from bits
    #[inline]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Get bits value
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    /// Check if valid
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.0 & Self::V != 0
    }

    /// Check if readable
    #[inline]
    pub fn is_readable(&self) -> bool {
        self.0 & Self::R != 0
    }

    /// Check if writable
    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & Self::W != 0
    }

    /// Check if executable
    #[inline]
    pub fn is_executable(&self) -> bool {
        self.0 & Self::X != 0
    }

    /// Check if user page
    #[inline]
    pub fn is_user(&self) -> bool {
        self.0 & Self::U != 0
    }

    /// Check if leaf entry (mapped page, not pointer to next level)
    /// A leaf entry has at least one of R, W, or X bits set
    #[inline]
    pub fn is_leaf(&self) -> bool {
        (self.0 & (Self::R | Self::W | Self::X)) != 0
    }

    /// Get physical page number（PPN，bits [53:10]）
    #[inline]
    pub fn ppn(&self) -> u64 {
        (self.0 >> 10) & 0x00FFFFFFFFFFFFFF
    }

    /// Create PTE pointing to next level page table
    #[inline]
    pub fn new_table(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V)
    }

    /// Create PTE pointing to physical page (kernel permission)
    #[inline]
    pub fn new_page_kernel(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::W | Self::X | Self::A | Self::D)
    }

    /// Create PTE pointing to physical page (user permission)
    #[inline]
    pub fn new_page_user(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::W | Self::X | Self::U | Self::A | Self::D)
    }

    /// Create PTE pointing to physical page (read-only)
    #[inline]
    pub fn new_page_ro(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::X | Self::A)
    }
}

impl Default for PageTableEntry {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Page table ====================

#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Create new page table (zeroed)
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::new(); 512],
        }
    }

    /// Get page table entry
    #[inline]
    pub fn get(&self, index: usize) -> PageTableEntry {
        self.entries[index]
    }

    /// Set page table entry
    #[inline]
    pub fn set(&mut self, index: usize, entry: PageTableEntry) {
        self.entries[index] = entry;
    }

    /// Clear page table (set all PTEs to 0)
    pub fn zero(&mut self) {
        for i in 0..512 {
            self.entries[i] = PageTableEntry::new();
        }
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== satp CSR ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Satp(pub u64);

impl Satp {
    /// Bare (No address translation)
    pub const MODE_BARE: u64 = 0;

    /// Sv39 (39-bit virtual address)
    pub const MODE_SV39: u64 = 8;

    /// Create satp value
    #[inline]
    pub const fn new(mode: u64, asid: u16, ppn: u64) -> Self {
        Self(((mode as u64) << 60) | ((asid as u64) << 44) | (ppn & 0x0FFFFFFFFFFFFFFF))
    }

    /// Create Sv39 satp
    #[inline]
    pub const fn sv39(ppn: u64, asid: u16) -> Self {
        Self::new(Self::MODE_SV39, asid, ppn)
    }

    /// Get bits value
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    /// Get mode
    #[inline]
    pub fn mode(&self) -> u64 {
        self.0 >> 60
    }

    /// Check if Bare mode (MMU disabled)
    #[inline]
    pub fn is_bare(&self) -> bool {
        self.mode() == Self::MODE_BARE
    }

    /// Check if Sv39 mode
    #[inline]
    pub fn is_sv39(&self) -> bool {
        self.mode() == Self::MODE_SV39
    }
}

// ==================== Address Space ====================
//
// Note: AddressSpace is now defined in kernel/src/mm/mm_struct.rs
// Here only contains architecture-specific extension methods

extern crate alloc;
use alloc::vec::Vec;

use crate::mm::vma::{Vma, VmaFlags, VmaType};
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::page::{VirtAddr as PageVirtAddr, PAGE_SIZE as PAGE_SIZE_USIZE};

// Re-export MmStruct and AddressSpace so other modules can access them through arch module
pub use crate::mm::{MmStruct, AddressSpace};

// ==================== Architecture-specific MmStruct extension methods ====================

impl MmStruct {
    /// Enable this address space (switch page table)
    pub unsafe fn enable(&self) {
        let satp = Satp::sv39(self.pgd, 0);
        asm!("csrw satp, {}", in(reg) satp.bits());
        asm!("sfence.vma zero, zero");
    }

    /// Disable address space (switch to bare mode)
    pub unsafe fn disable() {
        let satp = Satp::new(Satp::MODE_BARE, 0, 0);
        asm!("csrw satp, {}", in(reg) satp.bits());
        asm!("sfence.vma zero, zero");
    }

    /// Flush entire TLB
    pub unsafe fn flush_tlb() {
        asm!("sfence.vma zero, zero");
    }

    /// Flush TLB for specified page
    pub unsafe fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        asm!("sfence.vma {}, zero", in(reg) vaddr.as_usize());
    }

    // ==================== VMA Operations ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages. Pages are mapped on first access through page fault handling.
    /// This avoids TLB flush issues.
    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();

        let start = vma.start();
        let end = vma.end();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // Save permission info to VMA (for page fault handling)
        // VMA already has flags, we don't need extra storage

        // Update virtual memory statistics
        let pages = ((end.as_usize() - start.as_usize()) / PAGE_SIZE_USIZE) as u64;
        self.add_total_vm(pages);
        self.update_highest_vm_end(end.as_usize());

        // Don't pre-map pages, use lazy mapping
        // Pages will be mapped on first access through page fault handling

        Ok(())
    }

    /// Map single page (for lazy mapping/page fault handling)
    pub fn map_single_page(&self, virt_addr: VirtAddr, perm: Perm) -> Result<(), MapError> {
        use core::sync::atomic::fence;
        use core::sync::atomic::Ordering;

        // Allocate physical page
        let phys_addr = alloc_user_phys_page().ok_or(MapError::OutOfMemory)? as usize;
        let flags = perm_to_flags(perm, self.space_type());

        // Zero physical page using linear mapping
        unsafe {
            let ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(ptr.bits() as *mut u8, 0, PAGE_SIZE_USIZE);
            fence(Ordering::SeqCst);
        }

        // Map page
        unsafe {
            map_page(
                self.pgd,
                virt_addr,
                PhysAddr::new(phys_addr as u64),
                flags,
            );
        }

        Ok(())
    }

    /// Unmap VMA (requires write lock)
    pub fn unmap_vma(&self, start: PageVirtAddr) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();

        let vma = vma_mgr.find(start).ok_or(MapError::NotMapped)?;
        let _end = vma.end();
        let _ = vma_mgr.remove(start);
        // TODO: Actually unmap page table entry
        Ok(())
    }

    /// Adjust heap pointer (requires write lock)
    pub fn set_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        use crate::mm;

        if new_brk.as_usize() == 0 {
            return Ok(self.brk());
        }

        if self.space_type() != PageTableType::User {
            return Err(MapError::Invalid);
        }

        // Heap region: use constants defined in user_addr module
        use user_addr::{HEAP_START, HEAP_MAX_SIZE, BRK_DEFAULT, MMAP_START};
        let heap_end = BRK_DEFAULT + HEAP_MAX_SIZE;  // brk can grow up to here

        if new_brk.as_usize() < HEAP_START || new_brk.as_usize() > heap_end.min(MMAP_START) {
            return Ok(self.brk());
        }

        let old_brk = self.brk().as_usize();

        if new_brk.as_usize() < old_brk {
            self.set_brk_val(new_brk.as_usize());
            return Ok(new_brk);
        }

        if new_brk.as_usize() > old_brk {
            let old_brk_aligned = old_brk & !(PAGE_SIZE_USIZE - 1);
            let new_brk_aligned = new_brk.as_usize() & !(PAGE_SIZE_USIZE - 1);

            let mut addr = old_brk_aligned;
            while addr < new_brk_aligned {
                if unsafe { PageTableWalker::walk(self.pgd, addr as u64) }.is_none() {
                    // Use zone allocator instead of legacy frame allocator
                    let phys_addr = alloc_pages(GfpFlags::GFP_KERNEL, 0);
                    if phys_addr == 0 {
                        return Err(MapError::OutOfMemory);
                    }
                    let flags = perm_to_flags(Perm::ReadWrite, self.space_type());
                    unsafe {
                        map_page(
                            self.pgd,
                            VirtAddr::new(addr as u64),
                            PhysAddr::new(phys_addr as u64),
                            flags,
                        );
                    }

                    // Add VMA inside write lock
                    let mut vma_mgr = self.vma_write();
                    let mut vma_flags = VmaFlags::new();
                    vma_flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSUP);
                    let vma = Vma::new(
                        PageVirtAddr::new(addr),
                        PageVirtAddr::new(addr + PAGE_SIZE_USIZE),
                        vma_flags,
                    );
                    let _ = vma_mgr.add(vma);
                }
                addr += PAGE_SIZE_USIZE;
            }

            self.set_brk_val(new_brk.as_usize());
        }

        Ok(new_brk)
    }

    /// mmap system call implementation
    ///
    ///
    /// # Arguments
    /// - `addr`: Suggested start address (0 means kernel chooses)
    /// - `size`: Mapping length
    /// - `flags`: VMA flags
    /// - `vma_type`: VMA type
    /// - `perm`: Page permissions
    /// - `map_flags`: mmap flags (MAP_FIXED, etc.)
    ///
    /// # Returns
    /// Returns mapped start address on success, MapError on failure
    pub fn mmap(
        &self,
        addr: PageVirtAddr,
        size: usize,
        flags: VmaFlags,
        vma_type: VmaType,
        perm: Perm,
        map_flags: u32,
    ) -> Result<PageVirtAddr, MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        // Check MAP_FIXED
        let is_fixed = map_flags & map::MAP_FIXED != 0;

        // Check brk region conflict for all cases
        use user_addr::BRK_DEFAULT;
        use user_addr::MMAP_START;
        let end_addr = addr.as_usize() + aligned_size;
        let has_brk_conflict = addr.as_usize() < MMAP_START && end_addr > BRK_DEFAULT;

        // Determine mapping start address
        let start = if is_fixed {
            // MAP_FIXED: Force using specified address
            let start = addr;
            // Check address alignment
            if start.as_usize() % PAGE_SIZE_USIZE != 0 {
                return Err(MapError::Invalid);
            }
            // Check address range
            if start.as_usize() < user_addr::USER_START {
                return Err(MapError::Invalid);
            }
            // MAP_FIXED with brk conflict is not allowed
            if has_brk_conflict {
                return Err(MapError::Invalid);
            }
            start
        } else if addr.as_usize() == 0 {
            // Address is 0, let kernel choose appropriate address
            self.find_free_area(aligned_size)?
        } else {
            // Try using suggested address, if conflict then find another address
            let end = PageVirtAddr::new(addr.as_usize() + aligned_size);
            let test_vma = Vma::new(addr, end, flags);

            // Check if conflicts with existing VMA
            let vma_mgr = self.vma_read();
            let has_vma_conflict = vma_mgr.iter().any(|v| v.overlaps(&test_vma));
            drop(vma_mgr);

            // Check if conflicts with brk region
            // brk region starts from BRK_DEFAULT, grows upward
            // We assume brk can grow up to MMAP_START
            use user_addr::BRK_DEFAULT;
            use user_addr::MMAP_START;
            let has_brk_conflict = addr.as_usize() < MMAP_START && addr.as_usize() >= BRK_DEFAULT;

            if has_vma_conflict || has_brk_conflict {
                self.find_free_area(aligned_size)?
            } else {
                addr
            }
        };

        // MAP_FIXED: Need to unmap existing pages first
        if is_fixed {
            // Iterate and remove conflicting VMAs
            let mut vma_mgr = self.vma_write();
            let mut vmas_to_remove = Vec::new();
            for vma in vma_mgr.iter() {
                if vma.overlaps(&Vma::new(start, PageVirtAddr::new(start.as_usize() + aligned_size), flags)) {
                    vmas_to_remove.push(vma.start());
                }
            }
            drop(vma_mgr);

            // Remove VMAs
            for vma_start in vmas_to_remove {
                let mut vma_mgr = self.vma_write();
                let _ = vma_mgr.remove(vma_start);
            }

            // Clear page mappings
            let mut addr = start.as_usize();
            while addr < start.as_usize() + aligned_size {
                unsafe {
                    self.clear_pte(addr as u64);
                }
                addr += PAGE_SIZE_USIZE;
            }

            // Flush TLB
            unsafe {
                core::arch::asm!("sfence.vma zero, zero");
            }
        }

        let end = PageVirtAddr::new(start.as_usize() + aligned_size);
        let mut vma = Vma::new(start, end, flags);
        vma.set_type(vma_type);
        self.map_vma(vma, perm)?;
        Ok(start)
    }

    /// Find free virtual address area
    ///
    fn find_free_area(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        use user_addr::{MMAP_START, MMAP_END, USER_END};

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        let vma_mgr = self.vma_read();

        // Start searching from mmap area
        let mut search_start = MMAP_START;
        let search_end = MMAP_END.min(USER_END - aligned_size);

        // Iterate existing VMAs, find gaps
        for vma in vma_mgr.iter() {
            let vma_start = vma.start().as_usize();

            // If current VMA start address is within search range
            if vma_start > search_start {
                // Check if gap is large enough
                let gap_size = vma_start - search_start;
                if gap_size >= aligned_size {
                    return Ok(PageVirtAddr::new(search_start));
                }
            }

            // Update search start to current VMA end address
            if vma.end().as_usize() > search_start {
                search_start = (vma.end().as_usize() + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
            }

            // Check if exceeded search range
            if search_start > search_end {
                break;
            }
        }

        // Check last gap
        if search_start <= search_end && (search_end - search_start) >= aligned_size {
            return Ok(PageVirtAddr::new(search_start));
        }

        Err(MapError::OutOfMemory)
    }

    /// munmap system call implementation
    ///
    ///
    /// # Arguments
    /// - `addr`: Start address to unmap
    /// - `size`: Size to unmap
    ///
    /// # Returns
    /// Returns Ok(()) on success, MapError on failure
    pub fn munmap(&self, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        // Check address alignment
        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        let end_addr = addr.as_usize() + aligned_size;

        // Find and delete corresponding VMA
        {
            let vma_mgr = self.vma_read();

            // Find VMA containing start address, get necessary info
            let vma_info = vma_mgr.find(addr).map(|vma| {
                (vma.start(), vma.end())
            });
            drop(vma_mgr);  // Release read lock

            if let Some((vma_start, vma_end)) = vma_info {
                let vma_start_usize = vma_start.as_usize();
                let vma_end_usize = vma_end.as_usize();

                // Check if completely covers VMA
                if addr.as_usize() <= vma_start_usize && end_addr >= vma_end_usize {
                    // Complete unmap
                    let mut vma_mgr = self.vma_write();
                    vma_mgr.remove(vma_start)?;
                } else if addr.as_usize() > vma_start_usize && end_addr < vma_end_usize {
                    // Partial unmap (middle part) - need to split VMA
                    // TODO: Implement VMA splitting
                    return Err(MapError::Invalid);
                } else {
                    // Partial overlap
                    return Err(MapError::Invalid);
                }
            }
        }

        // Unmap physical pages
        self.unmap_pages(addr, aligned_size)?;

        Ok(())
    }

    /// Unmap physical pages in specified range
    fn unmap_pages(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let mut addr = start.as_usize();
        let end = addr + size;

        while addr < end {
            // Find page table entry
            let ppn = unsafe { PageTableWalker::walk(self.pgd, addr as u64) };

            if let Some(ppn) = ppn {
                // Free physical page (if reference count is 1)
                // TODO: Implement proper page reference counting
                let _ = ppn; // Ignore for now

                // Clear page table entry
                unsafe {
                    self.clear_pte(addr as u64);
                }
            }

            addr += PAGE_SIZE_USIZE;
        }

        // Flush TLB
        unsafe {
            core::arch::asm!("sfence.vma zero, zero");
        }

        Ok(())
    }

    /// Clear page table entry at specified virtual address
    unsafe fn clear_pte(&self, virt: u64) {
        let vpn2 = ((virt >> 30) & 0x1FF) as usize;
        let vpn1 = ((virt >> 21) & 0x1FF) as usize;
        let vpn0 = ((virt >> 12) & 0x1FF) as usize;

        let root_table = get_page_table_virt(self.pgd << PAGE_SHIFT);

        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() {
            return;
        }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() {
            return;
        }

        let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);

        // Clear page table entry
        (*table0).set(vpn0, PageTableEntry::from_bits(0));
    }

    /// brk system call implementation (legacy interface)
    pub fn do_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        self.set_brk(new_brk)
    }

    /// Allocate stack space
    pub fn allocate_stack(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let stack_size = if size == 0 {
            user_addr::STACK_MAX_SIZE
        } else {
            size
        };
        let aligned_size = (stack_size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        // Use TASK_SIZE as stack top (Linux-compatible: stack grows down from TASK_SIZE)
        let stack_top = PageVirtAddr::new(user_addr::STACK_TOP & !(PAGE_SIZE_USIZE - 1));
        let stack_start = PageVirtAddr::new(stack_top.as_usize() - aligned_size);

        let mut flags = VmaFlags::new();
        flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN);
        let vma = Vma::new(stack_start, stack_top, flags);
        self.map_vma(vma, Perm::ReadWrite)?;

        // Set stack layout
        self.setup_stack(stack_top.as_usize(), stack_size);

        Ok(stack_top)
    }

    /// Copy address space using Copy-on-Write mechanism
    ///
    /// Use COW to mark writable pages, avoiding immediate copying of all physical pages
    pub fn fork(&self) -> Result<MmStruct, MapError> {
        // Copy using COW page table
        let new_root_ppn = unsafe {
            copy_page_table_cow(self.pgd).ok_or(MapError::OutOfMemory)?
        };

        let new_space = unsafe { MmStruct::new_shared(
            new_root_ppn,
            self.space_type(),
            self.brk(),
        ) };

        // Copy VMA to child process
        // Since they are different MmStruct, VMA locks will not conflict
        {
            let vma_mgr = self.vma_read();
            if vma_mgr.iter().count() > 0 {
                let mut new_vma_mgr = new_space.vma_write();
                for vma in vma_mgr.iter() {
                    let new_vma = Vma::new(vma.start(), vma.end(), vma.flags());
                    let _ = new_vma_mgr.add(new_vma);
                }
            }
        }

        // Copy segment layout
        new_space.set_start_code(self.start_code());
        new_space.set_end_code(self.end_code());
        new_space.set_start_data(self.start_data());
        new_space.set_end_data(self.end_data());
        new_space.set_start_stack(self.start_stack());
        new_space.set_stack_limit(self.stack_limit());  // Copy stack limit for expansion
        new_space.set_arg_start(self.arg_start());
        new_space.set_arg_end(self.arg_end());
        new_space.set_env_start(self.env_start());
        new_space.set_env_end(self.env_end());

        Ok(new_space)
    }
}

fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
    let mut flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    match perm {
        Perm::None => {
            // PROT_NONE: In RISC-V, V=1 and R=W=X=0 is a non-leaf PTE
            // To create an inaccessible mapping, we set it as read-only but disallow access
            // Simplified here: set as read-only, actual access will be handled by page fault
            flags |= PageTableEntry::R;  // Must set at least one permission bit to be a valid leaf PTE
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
    }
    if space_type == PageTableType::User {
        flags |= PageTableEntry::U;
    }
    flags
}

// ==================== MMU Initialization ====================

#[link_section = ".bss"]
pub static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[link_section = ".bss"]
static mut TRAP_STACKS: [[u8; 16384]; 4] = [[0; 16384]; 4];  // 4 CPUs

pub unsafe fn get_trap_stack() -> u64 {
    let cpu_id = crate::arch::riscv64::smp::cpu_id() as usize;
    if cpu_id >= 4 {
        panic!("mm: Invalid CPU ID {}", cpu_id);
    }
    let stack_base = &mut TRAP_STACKS[cpu_id] as *mut [u8; 16384] as *mut u8;
    stack_base.add(16384) as u64  // stack top
}

// ============================================================================
// Page Table Allocation (Linux-style three-stage approach)
// ============================================================================
//
// Linux uses three stages for page table allocation:
// 1. Early: Static arrays (MMU not enabled yet, identity mapping)
// 2. Fixmap: memblock allocation (MMU enabled, but buddy not ready)
// 3. Late: Buddy allocator (full memory management available)
//
// Key insight: Early stage only needs a few page tables to map the kernel
// itself. After MMU is enabled, we can use memblock for dynamic allocation.

/// Number of early page tables (Linux uses just a few for kernel mapping)
/// For Sv39 3-level page tables:
/// - 1 L1 page table can map 512 * 512 * 4KB = 1GB of virtual space
/// - Each L0 page table covers 2MB of virtual space
///
/// Rux's early mappings:
/// - Kernel: 8MB (needs 4 L0 tables)
/// - Heap: 32MB (needs 16 L0 tables)
/// - Slab: 4MB (needs 2 L0 tables)
/// - Gap: 18MB (needs 9 L0 tables)
/// - Devices: ~1MB (needs a few L0 tables)
/// Total: ~31 L0 tables + 2-3 L1 tables
///
/// We provide a bit more to be safe
const NUM_EARLY_PMD: usize = 4;  // L1 page tables (covers 4GB virtual space)
const NUM_EARLY_PTE: usize = 48; // L0 page tables (covers 96MB mapped space)

/// Early page tables for boot (like Linux's early_pmd, early_pte)
/// These are placed in .bss and use identity mapping
#[link_section = ".bss"]
static mut EARLY_PMD: [PageTable; NUM_EARLY_PMD] = [PageTable::new(); NUM_EARLY_PMD];
#[link_section = ".bss"]
static mut EARLY_PTE: [PageTable; NUM_EARLY_PTE] = [PageTable::new(); NUM_EARLY_PTE];

/// Counter for early page table allocation
static EARLY_PMD_NEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static EARLY_PTE_NEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Allocation stage tracking
#[derive(Debug, Clone, Copy, PartialEq)]
enum AllocStage {
    /// Early boot: MMU not fully enabled, use static arrays with identity mapping
    Early,
    /// Fixmap stage: MMU enabled, use memblock allocation
    Fixmap,
    /// Late stage: Buddy allocator ready, use normal page allocation
    Late,
}

/// Current allocation stage
static ALLOC_STAGE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(AllocStage::Early as u8);

impl AllocStage {
    const fn from_u8(v: u8) -> Self {
        match v {
            0 => AllocStage::Early,
            1 => AllocStage::Fixmap,
            2 => AllocStage::Late,
            _ => AllocStage::Late,
        }
    }
}

/// Get current allocation stage
fn get_alloc_stage() -> AllocStage {
    AllocStage::from_u8(ALLOC_STAGE.load(core::sync::atomic::Ordering::Acquire))
}

/// Transition to fixmap stage (MMU enabled, can use memblock)
/// Called after MMU is enabled and linear mapping is set up
pub fn pt_ops_set_fixmap() {
    ALLOC_STAGE.store(AllocStage::Fixmap as u8, core::sync::atomic::Ordering::Release);
}

/// Transition to late stage (buddy allocator ready)
/// Called after frame allocator is initialized
pub fn pt_ops_set_late() {
    ALLOC_STAGE.store(AllocStage::Late as u8, core::sync::atomic::Ordering::Release);
}

/// Check if frame allocator is ready (for backward compatibility)
#[inline]
fn is_frame_allocator_ready() -> bool {
    get_alloc_stage() == AllocStage::Late
}

/// Allocate a page table and return its physical address
/// Three-stage allocation like Linux:
/// - Early: static arrays (identity mapped)
/// - Fixmap: memblock allocation (linear mapped)
/// - Late: buddy allocator (linear mapped)
pub unsafe fn alloc_page_table() -> Option<u64> {
    let stage = get_alloc_stage();
    match stage {
        AllocStage::Early => {
            // Early boot: use static arrays with identity mapping
            // Try PMD first (for L1), then PTE (for L0)
            let pmd_idx = EARLY_PMD_NEXT.load(core::sync::atomic::Ordering::Acquire);
            if pmd_idx < NUM_EARLY_PMD {
                EARLY_PMD_NEXT.store(pmd_idx + 1, core::sync::atomic::Ordering::Release);
                let table_ptr = &EARLY_PMD[pmd_idx] as *const PageTable as u64;
                core::ptr::write_bytes(table_ptr as *mut u8, 0, PAGE_SIZE as usize);
                return Some(table_ptr);
            }

            let pte_idx = EARLY_PTE_NEXT.load(core::sync::atomic::Ordering::Acquire);
            if pte_idx < NUM_EARLY_PTE {
                EARLY_PTE_NEXT.store(pte_idx + 1, core::sync::atomic::Ordering::Release);
                let table_ptr = &EARLY_PTE[pte_idx] as *const PageTable as u64;
                core::ptr::write_bytes(table_ptr as *mut u8, 0, PAGE_SIZE as usize);
                return Some(table_ptr);
            }

            panic!("mm: Out of early page tables (PMD: {}, PTE: {})", pmd_idx, pte_idx);
        }
        AllocStage::Fixmap => {
            // Fixmap stage: use memblock allocation
            let phys_addr = crate::mm::memblock::memblock_phys_alloc()?;

            // Use identity mapping only for actually identity-mapped regions
            // Early boot maps: 0x80200000 - 0x84000000 (kernel + heap + slab + gap)
            let virt_addr = if phys_addr >= 0x80200000 && phys_addr < 0x84000000 {
                phys_addr as *mut u8  // Identity mapping
            } else {
                // For other regions, use linear mapping
                // This should work after setup_linear_mapping() completes
                phys_to_virt(PhysAddr::new(phys_addr as u64)).bits() as *mut u8
            };
            core::ptr::write_bytes(virt_addr, 0, PAGE_SIZE as usize);
            Some(phys_addr as u64)
        }
        AllocStage::Late => {
            // Late stage: use zone allocator exclusively
            // IMPORTANT: pt_ops_set_late() must be called AFTER init_zone_system()
            use crate::mm::zone::ZoneType;

            // Allocate from ZONE_NORMAL
            let phys_addr = if let Some(node) = crate::mm::pglist::first_online_node_mut() {
                if let Some(zone) = node.zone_mut(ZoneType::ZoneNormal) {
                    if zone.is_initialized() {
                        if let Some(pfn) = zone.alloc_pages(0) {
                            // Update page descriptor
                            let page = crate::mm::page_desc::pfn_to_page_mut(pfn);
                            if !page.is_null() {
                                unsafe {
                                    (*page).set_refcount(1);
                                    (*page).set_order(0);
                                    (*page).set_flag(crate::mm::page_desc::PageFlag::Referenced);
                                }
                            }
                            crate::mm::zone::pfn_to_phys(pfn) as u64
                        } else {
                            0
                        }
                    } else {
                        0
                    }
                } else {
                    0
                }
            } else {
                0
            };

            if phys_addr == 0 {
                return None;
            }

            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            core::ptr::write_bytes(virt_addr.bits() as *mut u8, 0, PAGE_SIZE as usize);
            Some(phys_addr)
        }
    }
}

/// Get virtual address for accessing a page table given its physical address
#[inline]
pub unsafe fn get_page_table_virt(phys_addr: u64) -> *mut PageTable {
    match get_alloc_stage() {
        AllocStage::Early => {
            // Identity mapping during early boot
            phys_addr as *mut PageTable
        }
        AllocStage::Fixmap => {
            // Use identity mapping only for actually identity-mapped regions
            // Early boot maps: 0x80200000 - 0x84000000 (kernel + heap + slab + gap)
            if phys_addr >= 0x80200000 && phys_addr < 0x84000000 {
                phys_addr as *mut PageTable  // Identity mapping
            } else {
                // For other regions, use linear mapping
                let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
                virt_addr.bits() as *mut PageTable
            }
        }
        AllocStage::Late => {
            // Use linear mapping after buddy allocator is ready
            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            virt_addr.bits() as *mut PageTable
        }
    }
}

/// Free a page table (only valid for late stage allocations)
unsafe fn free_page_table(phys_addr: u64) {
    if get_alloc_stage() != AllocStage::Late {
        return;
    }

    // Check if it's from early static region (shouldn't happen, but be safe)
    let early_pmd_start = &EARLY_PMD as *const _ as u64;
    let early_pmd_end = early_pmd_start + (NUM_EARLY_PMD * PAGE_SIZE as usize) as u64;
    let early_pte_start = &EARLY_PTE as *const _ as u64;
    let early_pte_end = early_pte_start + (NUM_EARLY_PTE * PAGE_SIZE as usize) as u64;

    if (phys_addr >= early_pmd_start && phys_addr < early_pmd_end) ||
       (phys_addr >= early_pte_start && phys_addr < early_pte_end) {
        // Don't free early page tables
        return;
    }

    // Use zone allocator to free (same as alloc_page_table uses)
    crate::mm::page_alloc::free_pages(phys_addr as usize, 0);
}

/// Free all page tables and user data pages used by a user address space
/// Called when process exits or execve replaces address space
pub unsafe fn free_user_page_tables(root_ppn: u64) {
    use crate::mm::{pfn_to_page, phys_to_pfn, phys_valid, page_desc::PageFlag};

    let root_phys = root_ppn << PAGE_SHIFT;
    let root_table = get_page_table_virt(root_phys);

    // Walk and free all levels (only user space: VPN2 0-255)
    for vpn2 in 0..256 {
        let pte2 = (*root_table).get(vpn2);
        if pte2.is_valid() {
            // Check if L2 is a leaf (1GB huge page)
            if pte2.is_leaf() {
                // 1GB huge page - free user data page
                let phys_addr = pte2.ppn() << PAGE_SHIFT;
                let pfn = phys_to_pfn(phys_addr as usize);
                let page = pfn_to_page(pfn);
                if !page.is_null() {
                    if (*page).test_flag(PageFlag::Cow) {
                        (*page).put_page();
                    } else {
                        free_pages(phys_addr as usize, 0);
                    }
                }
                continue;
            }

            let ppn1 = pte2.ppn();
            let table1_phys = ppn1 << PAGE_SHIFT;

            // Validate L1 table physical address is within valid RAM range
            if !phys_valid(table1_phys as usize) {
                continue;
            }

            // Detect circular reference
            if table1_phys == root_phys {
                continue;
            }

            let table1 = get_page_table_virt(table1_phys);

            for vpn1 in 0..512 {
                let pte1 = (*table1).get(vpn1);
                if pte1.is_valid() {
                    // Check if L1 is a leaf (2MB huge page)
                    if pte1.is_leaf() {
                        // 2MB huge page - free user data page
                        let phys_addr = pte1.ppn() << PAGE_SHIFT;
                        let pfn = phys_to_pfn(phys_addr as usize);
                        let page = pfn_to_page(pfn);
                        if !page.is_null() {
                            if (*page).test_flag(PageFlag::Cow) {
                                (*page).put_page();
                            } else {
                                free_pages(phys_addr as usize, 0);
                            }
                        }
                        continue;
                    }

                    let ppn0 = pte1.ppn();
                    let table0_phys = ppn0 << PAGE_SHIFT;

                    if !phys_valid(table0_phys as usize) {
                        continue;
                    }

                    let table0 = get_page_table_virt(table0_phys);

                    for vpn0 in 0..512 {
                        let pte0 = (*table0).get(vpn0);
                        if pte0.is_valid() && pte0.is_leaf() {
                            // 4KB page - free user data page
                            let phys_addr = pte0.ppn() << PAGE_SHIFT;
                            let pfn = phys_to_pfn(phys_addr as usize);
                            let page = pfn_to_page(pfn);

                            // Skip invalid pages (outside valid memory range or null)
                            if page.is_null() || phys_addr < 0x80000000 {
                                continue;
                            }

                            if (*page).test_flag(PageFlag::Cow) {
                                (*page).put_page();
                            } else {
                                free_pages(phys_addr as usize, 0);
                            }
                        }
                    }
                    // Free L0 table
                    free_page_table(table0_phys);
                }
            }
            // Free L1 table
            free_page_table(table1_phys);
        }
    }

    // Free root table (L2)
    free_page_table(root_phys);
}

pub unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    // Extract virtual page numbers（VPN2, VPN1, VPN0）
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // Get root page table (L2) using linear mapping
    let root_table_addr = root_ppn << PAGE_SHIFT;
    let root_table = get_page_table_virt(root_table_addr);
    let root = &mut *root_table;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_page: failed to allocate L1 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 1 -> Level 0
    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    let table1_ref = &mut *table1;
    let pte1 = table1_ref.get(vpn1);
    let ppn0 = if pte1.is_valid() {
        pte1.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_page: failed to allocate L0 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        table1_ref.set(vpn1, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 0 -> Physical page
    let table0_phys = ppn0 << PAGE_SHIFT;
    let table0 = get_page_table_virt(table0_phys);
    let table0_ref = &mut *table0;
    let ppn: u64 = phys_addr >> PAGE_SHIFT;
    let pte_bits: u64 = (ppn << 10) | flags;

    table0_ref.set(vpn0, PageTableEntry::from_bits(pte_bits));

    // Flush TLB
    core::arch::asm!("sfence.vma");
}

unsafe fn map_region(root_ppn: u64, start: u64, size: u64, flags: u64) {
    let virt_start = VirtAddr::new(start);
    let phys_start = PhysAddr::new(start);
    let virt_end = VirtAddr::new(start + size);

    let mut virt = virt_start.floor();
    let end = virt_end.ceil();

    while virt.bits() < end.bits() {
        // Use identity mapping: virtual address = physical address
        let offset = virt.bits() - virt_start.bits();
        let phys = PhysAddr::new(phys_start.bits() + offset);
        map_page(root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
}

pub fn init() {
    unsafe {
        // Read current satp value
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);

        // Check if MMU is already enabled (fast path)
        if satp >> 60 != 0 {
            // MMU already enabled, return directly
            return;
        }

        // Try to acquire initialization lock (using CAS operation)
        // Only the first core to reach here can successfully set false -> true
        if !MMU_INITIALIZED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            // Other cores are initializing or initialized, waiting for completion
            while !MMU_INITIALIZED.load(Ordering::Acquire) {
                // Brief delay
                asm!("nop", options(nomem, nostack));
            }

            // Boot core has completed page table initialization, secondary cores now need to enable their MMU
            // Calculate root page table physical page number (using same page table as boot core)
            let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

            let addr_space = MmStruct::new_kernel(root_ppn);
            addr_space.enable();

            return;
        }

        // Only boot core will execute here

        // Initialize root page table (zero out)
        ROOT_PAGE_TABLE.zero();

        // Calculate root page table physical page number
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        // Map kernel space (KERNEL_ENTRY - HEAP_START, 8MB)
        // QEMU virt: kernel starts at KERNEL_ENTRY
        // Increase mapping size to avoid memory layout changes due to code growth
        let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, KERNEL_ENTRY, KERNEL_SIZE, kernel_flags);

        // Map heap space (starts at HEAP_START, size determined by config)
        // For dynamic memory allocation (Buddy System)
        // Use identity mapping: virtual = physical
        // Note: This ensures virt_to_phys() correctly converts VirtQueue DMA addresses
        let heap_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        let heap_virt_start = HEAP_START;
        let heap_phys_start = HEAP_START;  // identity mapping
        let heap_size = crate::config::KERNEL_HEAP_SIZE as u64;

        let virt_start = VirtAddr::new(heap_virt_start);
        let phys_start = PhysAddr::new(heap_phys_start);
        let virt_end = VirtAddr::new(heap_virt_start + heap_size);
        let mut virt = virt_start.floor();
        let end = virt_end.ceil();

        while virt.bits() < end.bits() {
            let offset = virt.bits() - virt_start.bits();
            let phys = PhysAddr::new(phys_start.bits() + offset);
            map_page(root_ppn, virt, phys, heap_flags);
            virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
        }

        // Map Slab allocator area (after heap, 4MB)
        // Slab start address = heap end address
        let slab_virt_start = HEAP_START + crate::config::KERNEL_HEAP_SIZE as u64;
        let slab_size = 4 * 1024 * 1024u64; // 4MB
        map_region(root_ppn, slab_virt_start, slab_size, heap_flags);

        // Map the gap between slab and physical memory region 1 (for vmemmap and other uses)
        // This region: 0x82E00000 - 0x84000000 (18MB)
        let gap_start = slab_virt_start + slab_size;  // 0x82E00000
        let gap_end = 0x84000000u64;  // End of gap
        let gap_size = gap_end - gap_start;    // 0x84000000 - 0x82E00000 = 0x1200000 (18MB)
        map_region(root_ppn, gap_start, gap_size, heap_flags);

        // Note: Linear mapping is set up later in setup_linear_mapping()
        // after memblock is initialized with memory regions from device tree.

        // Map UART device (essential for early console output)
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, UART_BASE, 0x1000, device_flags);

        // Map DTB area (essential for parsing device tree)
        // Map 1MB is enough for DTB
        map_region(root_ppn, DTB_BASE, 0x100000, device_flags);

        // Note: Large device mappings (PLIC, CLINT, VirtIO, PCIe) are deferred
        // to setup_device_mappings() which is called after fixmap stage is ready.
        // This avoids consuming too many early page tables.

        // Initialize UART fixmap (map UART to fixmap virtual address)
        super::fixmap::init_uart_fixmap();

        // Enable MMU
        let addr_space = MmStruct::new_kernel(root_ppn);
        addr_space.enable();
    }
}

/// Setup device mappings (called after fixmap stage is ready)
///
/// This function maps large device regions that would consume too many
/// early page tables. It should be called after pt_ops_set_fixmap().
pub fn setup_device_mappings() {
    unsafe {
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;

        // Map VirtIO device MMIO area (1MB)
        map_region(root_ppn, VIRTIO_MMIO_BASE, 0x100000, device_flags);

        // Map PLIC (Platform-Level Interrupt Controller, 2MB)
        map_region(root_ppn, PLIC_BASE, 0x200000, device_flags);

        // Map CLINT (Core Local Interruptor, 64KB)
        map_region(root_ppn, CLINT_BASE, 0x10000, device_flags);

        // Map PCIe ECAM space (1MB)
        map_region(root_ppn, PCIE_ECAM_BASE, 0x100000, device_flags);

        // Map PCI MMIO space (256MB)
        // This is a large mapping but now we can use memblock allocation
        map_region(root_ppn, PCI_MMIO_BASE, 0x10000000, device_flags);
    }
}

/// Setup linear mapping for physical memory (Linux-style)
///
/// This function creates the linear mapping (also called direct mapping) that maps
/// all physical memory to the PAGE_OFFSET virtual address region.
///
/// Linux approach (from arch/riscv/mm/init.c):
/// - for_each_mem_range() iterates over all memory regions
/// - create_linear_mapping_range() maps each region with va = __va(pa)
/// - __va(pa) = pa + va_pa_offset = pa + (PAGE_OFFSET - phys_ram_base)
/// - Uses best_map_size() to select optimal page size (2MB when aligned)
///
/// This function should be called AFTER memblock is initialized with memory
/// regions from the device tree.
///
/// # Arguments
/// - `memory_regions`: Slice of MemoryRegion representing physical memory regions
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    unsafe {
        // Linear mapping flags: RW, accessed, dirty
        let linear_flags = PageTableEntry::V | PageTableEntry::R |
                          PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;

        // Map each memory region
        for region in memory_regions {
            let phys_start = region.base;
            let size = region.size;
            let phys_end = phys_start + size;

            // Calculate virtual addresses using Linux formula:
            // va = pa + va_pa_offset = pa + (PAGE_OFFSET - PHYS_MEMORY_BASE)
            let virt_start = phys_start + VA_PA_OFFSET;

            // Map using best_map_size (Linux-style)
            let mut phys = phys_start;
            let mut virt = virt_start;

            while phys < phys_end {
                let remaining = phys_end - phys;
                let map_size = best_map_size(phys, virt, remaining);

                if map_size == PMD_SIZE as usize {
                    // Use 2MB huge page (PMD leaf entry)
                    map_pmd_huge_page(virt, phys, linear_flags);
                } else {
                    // Use 4KB page
                    map_kernel_page(virt as u64, phys as u64, linear_flags);
                }

                phys += map_size;
                virt += map_size;
            }
        }

        // Final TLB flush after all linear mappings
        core::arch::asm!("sfence.vma zero, zero", options(nomem, nostack));
    }
}

/// Select best mapping size (Linux-style)
///
/// From Linux arch/riscv/mm/init.c: best_map_size()
/// For 64-bit systems, prefer PMD_SIZE (2MB) when aligned.
///
/// # Arguments
/// - `pa`: physical address
/// - `va`: virtual address
/// - `size`: remaining size to map
///
/// # Returns
/// - PMD_SIZE (2MB) if both addresses are 2MB-aligned and size >= 2MB
/// - PAGE_SIZE (4KB) otherwise
#[inline]
fn best_map_size(pa: usize, va: usize, size: usize) -> usize {
    const PMD_MASK: usize = (PMD_SIZE as usize) - 1;

    // For 64-bit: use PMD_SIZE (2MB) if aligned
    if (pa & PMD_MASK) == 0 && (va & PMD_MASK) == 0 && size >= PMD_SIZE as usize {
        PMD_SIZE as usize
    } else {
        PAGE_SIZE as usize
    }
}

/// Map a 2MB huge page using PMD leaf entry
///
/// This creates a PMD-level leaf entry that maps 2MB directly,
/// avoiding the need for 512 PTE entries.
///
/// # Safety
/// This function modifies the kernel page table directly.
unsafe fn map_pmd_huge_page(virt: usize, phys: usize, flags: u64) {
    let vpn2 = (virt >> 30) & 0x1FF;
    let vpn1 = (virt >> 21) & 0x1FF;

    // Get root page table (L2)
    let root = &mut ROOT_PAGE_TABLE;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_pmd_huge_page: failed to allocate L1 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        core::arch::asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Create PMD leaf entry (2MB huge page)
    // PPN for 2MB page: bits [55:21] of physical address
    // PTE format: [PPN[2] (26 bits)] [PPN[1] (9 bits)] [PPN[0] (9 bits)] [RSW] [DGBUWRXV]
    let ppn = (phys >> 12) as u64;
    let entry_bits = (ppn << 10) | flags;

    // Use appropriate mapping to access the page table
    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    (*table1).set(vpn1, PageTableEntry::from_bits(entry_bits));
}

pub fn enable() {
    unsafe {
        // Calculate root page table physical page number
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        let addr_space = MmStruct::new_kernel(root_ppn);
        addr_space.enable();
    }
}

pub fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let vpn2 = virt.vpn(2) as usize;
    let ppn = phys.ppn();

    unsafe {
        ROOT_PAGE_TABLE.set(vpn2, PageTableEntry::from_bits((ppn << 10) | flags));
    }
}

/// Map device memory page to user space
///
/// Used to map device memory like framebuffer to user process address space
///
/// # Arguments
/// - virt: virtual address (user space)
/// - phys: physical address (device memory)
/// - flags: page table entry flags (V, R, W, X, U, etc.)
///
/// # Note
/// This is a simplified implementation using 2MB huge page mapping
pub fn map_device_page(virt: usize, phys: usize, flags: u64) {
    // Use 2MB huge page mapping
    // For framebuffer, using 2MB pages is simpler
    let vpn2 = (virt >> 30) & 0x1FF;  // VPN[2] for L2 index

    // Calculate PPN (physical page number, for 2MB page is PPN[2:1])
    let ppn_2m = (phys >> 21) as u64;  // 2MB-aligned physical page number

    unsafe {
        // Create 1GB huge page entry (L2 leaf)
        // PPN[2:1] needs to be placed at correct position
        // PTE format: [PPN[2] (26 bits)] [PPN[1] (9 bits)] [PPN[0] (9 bits)] [RSW] [DGBUWRXV]
        let ppn = (phys >> 12) as u64;  // Complete physical page number
        let entry_bits = (ppn << 10) | flags;

        ROOT_PAGE_TABLE.set(vpn2 as usize, PageTableEntry::from_bits(entry_bits));
    }

    // Flush TLB
    unsafe {
        core::arch::asm!("sfence.vma", options(nomem, nostack));
    }
}

/// Map a kernel virtual page to a physical page
///
/// Used for vmemmap and other kernel mappings that need 4KB page granularity.
/// This function walks the 3-level page table and creates missing page tables as needed.
///
/// # Arguments
/// - `virt`: virtual address (must be page-aligned)
/// - `phys`: physical address (must be page-aligned)
/// - `flags`: page table entry flags (V, R, W, A, D, etc.)
///
/// # Safety
/// This function is unsafe because it modifies the kernel page table directly.
pub unsafe fn map_kernel_page(virt: u64, phys: u64, flags: u64) {
    // Extract virtual page numbers (VPN2, VPN1, VPN0)
    let vpn2 = ((virt >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt >> 12) & 0x1FF) as usize;

    // Get root page table (L2)
    let root = &mut ROOT_PAGE_TABLE;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_kernel_page: failed to allocate L1 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        // Flush after adding new page table entry
        core::arch::asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Level 1 -> Level 0
    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    let table1_ref = &mut *table1;
    let pte1 = table1_ref.get(vpn1);
    let ppn0 = if pte1.is_valid() {
        pte1.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_kernel_page: failed to allocate L0 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        table1_ref.set(vpn1, PageTableEntry::new_table(ppn));
        // Flush after adding new page table entry
        core::arch::asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Level 0 -> Physical page
    let table0_phys = ppn0 << PAGE_SHIFT;
    let table0 = get_page_table_virt(table0_phys);
    let table0_ref = &mut *table0;
    let ppn: u64 = phys >> PAGE_SHIFT;
    let pte_bits: u64 = (ppn << 10) | flags;

    table0_ref.set(vpn0, PageTableEntry::from_bits(pte_bits));

    // Flush TLB - use global flush for safety during early boot
    core::arch::asm!("sfence.vma zero, zero", options(nomem, nostack));
}

pub fn get_satp() -> Satp {
    unsafe {
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);
        Satp(satp)
    }
}

/// Convert physical address to kernel virtual address (Linux-style PAGE_OFFSET mapping)
///
/// Linux uses: virt = phys + va_pa_offset, where va_pa_offset = PAGE_OFFSET - phys_ram_base
/// This allows the kernel to access all physical memory via the PAGE_OFFSET region.
#[inline]
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    VirtAddr::new(phys.0 + VA_PA_OFFSET as u64)
}

/// Convert kernel virtual address to physical address (Linux-style PAGE_OFFSET mapping)
///
/// Linux uses: phys = virt - va_pa_offset, where va_pa_offset = PAGE_OFFSET - phys_ram_base
#[inline]
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    let addr = virt.0;

    // Check if this is a linear mapping address (PAGE_OFFSET region)
    if is_linear_mapping(addr as usize) {
        // Linux-style: phys = virt - va_pa_offset
        PhysAddr::new(addr - VA_PA_OFFSET as u64)
    } else if addr >= KERNEL_ENTRY && addr < 0x90000000 {
        // Legacy identity mapping (for transition period)
        // Physical memory region: 0x80000000 - 0x90000000
        PhysAddr::new(addr)
    } else {
        // User virtual address or other: return as-is (may need page table walk)
        PhysAddr::new(addr)
    }
}

// ==================== User Address Space Management ====================

struct PageTableWalker;

impl PageTableWalker {
    /// Walk page table to find physical page number for virtual address
    /// Return Some(ppn) if found, None if unmapped
    unsafe fn walk(user_root_ppn: u64, virt: u64) -> Option<u64> {
        let virt_addr = VirtAddr::new(virt);

        // Extract virtual page numbers
        let vpn2 = virt_addr.vpn(2) as usize;
        let vpn1 = virt_addr.vpn(1) as usize;
        let vpn0 = virt_addr.vpn(0) as usize;

        // Access page table using linear mapping
        let root_table = get_page_table_virt(user_root_ppn << PAGE_SHIFT);

        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() {
            return None;
        }

        let ppn1 = pte2.ppn();
        let table1 = get_page_table_virt(ppn1 << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() {
            return None;
        }

        let ppn0 = pte1.ppn();
        let table0 = get_page_table_virt(ppn0 << PAGE_SHIFT);
        let pte0 = (*table0).get(vpn0);
        if !pte0.is_valid() {
            return None;
        }

        Some(pte0.ppn())
    }
}

/// Allocate one page from the unified zone allocator
/// Returns physical address, or None if allocation fails
pub fn alloc_user_phys_page() -> Option<u64> {
    // Use the unified zone allocator with GFP_USER flags
    let phys = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys != 0 {
        Some(phys as u64)
    } else {
        None
    }
}

pub fn create_user_address_space() -> Option<u64> {
    // Allocate root page table from zone allocator
    let phys_addr = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys_addr == 0 {
        return None;
    }
    let root_page = phys_addr as u64;

    unsafe {
        // Initialize page table using linear mapping
        let root_table = get_page_table_virt(root_page);
        (*root_table).zero();

        // Copy kernel mappings to user page table
        // User page table needs to access kernel code (for system calls)
        let kernel_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        // Map kernel space to user page table
        // Simplified: directly map entire kernel region
        let root_ppn = root_page / PAGE_SIZE;
        copy_kernel_mappings(root_ppn, kernel_ppn);

        Some(root_ppn)
    }
}

unsafe fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    // Use linear mapping to access page tables
    let kernel_phys = kernel_root_ppn * PAGE_SIZE;
    let user_phys = user_root_ppn * PAGE_SIZE;

    let kernel_virt = get_page_table_virt(kernel_phys);
    let user_virt = get_page_table_virt(user_phys);

    let kernel_table = kernel_virt as *const PageTable;
    let user_table = user_virt as *mut PageTable;

    // Step 1: Copy all kernel mappings except VPN2[0]
    for i in 0..512 {
        let pte = (*kernel_table).get(i);
        if pte.is_valid() {
            // Skip VPN2[0] (user code and stack)
            if i == 0 {
                continue;
            }

            // Copy all other VPN2 entries, including VPN2[2] (kernel code)
            // This allows sret instruction to execute from user page table
            (*user_table).set(i, pte);
            // Memory barrier to ensure write is visible
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    // Note: .pagetables section mapping removed - page tables are now dynamically
    // allocated from kernel heap which is already covered by VPN2[2] mapping

    // Step 2: Linear mapping is already shared via kernel page table entries
    // The linear mapping (PAGE_OFFSET region) is set up in init_kernel_page_table
    // and is shared with user processes via the kernel page table entries copy above.
    // No need to map PHYS_MEM_REGION1/2 separately anymore.

    // Step 3: Map UART device
    // Use kernel-only permissions (U=0). User programs access UART via system calls,
    // not by direct memory access. This prevents unauthorized device access.
    // NOTE: UART_BASE (0x10000000) is in user space address range and conflicts with user heap.
    // We map it here with identity mapping because the console driver accesses UART directly
    // using physical address (0x10000000) as virtual address. When running with user page table,
    // this access needs a valid mapping.
    // TODO: In the future, use a proper kernel virtual address for UART (e.g., fixmap or kmap)
    // to avoid conflicting with user space addresses.
    let uart_flags = PageTableEntry::V | PageTableEntry::R |
                       PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, UART_BASE, 0x1000, uart_flags);
}

pub unsafe fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) {
    map_page(user_root_ppn, user_virt, phys, flags);
}

pub unsafe fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) {
    // Check overflow
    let virt_end_checked = virt_start.checked_add(size);
    if virt_end_checked.is_none() {
        panic!("map_user_region: virt_start + size overflow: virt_start={:#x}, size={:#x}",
               virt_start, size);
    }
    let virt_end_val = virt_end_checked.unwrap();

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);
    let virt_end = VirtAddr::new(virt_end_val);

    let mut virt = virt_start_addr.floor();
    let end = virt_end.ceil();

    while virt.bits() < end.bits() {
        let virt_bits = virt.bits();
        let virt_start_bits = virt_start_addr.bits();
        if virt_bits < virt_start_bits {
            panic!("map_user_region: virt ({:#x}) < virt_start ({:#x}), floor() failed?",
                   virt_bits, virt_start_bits);
        }
        let offset = virt_bits - virt_start_bits;
        let phys = PhysAddr::new(phys_start_addr.bits() + offset);
        map_page(user_root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
}

pub unsafe fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // Calculate required page count
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    // Allocate physical pages from zone allocator
    // For small allocations (<= 1 page), use order 0
    // For larger allocations, we'd need to handle multi-page allocations differently
    // For now, allocate one page at a time and concatenate
    let phys_addr = if page_count == 1 {
        alloc_pages(GfpFlags::GFP_USER, 0)
    } else {
        // For multi-page allocations, try to find a suitable order
        let order = (page_count.next_power_of_two().trailing_zeros() as usize).min(10);
        alloc_pages(GfpFlags::GFP_USER, order)
    };

    if phys_addr == 0 {
        return None;
    }

    // Map to user address space
    map_user_region(user_root_ppn, virt_addr, phys_addr as u64, size, flags);

    // Zero through linear mapping (MAP_ANONYMOUS requirement)
    let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
    core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr as u64)
}

pub fn get_kernel_page_table_ppn() -> u64 {
    unsafe {
        let root_addr = &raw mut ROOT_PAGE_TABLE as *mut PageTable as u64;
        root_addr / PAGE_SIZE
    }
}

pub unsafe fn alloc_and_map_to_kernel_table(
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // Calculate required page count
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    // Allocate physical pages from zone allocator
    let phys_addr = if page_count == 1 {
        alloc_pages(GfpFlags::GFP_USER, 0)
    } else {
        let order = (page_count.next_power_of_two().trailing_zeros() as usize).min(10);
        alloc_pages(GfpFlags::GFP_USER, order)
    };

    if phys_addr == 0 {
        return None;
    }

    // Get kernel page table PPN
    let kernel_ppn = get_kernel_page_table_ppn();

    // Add U-bit (user accessible)
    let user_flags = flags | PageTableEntry::U;

    // Map to kernel page table
    map_user_region(kernel_ppn, virt_addr, phys_addr as u64, size, user_flags);

    // Zero allocated memory through linear mapping (important: ensure BSS and uninitialized data are zero)
    let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
    core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr as u64)
}

/// Allocate physical pages and map to specified user page table
///
/// # Arguments
/// - user_ppn: User page table root PPN
/// - virt_addr: Virtual address start
/// - size: Mapping size
/// - flags: Page table entry flags
///
/// # Returns
/// - Some(phys_addr): Physical address start
/// - None: Allocation failed
pub unsafe fn alloc_and_map_to_user_table(
    user_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // Calculate required page count
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    // Allocate physical pages from zone allocator
    let phys_addr = if page_count == 1 {
        alloc_pages(GfpFlags::GFP_USER, 0)
    } else {
        let order = (page_count.next_power_of_two().trailing_zeros() as usize).min(10);
        alloc_pages(GfpFlags::GFP_USER, order)
    };

    if phys_addr == 0 {
        return None;
    }

    // Add U-bit (user accessible)
    let user_flags = flags | PageTableEntry::U;

    // Map to user page table
    map_user_region(user_ppn, virt_addr, phys_addr as u64, size, user_flags);

    // Zero allocated memory (use linear mapping to access physical memory)
    let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
    core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr as u64)
}

// ==================== Copy-on-Write (COW) Support ====================

/// Copy-on-Write flags
///
/// Used to mark pages for copy-on-write
/// We use PageTableEntry reserved bits to store COW flag
/// In RISC-V Sv39, bits [63:54] are reserved for software use
pub mod cow_flags {
    /// COW flag - page is marked as copy-on-write
    pub const COW: u64 = 1 << 8;  // Use bit 8 (after A and D)
}

/// Copy page table (for fork)
///
/// Create new page table, copy parent's page table entries, but mark writable pages as read-only + COW
///
/// # Arguments
/// - parent_root_ppn: Parent process root page table physical page number
///
/// # Returns
/// Returns child process root page table physical page number
///
/// # Safety
/// This function is unsafe because it directly manipulates raw pointers and page tables
pub unsafe fn copy_page_table_cow(parent_root_ppn: u64) -> Option<u64> {
    use crate::mm::page_desc::pfn_to_page_mut;

    // Check if parent_root_ppn is valid
    if parent_root_ppn == 0 {
        return None;
    }

    // Allocate new root page table (L2)
    let child_root_phys = alloc_page_table()?;
    let child_root_ppn = child_root_phys >> PAGE_SHIFT;

    // Copy L2 page table entries (512 entries)
    // Use get_page_table_virt() to access parent's page table via linear mapping
    let parent_root_phys = parent_root_ppn << PAGE_SHIFT;
    let parent_root = get_page_table_virt(parent_root_phys);
    let child_root = get_page_table_virt(child_root_phys);

    for vpn2 in 0..512 {
        let pte2 = (*parent_root).get(vpn2);

        if !pte2.is_valid() {
            continue;  // Skip invalid entries
        }

        let ppn1 = pte2.ppn();

        // Check if it's a leaf node (at least one of R/W/X is set)
        let is_leaf = pte2.is_readable() || pte2.is_writable() || pte2.is_executable();

        // Kernel region (VPN2 >= 2): directly share page table entry
        if vpn2 >= 2 {
            (*child_root).set(vpn2, pte2);
            continue;
        }

        // User space (VPN2 < 2): need to copy page table structure
        // For non-leaf nodes (pointing to next level page table), need recursive copy
        if is_leaf {
            // L2 leaf node (1GB huge page): temporarily share directly
            // TODO: Implement huge page COW
            (*child_root).set(vpn2, pte2);
            continue;
        }

        // Allocate new L1 page table
        let child_table1_phys = alloc_page_table()?;
        let child_ppn1 = child_table1_phys >> PAGE_SHIFT;
        (*child_root).set(vpn2, PageTableEntry::new_table(child_ppn1));

        // Use get_page_table_virt() to access parent's L1 page table via linear mapping
        let parent_table1 = get_page_table_virt(ppn1 << PAGE_SHIFT);
        let child_table1_ref = &mut *get_page_table_virt(child_table1_phys);

        // Copy L1 page table entries (512 entries)
        for vpn1 in 0..512 {
            let pte1 = (*parent_table1).get(vpn1);

            if !pte1.is_valid() {
                continue;  // Skip invalid entries
            }

            // Check if L1 is a leaf (2MB huge page)
            let is_l1_leaf = pte1.is_readable() || pte1.is_writable() || pte1.is_executable();
            if is_l1_leaf {
                // L1 leaf node (2MB huge page): share directly for now
                // TODO: Implement 2MB huge page COW
                (*child_table1_ref).set(vpn1, pte1);
                continue;
            }

            let ppn0 = pte1.ppn();

            // Allocate new L0 page table
            let child_table0_phys = alloc_page_table()?;
            let child_ppn0 = child_table0_phys >> PAGE_SHIFT;
            (*child_table1_ref).set(vpn1, PageTableEntry::new_table(child_ppn0));

            // Use get_page_table_virt() to access parent's L0 page table via linear mapping
            let parent_table0 = get_page_table_virt(ppn0 << PAGE_SHIFT);
            let child_table0_ref = &mut *get_page_table_virt(child_table0_phys);

            // Copy L0 page table entries (512 entries)
            for vpn0 in 0..512 {
                let pte0 = (*parent_table0).get(vpn0);

                if !pte0.is_valid() {
                    continue;  // Skip invalid entries
                }

                // Only apply COW marking to user writable pages
                // But exclude device memory (like UART), device memory has no page descriptor
                let is_user = pte0.bits() & PageTableEntry::U != 0;
                let is_writable = pte0.is_writable();

                let new_pte = if is_user && is_writable {
                    // Get physical page's page descriptor and increment reference count
                    // PPN in PTE is already physical page number, use directly
                    let phys_ppn = pte0.ppn() as usize;
                    let page = pfn_to_page_mut(phys_ppn);

                    // Only do COW when page descriptor exists
                    // Device memory (like UART) has no page descriptor, share directly
                    if !page.is_null() {
                        // Increment reference count:
                        // - First get_page(): increment for parent process's existing mapping
                        // - Second get_page(): increment for child process's new mapping
                        // Note: user page's initial refcount is 0, so need two increments
                        let old_ref = (*page).refcount();
                        if old_ref == 0 {
                            // Page hasn't been referenced yet, need to increment once for parent
                            (*page).get_page();
                        }
                        (*page).get_page();  // Increment for child process
                        (*page).set_flag(crate::mm::page_desc::PageFlag::Cow);

                        // Create read-only + COW PTE
                        let cow_pte_bits = pte0.bits() & !PageTableEntry::W | cow_flags::COW;

                        // Critical: Also modify parent's PTE to become read-only + COW
                        // This ensures both parent and child trigger page fault on write
                        let parent_table0_mut = parent_table0 as *mut PageTable;
                        (*parent_table0_mut).set(vpn0, PageTableEntry::from_bits(cow_pte_bits));

                        PageTableEntry::from_bits(cow_pte_bits)
                    } else {
                        // Device memory, copy PTE directly
                        pte0
                    }
                } else {
                    // Non-user page or read-only page, copy PTE directly
                    pte0
                };

                (*child_table0_ref).set(vpn0, new_pte);
            }
        }
    }

    // Critical: Flush TLB to ensure parent sees updated read-only PTE
    // If not flushed, TLB still caches old writable permission
    core::arch::asm!("sfence.vma", options(nostack, preserves_flags));

    Some(child_root_ppn)
}

/// Handle copy-on-write page fault
///
/// When process tries to write to COW page, copy that page and update page table
///
/// # Arguments
/// - root_ppn: Process root page table physical page number
/// - fault_addr: Virtual address that triggered fault
///
/// # Returns
/// Returns Some(()) on success, None on failure
///
/// # Safety
/// This function is unsafe because it directly manipulates raw pointers and page tables
pub unsafe fn handle_cow_fault(root_ppn: u64, fault_addr: VirtAddr) -> Option<()> {
    use crate::mm::page_desc::pfn_to_page_mut;

    let virt_addr = fault_addr.bits();

    // Extract virtual page numbers（VPN2, VPN1, VPN0）
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // Get root page table (L2) using linear mapping
    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);

    let pte2 = (*root_table).get(vpn2);
    if !pte2.is_valid() {
        return None;
    }

    let ppn1 = pte2.ppn();
    let table1 = get_page_table_virt(ppn1 << PAGE_SHIFT);

    let pte1 = (*table1).get(vpn1);
    if !pte1.is_valid() {
        return None;
    }

    let ppn0 = pte1.ppn();
    let table0 = get_page_table_virt(ppn0 << PAGE_SHIFT);

    let old_pte = (*table0).get(vpn0);
    if !old_pte.is_valid() {
        return None;
    }

    // Check if it's a COW page
    let old_bits = old_pte.bits();
    if old_bits & cow_flags::COW == 0 {
        return None;
    }

    let old_ppn = old_pte.ppn();

    // Check old page's reference count
    // PPN is already physical page number, use directly
    let old_page = pfn_to_page_mut(old_ppn as usize);

    let refcount = if !old_page.is_null() {
        (*old_page).refcount()
    } else {
        1  // If no page descriptor, assume only one reference
    };

    // If only one reference, directly restore write permission (no need to copy)
    if refcount <= 1 {
        // Update page table entry: remove COW flag, add W flag, keep original PPN
        let new_pte = PageTableEntry::from_bits(
            (old_bits & !cow_flags::COW) | PageTableEntry::W
        );

        // Update page table entry (before TLB flush)
        (*table0).set(vpn0, new_pte);

        // Flush TLB (after updating page table)
        asm!("sfence.vma zero, zero");

        return Some(());
    }

    // Multiple references, need to copy page

    // Decrement old page's reference count
    if !old_page.is_null() {
        (*old_page).put_page();
    }

    // Allocate new physical page - use User Physical Allocator
    let new_phys = alloc_user_phys_page()?;
    let new_ppn = new_phys >> PAGE_SHIFT;

    // Use linear mapping to access physical pages
    let new_virt = phys_to_virt(PhysAddr::new(new_phys));
    let old_virt = phys_to_virt(PhysAddr::new(old_ppn << PAGE_SHIFT));

    // Debug: check for 0x8080808080808080 pattern in the NEW page (uncomment if needed)
    // let new_words = new_virt.bits() as *const u64;
    // for i in 0..(PAGE_SIZE as usize / 8) {
    //     let word = unsafe { core::ptr::read_volatile(new_words.add(i)) };
    //     if word == 0x8080808080808080 {
    //         crate::println!("handle_cow_fault: WARNING - NEW page has 0x8080808080808080 at offset {:#x}", i * 8);
    //     }
    // }

    // Copy page content
    core::ptr::copy_nonoverlapping(
        old_virt.bits() as *const u8,
        new_virt.bits() as *mut u8,
        PAGE_SIZE as usize
    );

    // Create new page table entry: use new PPN, remove COW flag, add W flag
    // PTE format: PPN[53:10] | RSW[9:8] | D | A | G | U | X | W | R | V
    let flags = (old_bits & 0xFF) | PageTableEntry::W;  // Keep original flags, add W, remove COW
    let new_pte = PageTableEntry::from_bits((new_ppn << 10) | flags);

    // Update page table entry (before TLB flush)
    (*table0).set(vpn0, new_pte);

    // Flush TLB (after updating page table)
    asm!("sfence.vma zero, zero");

    Some(())
}

/// Check if page is a COW page
///
/// # Arguments
/// - root_ppn: Process root page table physical page number
/// - addr: Virtual address
///
/// # Returns
/// Returns true if it's a COW page, false otherwise
pub unsafe fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    let virt_addr = addr.bits();

    // Extract virtual page numbers
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // Walk page table using linear mapping
    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
    let pte2 = (*root_table).get(vpn2);

    if !pte2.is_valid() {
        return false;
    }

    let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return false;
    }

    let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
    let pte0 = (*table0).get(vpn0);

    if !pte0.is_valid() {
        return false;
    }

    // Check COW flag
    (pte0.bits() & cow_flags::COW) != 0
}

/// Check if page has required permissions
///
/// Returns (has_read, has_write, has_exec, is_user)
pub unsafe fn check_pte_permissions(root_ppn: u64, addr: VirtAddr) -> Option<(bool, bool, bool, bool)> {
    let virt_addr = addr.bits();

    // Extract virtual page numbers
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // Walk page table using linear mapping
    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
    let pte2 = (*root_table).get(vpn2);

    if !pte2.is_valid() {
        return None;
    }

    let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return None;
    }

    let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
    let pte0 = (*table0).get(vpn0);

    if !pte0.is_valid() {
        return None;
    }

    let bits = pte0.bits();
    let has_read = (bits & PageTableEntry::R) != 0;
    let has_write = (bits & PageTableEntry::W) != 0;
    let has_exec = (bits & PageTableEntry::X) != 0;
    let is_user = (bits & PageTableEntry::U) != 0;

    Some((has_read, has_write, has_exec, is_user))
}

/// Page fault type flags
///
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
    /// Already mapped (no handling needed)
    AlreadyMapped,
    /// COW pending (handled by handle_cow_fault)
    CowPending,
}

/// Handle page fault (demand paging)
///
/// Try to expand stack when page fault occurs below current stack bottom
///
/// This implements Linux-style on-demand stack expansion.
/// Called when address is not found in any VMA.
///
/// # Arguments
/// - `addr_space`: Address space
/// - `fault_addr`: Virtual address that triggered fault
/// - `flags`: Fault type flags (FaultFlags)
/// - `root_ppn`: Page table root PPN
///
/// # Returns
/// - `MmFaultResult::Handled`: Stack expanded and page mapped successfully
/// - `MmFaultResult::Segfault`: Address is not valid for stack expansion
/// - `MmFaultResult::OutOfMemory`: Memory allocation failed
fn try_expand_stack(
    addr_space: &AddressSpace,
    fault_addr: VirtAddr,
    flags: u32,
    root_ppn: u64,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;
    use crate::mm::page::PAGE_SIZE as MM_PAGE_SIZE;
    use crate::mm::vma::{VmaFlags, VmaType};

    // Convert to mm::page::VirtAddr (type used by VmaManager)
    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    // Check if this could be a stack expansion
    let stack_limit = addr_space.stack_limit();
    let fault_addr_val = fault_addr.as_usize();

    // Check if fault address is within stack expansion range
    // (above stack_limit but below current stack bottom)
    if fault_addr_val < stack_limit {
        // Below stack limit, cannot expand
        return MmFaultResult::Segfault;
    }

    // Try to find the stack VMA (with GROWSDOWN flag)
    let vma_mgr_read = addr_space.vma_read();
    let (vma_start, stack_vma) = match vma_mgr_read.find_stack_vma(page_virt_addr) {
        Some((start, vma)) => (start, vma),
        None => {
            // No stack VMA found
            return MmFaultResult::Segfault;
        }
    };

    // Calculate new start address (page-aligned, covering fault address)
    let new_start = PageVirtAddr::new(fault_addr_val & !(MM_PAGE_SIZE - 1));

    // New start must be below current VMA start
    if new_start.as_usize() >= vma_start.as_usize() {
        // Address is not below stack VMA, cannot expand
        return MmFaultResult::Segfault;
    }

    // Check if expansion would exceed stack limit
    if new_start.as_usize() < stack_limit {
        // Would exceed stack limit
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
        // Write lock dropped here
    }

    // Allocate new page (using user physical memory allocator)
    let phys_addr = match alloc_user_phys_page() {
        Some(addr) => PhysAddr::new(addr),
        None => return MmFaultResult::OutOfMemory,
    };

    // Convert physical address to virtual address for kernel access
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;

    // Initialize page content based on type
    unsafe {
        match vma_type {
            VmaType::Anonymous => {
                // Anonymous mapping: zero page
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::FileBacked | VmaType::SharedMemory => {
                // File/Shared mapping: zero for now
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::Device => {
                // Device mapping: don't zero, handled by driver
            }
        }
    }

    // Build page table entry flags
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U; // User page

    if vma_flags.is_readable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }

    // Map page
    unsafe {
        map_page(root_ppn, fault_addr, phys_addr, pte_flags);

        // Address-specific TLB flush (not global)
        let vaddr = fault_addr.bits();
        core::arch::asm!(
            "fence",
            "sfence.vma {0}, zero",
            "fence",
            in(reg) vaddr,
            options(nostack, preserves_flags)
        );
    }

    MmFaultResult::Handled
}

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
pub fn handle_mm_fault(
    addr_space: &AddressSpace,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    use crate::mm::page::VirtAddr as PageVirtAddr;
    use crate::mm::vma::VmaType;

    // Convert to mm::page::VirtAddr (type used by VmaManager)
    let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());

    // Check if page is already mapped
    let root_ppn = addr_space.root_ppn();
    let already_mapped = unsafe {
        PageTableWalker::walk(root_ppn, fault_addr.bits() as u64).is_some()
    };

    // Calculate access type flags
    let is_write = flags & FaultFlags::WRITE != 0;
    let is_read = flags & FaultFlags::READ != 0;
    let is_exec = flags & FaultFlags::EXEC != 0;
    let is_user = flags & FaultFlags::USER != 0;

    // If page is already mapped, first check if it's COW
    if already_mapped {
        // Check COW
        if is_write && unsafe { is_cow_page(root_ppn, fault_addr) } {
            return MmFaultResult::CowPending;
        }

        // Check if page permissions meet access requirements
        if let Some((has_read, has_write, has_exec, pte_is_user)) =
            unsafe { check_pte_permissions(root_ppn, fault_addr) } {
            // Verify permissions
            let perm_ok = (!is_write || has_write)
                && (!is_read || has_read)
                && (!is_exec || has_exec)
                && (!is_user || pte_is_user);

            if perm_ok {
                // Permissions correct, but TLB might be stale
                // Flush TLB (using address-specific flush)
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

        // Permissions incorrect
        return MmFaultResult::PermissionDenied;
    }

    // 1. Find VMA
    let vma_mgr = addr_space.vma_read();
    let vma = match vma_mgr.find(page_virt_addr) {
        Some(v) => v,
        None => {
            // Address not in any VMA - try stack expansion
            drop(vma_mgr);

            // Try to expand stack if possible
            return try_expand_stack(addr_space, fault_addr, flags, root_ppn);
        }
    };

    // Get VMA attributes
    let vma_flags = vma.flags();
    let vma_type = vma.vma_type();

    // 2. Verify permissions
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

    // Release read lock, subsequent operations may need write
    drop(vma_mgr);

    // 4. Allocate new page (using user physical memory allocator)
    let phys_addr = match alloc_user_phys_page() {
        Some(addr) => PhysAddr::new(addr),
        None => return MmFaultResult::OutOfMemory,
    };

    // Convert physical address to virtual address for kernel access
    let page_ptr = phys_to_virt(phys_addr).bits() as *mut u8;

    // 5. Initialize page content based on type
    unsafe {
        match vma_type {
            VmaType::Anonymous => {
                // Anonymous mapping: zero page
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::FileBacked => {
                // File mapping: TODO - read from file
                // Temporarily zero
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
            VmaType::Device => {
                // Device mapping: don't zero, handled by driver
            }
            VmaType::SharedMemory => {
                // Shared memory: zero
                core::ptr::write_bytes(page_ptr, 0, PAGE_SIZE_USIZE);
            }
        }
    }

    // 6. Build page table entry flags
    let mut pte_flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    pte_flags |= PageTableEntry::U; // User page

    if vma_flags.is_readable() {
        pte_flags |= PageTableEntry::R;
    }
    if vma_flags.is_writable() {
        pte_flags |= PageTableEntry::W;
    }
    if vma_flags.is_executable() {
        pte_flags |= PageTableEntry::X;
    }

    // 7. Map page
    unsafe {
        map_page(root_ppn, fault_addr, phys_addr, pte_flags);

        // Address-specific TLB flush (not global)
        let vaddr = fault_addr.bits();
        core::arch::asm!(
            "fence",
            "sfence.vma {0}, zero",
            "fence",
            in(reg) vaddr,
            options(nostack, preserves_flags)
        );
    }

    MmFaultResult::Handled
}

