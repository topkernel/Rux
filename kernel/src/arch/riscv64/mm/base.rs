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

use crate::println;
use crate::config::MAX_PAGE_TABLES;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use spin::RwLock;

// ==================== Constant definitions ====================

pub const PAGE_SIZE: u64 = 4096;

pub const PAGE_SHIFT: u64 = 12;

pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;

pub const VA_BITS: u64 = 39;

pub const VA_MASK: u64 = (1 << VA_BITS) - 1;

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

/// User space address range
pub mod user_addr {
    /// User space start address
    pub const USER_START: usize = 0x0001_0000;
    /// User space end address (below stack)
    pub const USER_END: usize = 0x7fff_f000;
    /// brk default start address
    /// Note: musl libc's mallocng uses the brk return address as mmap(MAP_FIXED) argument
    /// We need to place brk at an address musl won't use for mmap
    /// musl typically performs mmap at lower addresses, so brk is placed at a higher address
    /// But actually musl uses brk(0) return value as mmap hint, requiring special handling
    /// Here we use 0x30000000 (768MB), between program area and musl mmap area
    pub const BRK_DEFAULT: usize = 0x3000_0000;  // 768MB
    /// mmap area start address
    pub const MMAP_START: usize = 0x5000_0000;  // 1.25 GB
    /// mmap area end address
    pub const MMAP_END: usize = 0x6000_0000;
    /// Stack base (grows down)
    pub const STACK_TOP: usize = 0x7fff_f000;
    /// Stack maximum size
    pub const STACK_MAX_SIZE: usize = 8 * 1024 * 1024;  // 8MB
    /// Heap start address
    pub const HEAP_START: usize = 0x0100_0000;
    /// Heap maximum size
    pub const HEAP_MAX_SIZE: usize = 128 * 1024 * 1024;  // 128MB
}

// ==================== Address types ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    /// Create virtual address
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr & VA_MASK)
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

#[repr(C)]
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

        // Zero physical page
        unsafe {
            let ptr = phys_addr as *mut u8;
            core::ptr::write_bytes(ptr, 0, PAGE_SIZE_USIZE);
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
                    let frame = mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
                    let flags = perm_to_flags(Perm::ReadWrite, self.space_type());
                    unsafe {
                        map_page(
                            self.pgd,
                            VirtAddr::new(addr as u64),
                            PhysAddr::new(frame.start_address().as_usize() as u64),
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
                    // Found large enough gap
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

        let root_table = (self.pgd << PAGE_SHIFT) as *mut PageTable;

        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() {
            return;
        }

        let table1 = (pte2.ppn() << PAGE_SHIFT) as *mut PageTable;
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() {
            return;
        }

        let table0 = (pte1.ppn() << PAGE_SHIFT) as *mut PageTable;

        // Clear page table entry
        (*table0).set(vpn0, PageTableEntry::from_bits(0));
    }

    /// brk system call implementation (legacy interface)
    pub fn do_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        self.set_brk(new_brk)
    }

    /// Allocate stack space
    pub fn allocate_stack(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let stack_size = if size == 0 { 8 * 1024 * 1024 } else { size };
        let aligned_size = (stack_size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        let stack_top = PageVirtAddr::new(0x7fff_f000 & !(PAGE_SIZE_USIZE - 1));
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

#[link_section = ".pagetables"]
static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

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

unsafe fn alloc_page_table() -> &'static mut PageTable {
    // Use statically allocated page table (simplified implementation)
    // Each page table occupies one 4KB page
    // Place in .pagetables section to avoid position changes due to code growth
    #[link_section = ".pagetables"]
    static mut PAGE_TABLES: [PageTable; MAX_PAGE_TABLES] = [PageTable::new(); MAX_PAGE_TABLES];
    static NEXT_INDEX: AtomicUsize = AtomicUsize::new(0);

    let idx = NEXT_INDEX.fetch_add(1, Ordering::AcqRel);
    if idx >= PAGE_TABLES.len() {
        panic!("mm: Out of page table pages (allocated {})", idx);
    }

    // println!("mm: alloc_page_table: allocated index {}", idx);
    &mut PAGE_TABLES[idx]
}

unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    // Extract virtual page numbers（VPN2, VPN1, VPN0）
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // Get root page table (L2)
    let root_table_addr = root_ppn << PAGE_SHIFT;
    let root_table = root_table_addr as *mut PageTable;
    let root = &mut *root_table;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table = alloc_page_table();
        let ppn = (table as *const PageTable as u64) >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 1 -> Level 0
    let table1_addr = ppn1 << PAGE_SHIFT;
    let table1 = table1_addr as *mut PageTable;
    let table1_ref = &mut *table1;
    let pte1 = table1_ref.get(vpn1);
    let ppn0 = if pte1.is_valid() {
        pte1.ppn()
    } else {
        let table = alloc_page_table();
        let ppn = (table as *const PageTable as u64) >> PAGE_SHIFT;
        table1_ref.set(vpn1, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 0 -> Physical page
    let table0_addr = ppn0 << PAGE_SHIFT;
    let table0 = table0_addr as *mut PageTable;
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

        // Map kernel space (0x80200000 - 0x80A00000, 8MB)
        // QEMU virt: kernel starts at 0x80200000
        // Increase mapping size to avoid memory layout changes due to code growth
        let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x80200000, 0x800000, kernel_flags);

        // Map heap space (starts at 0x80A00000, size determined by config)
        // For dynamic memory allocation (Buddy System)
        // Use identity mapping: virtual 0x80A00000 -> physical 0x80A00000
        // Note: This ensures virt_to_phys() correctly converts VirtQueue DMA addresses
        let heap_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        let heap_virt_start = 0x80A00000u64;
        let heap_phys_start = 0x80A00000u64;  // identity mapping
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
        let slab_virt_start = 0x80A00000u64 + crate::config::KERNEL_HEAP_SIZE as u64;
        let slab_size = 4 * 1024 * 1024u64; // 4MB
        map_region(root_ppn, slab_virt_start, slab_size, heap_flags);

        // Map user physical memory area (0x84000000 - 0x88000000, 64MB)
        // For accessing user page tables and user program memory
        // Use kernel permissions (not user), as this is kernel access
        let user_phys_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x84000000, 0x4000000, user_phys_flags);

        // Map UART device (0x10000000)
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x10000000, 0x1000, device_flags);

        // Map VirtIO device MMIO area (possible locations)
        // QEMU virt may place VirtIO devices at the following locations:
        // 1. 0x10001000-0x10009000 (legacy MMIO)
        // Map VirtIO MMIO area
        map_region(root_ppn, 0x10001000, 0x100000, device_flags);

        // Map PLIC (Platform-Level Interrupt Controller, 0x0c000000)
        // PLIC layout:
        // - 0x0c000000-0x0c00ffff: PRIORITY, PENDING
        // - 0x0c010000-0x0c01ffff: reserved
        // - 0x0c020000-0x0c03ffff: Hart 0 context (ENABLE, THRESHOLD, CLAIM/COMPLETE)
        // - 0x0c030000-0x0c03ffff: Hart 1 context
        // - 0x0c040000-0x0c04ffff: Hart 2 context
        // - 0x0c050000-0x0c05ffff: Hart 3 context
        // Need full mapping of 0x200000 (CONTEXT_SIZE * 4 = 0x1000 * 4 = 0x400000)
        map_region(root_ppn, 0x0c000000, 0x200000, device_flags);

        // Map CLINT (Core Local Interruptor, 0x02000000)
        map_region(root_ppn, 0x02000000, 0x10000, device_flags);

        // Map DTB area (0xbfe00000, OpenSBI usually places DTB here)
        // Map 1MB is enough for DTB
        map_region(root_ppn, 0xbfe00000, 0x100000, device_flags);

        // Map PCIe ECAM space (0x30000000-0x31ffffff, for PCI config space access)
        // RISC-V virt platform: PCIe ECAM starts at 0x30000000
        // Each device 4KB, max 256 devices, total 1MB
        map_region(root_ppn, 0x30000000, 0x100000, device_flags);

        // Map PCI MMIO space (0x40000000-0x50000000, for PCI device BAR access)
        // RISC-V virt platform: PCI device MMIO BAR address range
        // BAR addresses allocated for PCI devices are mapped to this area
        map_region(root_ppn, 0x40000000, 0x10000000, device_flags);

        // Enable MMU
        let addr_space = MmStruct::new_kernel(root_ppn);
        addr_space.enable();
    }
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

pub fn get_satp() -> Satp {
    unsafe {
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);
        Satp(satp)
    }
}

pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    // RISC-V Sv39 address translation
    // QEMU virt platform: kernel loaded at 0x80200000, uses identity mapping (virtual address = physical address)

    const KERNEL_VIRT_BASE: u64 = 0x80200000;
    const KERNEL_VIRT_END: u64 = 0x82000000;  // Kernel space end (heap + reserved space)

    // Heap space constants (using identity mapping)
    const HEAP_VIRT_BASE: u64 = 0x80A00000;

    let addr = virt.0;

    // Kernel space (including code, data and heap) all use **identity mapping**
    // virtual address = physical address
    if addr >= KERNEL_VIRT_BASE && addr < KERNEL_VIRT_END {
        // Kernel code/data/heap space: use identity mapping
        // 0x80200000 -> 0x80200000 (code)
        // 0x80A00000 -> 0x80A00000 (heap)
        PhysAddr::new(addr)
    } else if addr >= KERNEL_VIRT_BASE {
        // Kernel space but not in above range (should not happen)
        PhysAddr::new(addr)
    } else {
        // User virtual address: need to look up page table for translation
        PhysAddr::new(addr)
    }
}

// ==================== User Address Space Management ====================

/// User Physical Allocator
/// Place in .data section to avoid being zeroed by BSS
#[link_section = ".data"]
static mut USER_PHYS_ALLOCATOR: PhysAllocator = PhysAllocator::new();

/// User Physical Allocator initialization flag
fn user_phys_allocator_is_initialized() -> bool {
    static INIT_FLAG: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    INIT_FLAG.swap(true, core::sync::atomic::Ordering::AcqRel)
}

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

        // Access page table using physical address (identity mapping)
        let root_table_addr = user_root_ppn << PAGE_SHIFT;
        let root_table = root_table_addr as *const PageTable;

        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() {
            return None;
        }

        let ppn1 = pte2.ppn();
        let table1 = (ppn1 << PAGE_SHIFT) as *const PageTable;
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() {
            return None;
        }

        let ppn0 = pte1.ppn();
        let table0 = (ppn0 << PAGE_SHIFT) as *const PageTable;
        let pte0 = (*table0).get(vpn0);
        if !pte0.is_valid() {
            return None;
        }

        Some(pte0.ppn())
    }
}

struct PhysAllocator {
    /// Current allocation position (physical address)
    current: u64,
    /// Allocation limit (lowest address)
    limit: u64,
}

impl PhysAllocator {
    const fn new() -> Self {
        Self {
            current: 0,
            limit: 0,
        }
    }

    /// Initialize allocator
    ///
    /// # Arguments
    /// - `start`: Start physical address (allocate from high to low)
    /// - `limit`: Lowest allocatable address
    unsafe fn init(&mut self, start: u64, limit: u64) {
        self.current = start;
        self.limit = limit;
    }

    /// Allocate one physical page
    ///
    /// Return physical address of page, or None if allocation fails
    unsafe fn alloc_page(&mut self) -> Option<u64> {
        if self.current < self.limit + PAGE_SIZE {
            return None;
        }

        self.current -= PAGE_SIZE;
        Some(self.current)
    }

    /// Allocate multiple physical pages
    unsafe fn alloc_pages(&mut self, count: usize) -> Option<u64> {
        let total_size = count as u64 * PAGE_SIZE;

        if self.current < self.limit + total_size {
            return None;
        }

        self.current -= total_size;
        Some(self.current)
    }
}

pub fn init_user_phys_allocator(start: u64, size: u64) {
    // Prevent multi-core duplicate initialization
    if user_phys_allocator_is_initialized() {
        return;
    }

    unsafe {
        // Allocate from memory top down, preserve bottom for kernel
        // QEMU virt: usually has 128MB memory (0x80000000 + 128MB)
        let alloc_start = start + size;
        let alloc_limit = start + 0x4000000; // Reserve 64MB for kernel

        USER_PHYS_ALLOCATOR.init(alloc_start, alloc_limit);

        // Memory barrier: ensure writes visible to all CPUs
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    }
}

/// Allocate one page from user physical memory allocator
/// Returns physical address, or None if allocation fails
pub fn alloc_user_phys_page() -> Option<u64> {
    unsafe { USER_PHYS_ALLOCATOR.alloc_page() }
}

pub fn create_user_address_space() -> Option<u64> {
    unsafe {
        // Allocate root page table (one page)
        let root_page = USER_PHYS_ALLOCATOR.alloc_page()?;

        // Initialize page table
        let root_table = root_page as *mut PageTable;
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
    // Use physical address as virtual address (QEMU virt identity mapping)
    // Note: This relies on QEMU virt platform physical address layout
    let kernel_virt = kernel_root_ppn * PAGE_SIZE;
    let user_virt = user_root_ppn * PAGE_SIZE;

    let kernel_table = kernel_virt as *const PageTable;
    let user_table = user_virt as *mut PageTable;

    // Step 1: Copy all kernel mappings except VPN2[0]
    let mut copied = 0;
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
            copied += 1;
        }
    }

    // Step 2: Map .pagetables section (for page table allocator)
    // This ensures map_page can access page tables allocated by alloc_page_table()
    // .pagetables section address: 0x803f6000 - 0x807f7000 (about 4MB)
    extern "C" {
        static __pagetables_start: u8;
        static __pagetables_end: u8;
    }
    let pagetables_start = &__pagetables_start as *const u8 as u64;
    let pagetables_end = &__pagetables_end as *const u8 as u64;
    let pagetables_size = pagetables_end - pagetables_start;

    // Map .pagetables section with kernel permissions (not user permissions)
    // Because this is accessed by kernel when operating page tables
    let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                       PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, pagetables_start, pagetables_size, kernel_flags);

    // Step 3: Map user physical memory region (0x84000000 - 0x88000000)
    // This region contains memory managed by user physical page allocator
    // Use kernel-only permissions (U=0) to prevent user processes from accessing
    // other processes' physical memory. Kernel can still access via these mappings.
    let user_phys_flags = PageTableEntry::V | PageTableEntry::R |
                          PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x84000000, 0x4000000, user_phys_flags);

    // Step 4: Map UART device (0x10000000)
    // Use kernel-only permissions (U=0). User programs access UART via system calls,
    // not by direct memory access. This prevents unauthorized device access.
    let uart_flags = PageTableEntry::V | PageTableEntry::R |
                       PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x10000000, 0x1000, uart_flags);
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

    let mut iteration = 0;
    while virt.bits() < end.bits() {
        // offset = current virtual address - start virtual address
        // virt >= virt_start_addr should always hold since virt = floor(virt_start)
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
        iteration += 1;
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

    // Allocate physical pages
    let phys_addr = USER_PHYS_ALLOCATOR.alloc_pages(page_count)?;

    // Map to user address space
    map_user_region(user_root_ppn, virt_addr, phys_addr, size, flags);

    // Zero through physical address (MAP_ANONYMOUS requirement)
    // Kernel uses identity mapping, physical address can be accessed directly
    core::ptr::write_bytes(phys_addr as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr)
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

    // Allocate physical pages
    let phys_addr = USER_PHYS_ALLOCATOR.alloc_pages(page_count)?;

    // Get kernel page table PPN
    let kernel_ppn = get_kernel_page_table_ppn();

    // Add U-bit (user accessible)
    let user_flags = flags | PageTableEntry::U;

    // Map to kernel page table
    map_user_region(kernel_ppn, virt_addr, phys_addr, size, user_flags);

    // Zero allocated memory (important: ensure BSS and uninitialized data are zero)
    // Kernel uses identity mapping, physical address can be accessed directly
    core::ptr::write_bytes(phys_addr as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr)
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

    // Allocate physical pages
    let phys_addr = USER_PHYS_ALLOCATOR.alloc_pages(page_count)?;

    // Add U-bit (user accessible)
    let user_flags = flags | PageTableEntry::U;

    // Map to user page table
    map_user_region(user_ppn, virt_addr, phys_addr, size, user_flags);

    // Zero allocated memory
    core::ptr::write_bytes(phys_addr as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr)
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
    use crate::mm::page_desc::{pfn_to_page_mut, PHYS_MEMORY_BASE};

    // Check if parent_root_ppn is valid
    if parent_root_ppn == 0 {
        return None;
    }

    // Allocate new root page table (L2)
    let child_root_table = alloc_page_table();
    let child_root_ppn = (child_root_table as *const PageTable as u64) >> PAGE_SHIFT;

    // Copy L2 page table entries (512 entries)
    let parent_root = (parent_root_ppn << PAGE_SHIFT) as *const PageTable;
    let child_root = child_root_table as *mut PageTable;

    let mut kernel_entries = 0;
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
            kernel_entries += 1;
            continue;
        }

        // User space (VPN2 < 2): need to copy page table structure
        // For non-leaf nodes (pointing to next level page table), need recursive copy
        if is_leaf {
            // L2 leaf node (2MB huge page): temporarily share directly
            // TODO: Implement huge page COW
            (*child_root).set(vpn2, pte2);
            continue;
        }

        // Allocate new L1 page table
        let child_table1 = alloc_page_table();
        let child_ppn1 = (child_table1 as *const PageTable as u64) >> PAGE_SHIFT;
        (*child_root).set(vpn2, PageTableEntry::new_table(child_ppn1));

        let parent_table1 = (ppn1 << PAGE_SHIFT) as *const PageTable;
        let child_table1_ref = &mut *child_table1;

        // Copy L1 page table entries (512 entries)
        for vpn1 in 0..512 {
            let pte1 = (*parent_table1).get(vpn1);

            if !pte1.is_valid() {
                continue;  // Skip invalid entries
            }

            let ppn0 = pte1.ppn();

            // Allocate new L0 page table
            let child_table0 = alloc_page_table();
            let child_ppn0 = (child_table0 as *const PageTable as u64) >> PAGE_SHIFT;
            (*child_table1_ref).set(vpn1, PageTableEntry::new_table(child_ppn0));

            let parent_table0 = (ppn0 << PAGE_SHIFT) as *const PageTable;
            let child_table0_ref = &mut *child_table0;

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

    // Get root page table (L2)
    let root_table_addr = root_ppn << PAGE_SHIFT;
    let root_table = root_table_addr as *mut PageTable;

    let pte2 = (*root_table).get(vpn2);
    if !pte2.is_valid() {
        return None;
    }

    let ppn1 = pte2.ppn();
    let table1 = (ppn1 << PAGE_SHIFT) as *mut PageTable;

    let pte1 = (*table1).get(vpn1);
    if !pte1.is_valid() {
        return None;
    }

    let ppn0 = pte1.ppn();
    let table0 = (ppn0 << PAGE_SHIFT) as *mut PageTable;

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

    let new_virt = new_phys as *mut u8;
    let old_virt = (old_ppn << PAGE_SHIFT) as *const u8;

    // Copy page content
    core::ptr::copy_nonoverlapping(old_virt, new_virt, PAGE_SIZE as usize);

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

    // Walk page table
    let root_table = (root_ppn << PAGE_SHIFT) as *const PageTable;
    let pte2 = (*root_table).get(vpn2);

    if !pte2.is_valid() {
        return false;
    }

    let table1 = (pte2.ppn() << PAGE_SHIFT) as *const PageTable;
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return false;
    }

    let table0 = (pte1.ppn() << PAGE_SHIFT) as *const PageTable;
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

    // Walk page table
    let root_table = (root_ppn << PAGE_SHIFT) as *const PageTable;
    let pte2 = (*root_table).get(vpn2);

    if !pte2.is_valid() {
        return None;
    }

    let table1 = (pte2.ppn() << PAGE_SHIFT) as *const PageTable;
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return None;
    }

    let table0 = (pte1.ppn() << PAGE_SHIFT) as *const PageTable;
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

    // If page is already mapped, first check if it's COW
    if already_mapped {
        let is_write = flags & FaultFlags::WRITE != 0;
        let is_read = flags & FaultFlags::READ != 0;
        let is_exec = flags & FaultFlags::EXEC != 0;
        let is_user = flags & FaultFlags::USER != 0;

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
                    // Extra global flush
                    core::arch::asm!("sfence.vma", options(nostack, preserves_flags));
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
            // Address not in any VMA, and page unmapped
            return MmFaultResult::Segfault;
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

    let page_ptr = phys_addr.bits() as *mut u8;

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

        // Force memory barrier and TLB flush
        // Use address-specific sfence.vma to flush single page's TLB entry
        let vaddr = fault_addr.bits();
        core::arch::asm!(
            "fence",                          // Memory barrier
            "sfence.vma {0}, zero",           // Flush TLB entry for specified virtual address
            "fence",                          // Memory barrier again
            in(reg) vaddr,
            options(nostack, preserves_flags)
        );

        // Extra global TLB flush (QEMU TCG might need it)
        core::arch::asm!("sfence.vma", options(nostack, preserves_flags));
    }

    MmFaultResult::Handled
}

