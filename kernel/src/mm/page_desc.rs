//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Page Descriptor
//!
//! Maintains metadata for each physical page frame, including:
//! - Reference count (_refcount)
//! - Page flags (flags)
//! - Map count (_mapcount)
//! - Other metadata
//!
//! # Safety Invariants
//!
//! The following invariants must hold at all times for every `Page` descriptor:
//!
//! - **INV-REF-1**: `_refcount` must never be negative.
//!   `put_page()` restores the old value on underflow and warns.
//!
//! - **INV-REF-2**: `_refcount == 0` ⟺ page is on a buddy free list (or otherwise free).
//!
//! - **INV-REF-3**: `_refcount > 0` ⟺ page is in use by at least one owner.
//!
//! - **INV-REF-4**: `_mapcount == -1` (PAGE_MAPCOUNT_BIAS) ⟺ page is not mapped
//!   by any page table entry.
//!
//! - **INV-REF-5**: `_mapcount > -1` ⟺ page is mapped in `(_mapcount + 1)` page tables.
//!
//! - **INV-REF-6**: If `Cow` flag is set ⟹ `_refcount >= 2` AND the PTE `W` bit is clear.
//!   This is the COW pre-condition: the page is shared read-only among multiple mappings.
//!

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

use super::page::{PhysAddr, PhysFrame, PhysFrameNr, VirtAddr, PAGE_SIZE};

/// Page flags
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageFlag {
    /// Page is locked, not accessible
    Locked = 1 << 0,
    /// Page is being written back
    Writeback = 1 << 1,
    /// Page has been accessed (for LRU)
    Referenced = 1 << 2,
    /// Page data is valid (read from disk)
    UpToDate = 1 << 3,
    /// Page has been modified (needs writeback)
    Dirty = 1 << 4,
    /// Page is in LRU list
    Lru = 1 << 5,
    /// Head page of compound page
    Head = 1 << 6,
    /// Page has waiters
    Waiters = 1 << 7,
    /// Page is in active LRU list
    Active = 1 << 8,
    /// Reserved page (kernel use, not swappable)
    Reserved = 1 << 9,
    /// Page has private data (stored in private field)
    Private = 1 << 10,
    /// Page will be reclaimed
    Reclaim = 1 << 11,
    /// Page is backed by swap space
    SwapBacked = 1 << 12,
    /// Page is unevictable
    Unevictable = 1 << 13,
    /// Copy-on-write page (Rux extension)
    Cow = 1 << 14,
    /// Anonymous page (Rux extension)
    Anonymous = 1 << 15,
}

/// Page flags collection
#[derive(Debug, Default)]
pub struct PageFlags(AtomicU32);

impl PageFlags {
    /// Create empty flags collection
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Create from raw value
    pub const fn from_raw(flags: u32) -> Self {
        Self(AtomicU32::new(flags))
    }

    /// Get raw value
    pub fn raw(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Test if flag is set
    pub fn test(&self, flag: PageFlag) -> bool {
        self.0.load(Ordering::Relaxed) & (flag as u32) != 0
    }

    /// Set flag
    pub fn set(&self, flag: PageFlag) {
        self.0.fetch_or(flag as u32, Ordering::Release);
    }

    /// Clear flag
    pub fn clear(&self, flag: PageFlag) {
        self.0.fetch_and(!(flag as u32), Ordering::Release);
    }

    /// Test and set flag (returns old value)
    pub fn test_and_set(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_or(bit, Ordering::AcqRel) & bit) != 0
    }

    /// Test and clear flag (returns old value)
    pub fn test_and_clear(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_and(!bit, Ordering::AcqRel) & bit) != 0
    }

    /// Clear all flags
    pub fn clear_all(&self) {
        self.0.store(0, Ordering::Release);
    }
}

/// Page type
///
/// Used for special page identification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageType {
    /// Normal page
    Normal = 0,
    /// Buddy system free page
    Buddy = 1,
    /// Slab allocator page
    Slab = 2,
    /// Page cache page
    PageCache = 3,
    /// Anonymous page
    Anonymous = 4,
}

/// Page descriptor
///
/// Each physical page frame corresponds to a Page structure, used to track page usage.
///
/// Memory layout (64 bytes, aligned to cache line):
/// - flags: 4 bytes (atomic flags)
/// - _mapcount: 4 bytes (map count, -1 means unmapped)
/// - _refcount: 4 bytes (reference count)
/// - private: 8 bytes (private data)
/// - mapping: 8 bytes (associated address_space, for rmap)
/// - index: 8 bytes (offset in mapping, for rmap)
/// - _type: 4 bytes (page type)
/// - next_free: 8 bytes (free list pointer, for allocator)
/// - lru_next: 8 bytes (LRU next PFN, for singly-linked LRU list)
///
#[repr(C, align(64))]
pub struct Page {
    /// Atomic flags
    flags: PageFlags,

    /// Map count: how many page table entries directly reference this page
    /// -1 means unmapped, 0 means mapped once, etc.
    /// Note: initial value is -1 (PAGE_MAPCOUNT_BIAS)
    _mapcount: AtomicI32,

    /// Reference count: number of references to this page
    /// 0 means free, > 0 means in use
    _refcount: AtomicI32,

    /// Private data
    /// - Buddy system: stores order
    /// - Slab: stores slab management structure
    /// - File system: stores buffer_head
    private: AtomicUsize,

    /// Associated address space (for rmap)
    /// Points to struct address_space or stores VPN for anon pages
    /// This field is rmap-only; LRU uses the dedicated lru_next field.
    mapping: AtomicUsize,

    /// Offset in mapping (in page units, for rmap)
    /// This field is rmap-only; LRU uses the dedicated lru_next field.
    index: AtomicUsize,

    /// Page type (for special pages)
    _type: AtomicU32,

    /// Free list pointer (for allocator internal use)
    next_free: AtomicUsize,

    /// LRU next pointer (PFN of next page in LRU list, 0 = end of list)
    /// Used for singly-linked LRU lists; separate from mapping/index (rmap).
    lru_next: AtomicUsize,
}

/// Map count initial offset value (-1 means unmapped)
const PAGE_MAPCOUNT_BIAS: i32 = -1;

impl Page {
    /// Create new page descriptor (initialized to free state)
    pub const fn new() -> Self {
        Self {
            flags: PageFlags::new(),
            _mapcount: AtomicI32::new(PAGE_MAPCOUNT_BIAS),
            _refcount: AtomicI32::new(0),
            private: AtomicUsize::new(0),
            mapping: AtomicUsize::new(0),
            index: AtomicUsize::new(0),
            _type: AtomicU32::new(PageType::Normal as u32),
            next_free: AtomicUsize::new(usize::MAX),  // FREE_LIST_NULL
            lru_next: AtomicUsize::new(0),
        }
    }

    /// Initialize as reserved page (kernel code, device memory, etc.)
    pub fn init_reserved(&self) {
        self.flags.set(PageFlag::Reserved);
        self._refcount.store(1, Ordering::Release);
    }

    /// Initialize as normal available page
    pub fn init_free(&self) {
        self.flags.clear_all();
        self._mapcount.store(PAGE_MAPCOUNT_BIAS, Ordering::Release);
        self._refcount.store(0, Ordering::Release);
        self.private.store(0, Ordering::Release);
        self.mapping.store(0, Ordering::Release);
        self.index.store(0, Ordering::Release);
        self.lru_next.store(0, Ordering::Release);
    }

    // ========== Flag operations ==========

    /// Test flag
    #[inline]
    pub fn test_flag(&self, flag: PageFlag) -> bool {
        self.flags.test(flag)
    }

    /// Set flag
    #[inline]
    pub fn set_flag(&self, flag: PageFlag) {
        self.flags.set(flag);
    }

    /// Clear flag
    #[inline]
    pub fn clear_flag(&self, flag: PageFlag) {
        self.flags.clear(flag);
    }

    /// Test and set flag
    #[inline]
    pub fn test_and_set_flag(&self, flag: PageFlag) -> bool {
        self.flags.test_and_set(flag)
    }

    /// Test and clear flag
    #[inline]
    pub fn test_and_clear_flag(&self, flag: PageFlag) -> bool {
        self.flags.test_and_clear(flag)
    }

    /// Check if page is locked
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.test_flag(PageFlag::Locked)
    }

    /// Check if page is reserved
    #[inline]
    pub fn is_reserved(&self) -> bool {
        self.test_flag(PageFlag::Reserved)
    }

    /// Check if page is dirty
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.test_flag(PageFlag::Dirty)
    }

    /// Check if page is copy-on-write
    #[inline]
    pub fn is_cow(&self) -> bool {
        self.test_flag(PageFlag::Cow)
    }

    /// Check if page is anonymous
    #[inline]
    pub fn is_anonymous(&self) -> bool {
        self.test_flag(PageFlag::Anonymous)
    }

    /// Check if page data is valid
    #[inline]
    pub fn is_uptodate(&self) -> bool {
        self.test_flag(PageFlag::UpToDate)
    }

    // ========== Reference count operations ==========

    /// Get reference count
    #[inline]
    pub fn refcount(&self) -> i32 {
        self._refcount.load(Ordering::Acquire)
    }

    /// Increment reference count
    /// Returns the value after increment
    #[inline]
    pub fn get_page(&self) -> i32 {
        self._refcount.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement reference count
    /// Returns the value after decrement; if it becomes 0, caller should free the page
    /// On underflow (refcount was already 0), restores the value and warns
    #[inline]
    pub fn put_page(&self) -> i32 {
        let prev = self._refcount.fetch_sub(1, Ordering::AcqRel);
        let result = prev - 1;
        if result < 0 {
            // Underflow: restore refcount, return negative to prevent caller from freeing
            self._refcount.fetch_add(1, Ordering::AcqRel);
            crate::pr_warn!("put_page: refcount underflow (prev={})", prev);
        }
        result
    }

    /// Try to increment reference count (only if refcount > 0)
    /// Returns true on success
    #[inline]
    pub fn try_get_page(&self) -> bool {
        loop {
            let old = self._refcount.load(Ordering::Acquire);
            if old <= 0 {
                return false;
            }
            match self._refcount.compare_exchange_weak(
                old,
                old + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// Set reference count (only for initialization)
    #[inline]
    pub fn set_refcount(&self, count: i32) {
        self._refcount.store(count, Ordering::Release);
    }

    // ========== Map count operations ==========

    /// Get map count (-1 means unmapped)
    #[inline]
    pub fn mapcount(&self) -> i32 {
        self._mapcount.load(Ordering::Acquire)
    }

    /// Increment map count
    /// Returns the value after increment
    #[inline]
    pub fn add_mapcount(&self) -> i32 {
        self._mapcount.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement map count
    /// Returns the value after decrement
    #[inline]
    pub fn sub_mapcount(&self) -> i32 {
        self._mapcount.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// Check if page is mapped
    #[inline]
    pub fn is_mapped(&self) -> bool {
        self._mapcount.load(Ordering::Acquire) > PAGE_MAPCOUNT_BIAS
    }

    /// Increment map count (alias for add_mapcount)
    #[inline]
    pub fn inc_mapcount(&self) -> i32 {
        self.add_mapcount()
    }

    /// Decrement map count (alias for sub_mapcount)
    #[inline]
    pub fn dec_mapcount(&self) -> i32 {
        self.sub_mapcount()
    }

    /// Reset map count
    #[inline]
    pub fn reset_mapcount(&self) {
        self._mapcount.store(PAGE_MAPCOUNT_BIAS, Ordering::Release);
    }

    // ========== Private data operations ==========

    /// Get private data
    #[inline]
    pub fn private(&self) -> usize {
        self.private.load(Ordering::Acquire)
    }

    /// Set private data
    #[inline]
    pub fn set_private(&self, value: usize) {
        self.private.store(value, Ordering::Release);
    }

    // ========== Mapping info operations ==========

    /// Get associated address_space
    #[inline]
    pub fn mapping(&self) -> *mut core::ffi::c_void {
        self.mapping.load(Ordering::Acquire) as *mut core::ffi::c_void
    }

    /// Set associated address_space
    #[inline]
    pub fn set_mapping(&self, mapping: *mut core::ffi::c_void) {
        self.mapping.store(mapping as usize, Ordering::Release);
    }

    /// Get page index
    #[inline]
    pub fn index(&self) -> usize {
        self.index.load(Ordering::Acquire)
    }

    /// Set page index
    #[inline]
    pub fn set_index(&self, index: usize) {
        self.index.store(index, Ordering::Release);
    }

    // ========== Page type operations ==========

    /// Get page type
    #[inline]
    pub fn page_type(&self) -> PageType {
        match self._type.load(Ordering::Acquire) {
            0 => PageType::Normal,
            1 => PageType::Buddy,
            2 => PageType::Slab,
            3 => PageType::PageCache,
            4 => PageType::Anonymous,
            _ => PageType::Normal,
        }
    }

    /// Set page type
    #[inline]
    pub fn set_page_type(&self, page_type: PageType) {
        self._type.store(page_type as u32, Ordering::Release);
    }

    // ========== LRU operations ==========

    /// Get LRU next PFN (0 = end of list)
    #[inline]
    pub fn lru_next(&self) -> usize {
        self.lru_next.load(Ordering::Acquire)
    }

    /// Set LRU next PFN
    #[inline]
    pub fn set_lru_next(&self, pfn: usize) {
        self.lru_next.store(pfn, Ordering::Release);
    }

    // ========== Free list operations (allocator internal use) ==========

    /// Get next free page's PFN
    #[inline]
    pub(crate) fn next_free(&self) -> usize {
        self.next_free.load(Ordering::Acquire)
    }

    /// Set next free page's PFN
    #[inline]
    pub(crate) fn set_next_free(&self, pfn: usize) {
        self.next_free.store(pfn, Ordering::Release);
    }

    // ========== Buddy allocator operations ==========

    /// Get order (stored in private field for buddy pages)
    #[inline]
    pub fn order(&self) -> u8 {
        (self.private.load(Ordering::Acquire) & 0xFF) as u8
    }

    /// Set order (stored in private field for buddy pages)
    #[inline]
    pub fn set_order(&self, order: u8) {
        self.private.store(order as usize, Ordering::Release);
    }

    /// Check if page is free (refcount == 0)
    #[inline]
    pub fn is_free(&self) -> bool {
        self._refcount.load(Ordering::Acquire) == 0
    }
}

// ========== Page content operations ==========

/// Copy contents of one physical page to another.
///
/// Both `src_pfn` and `dst_pfn` must be valid page frame numbers.
/// The pages must not overlap.
///
/// # Safety
/// - src_pfn and dst_pfn must be valid
/// - The source page must contain valid data
/// - The caller must ensure no concurrent writes to either page
pub unsafe fn copy_page_contents(src_pfn: usize, dst_pfn: usize) {
    let src = super::zone::pfn_to_phys(src_pfn) as *const u8;
    let dst = super::zone::pfn_to_phys(dst_pfn) as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
}

// ========== Global page array (mem_map) ==========

/// Physical memory constants
pub const PHYS_MEMORY_BASE: usize = 0x8000_0000; // QEMU virt: physical memory start address

// Use physical memory size from config (Kernel.toml: memory.physical_memory)
pub const PHYS_MEMORY_SIZE: usize = crate::config::PHYS_MEMORY_SIZE;

/// Maximum Page Frame Number
pub const MAX_PFN: usize = (PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE) / PAGE_SIZE;

/// Minimum Page Frame Number (physical memory starts here)
pub const MIN_PFN: usize = PHYS_MEMORY_BASE / PAGE_SIZE;

/// Check if a Page Frame Number is valid (within physical memory range)
/// Check if PFN is valid
#[inline]
pub const fn pfn_valid(pfn: usize) -> bool {
    pfn >= MIN_PFN && pfn < MAX_PFN
}

/// Check if a physical address is valid (within physical memory range)
#[inline]
pub const fn phys_valid(phys: usize) -> bool {
    phys >= PHYS_MEMORY_BASE && phys < PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE
}

/// Page array size
///
/// MAX_PAGES must match the actual physical memory size configured.
/// This is used for vmemmap bounds checking in pfn_to_page().
///
/// Note: MEM_MAP array size = MAX_PAGES * sizeof(Page) = MAX_PAGES * 64 bytes
/// - 16384 pages = 64MB physical memory = 1MB descriptors
/// - 32768 pages = 128MB physical memory = 2MB descriptors
/// - 65536 pages = 256MB physical memory = 4MB descriptors
/// - 262144 pages = 1GB physical memory = 16MB descriptors
/// - 524288 pages = 2GB physical memory = 32MB descriptors
///
/// CRITICAL: This must match PHYS_MEMORY_SIZE for vmemmap bounds checking to work correctly.
/// If they don't match, pfn_to_page() may return invalid pointers for PFNs beyond actual memory.
pub const MAX_PAGES: usize = PHYS_MEMORY_SIZE / PAGE_SIZE;

/// Global page array (legacy - actual page access via vmemmap)
///
/// This is a minimal placeholder array. Actual page descriptor access
/// is done through vmemmap virtual addresses (see pfn_to_page).
/// We keep a small array for legacy API compatibility.
/// DO NOT use this for actual page descriptor storage - use vmemmap instead.
#[link_section = ".bss"]
static mut MEM_MAP: [u8; 4096] = [0u8; 4096]; // Just 4KB placeholder

/// Whether page array is initialized
static MEM_MAP_INIT: AtomicUsize = AtomicUsize::new(0);

/// Get page array start address
/// Note: Returns pointer to BSS array, memory is zero-initialized
#[inline]
pub fn mem_map() -> *const Page {
    unsafe { MEM_MAP.as_ptr() as *const Page }
}

/// Get mutable page array start address
/// Note: Returns pointer to BSS array, memory is zero-initialized
#[inline]
pub fn mem_map_mut() -> *mut Page {
    unsafe { MEM_MAP.as_mut_ptr() as *mut Page }
}

/// Initialize page array
///
/// Initialize page descriptors within specified range.
/// Pages outside the range are marked as reserved.
///
/// # Arguments
/// - `start_pfn`: Available memory start PFN
/// - `nr_pages`: Number of available pages
pub fn init_mem_map(start_pfn: PhysFrameNr, nr_pages: usize) {
    // Prevent duplicate initialization
    if MEM_MAP_INIT.swap(1, Ordering::AcqRel) != 0 {
        return;
    }

    // Use vmemmap to access page descriptors
    // Only iterate over the pages that are actually mapped in vmemmap
    let init_count = if nr_pages > MAX_PAGES { MAX_PAGES } else { nr_pages };

    // Mark all pages as reserved first, then init available pages
    for i in 0..init_count {
        let pfn = start_pfn + i;
        let page = pfn_to_page(pfn);
        if !page.is_null() {
            unsafe {
                (*page).init_reserved();
            }
        }
    }

    // Initialize available pages as free
    for i in 0..init_count {
        let pfn = start_pfn + i;
        let page = pfn_to_page(pfn);
        if !page.is_null() {
            unsafe {
                (*page).init_free();
            }
        }
    }
}

// ========== PFN <-> Page conversion (vmemmap-style) ==========

/// vmemmap base address for page descriptors
/// This is defined in arch/riscv64/mm/base.rs
pub const VMEMMAP_START: usize = crate::arch::riscv64::mm::VMEMMAP_START;

/// PFN (Page Frame Number) to Page pointer
///
/// Uses vmemmap addressing:
///   page_addr = VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)
///
/// This is O(1).
///
/// # Safety
/// Caller must ensure pfn is in valid range
#[inline]
pub fn pfn_to_page(pfn: PhysFrameNr) -> *const Page {
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;

    // Check if pfn is in valid range
    if pfn < base_pfn {
        return core::ptr::null();
    }

    let idx = pfn - base_pfn;
    if idx >= MAX_PAGES {
        return core::ptr::null();
    }

    // vmemmap: VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)
    let vaddr = VMEMMAP_START + idx * core::mem::size_of::<Page>();
    vaddr as *const Page
}

/// PFN to mutable Page pointer
#[inline]
pub fn pfn_to_page_mut(pfn: PhysFrameNr) -> *mut Page {
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;

    // Check if pfn is in valid range
    if pfn < base_pfn {
        return core::ptr::null_mut();
    }

    let idx = pfn - base_pfn;
    if idx >= MAX_PAGES {
        return core::ptr::null_mut();
    }

    // vmemmap: VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)
    let vaddr = VMEMMAP_START + idx * core::mem::size_of::<Page>();
    vaddr as *mut Page
}

/// Page pointer to PFN
///
/// Uses vmemmap-style addressing:
///   idx = (page_addr - VMEMMAP_START) / sizeof(Page)
///   pfn = idx + base_pfn
///
/// # Safety
/// Caller must ensure page pointer is valid
#[inline]
pub fn page_to_pfn(page: *const Page) -> PhysFrameNr {
    let page_addr = page as usize;
    let idx = (page_addr - VMEMMAP_START) / core::mem::size_of::<Page>();
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;
    idx + base_pfn
}

/// Physical address to Page pointer
#[inline]
pub fn phys_to_page(phys: PhysAddr) -> *const Page {
    pfn_to_page(phys.frame_number())
}

/// Physical address to mutable Page pointer
#[inline]
pub fn phys_to_page_mut(phys: PhysAddr) -> *mut Page {
    pfn_to_page_mut(phys.frame_number())
}

/// Physical frame to Page pointer
#[inline]
pub fn frame_to_page(frame: PhysFrame) -> *const Page {
    pfn_to_page(frame.number)
}

/// Physical frame to mutable Page pointer
#[inline]
pub fn frame_to_page_mut(frame: PhysFrame) -> *mut Page {
    pfn_to_page_mut(frame.number)
}

// ========== Helper functions ==========

/// Get total number of pages
#[inline]
pub fn total_pages() -> usize {
    MAX_PAGES
}

/// Get Page structure size (bytes)
#[inline]
pub fn page_size() -> usize {
    core::mem::size_of::<Page>()
}

/// Page descriptor statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct PageDescStats {
    /// Total page count
    pub total_pages: usize,
    /// Free page count (refcount == 0)
    pub free_pages: usize,
    /// In-use page count (refcount > 0)
    pub used_pages: usize,
    /// Reserved page count (Reserved flag)
    pub reserved_pages: usize,
    /// Mapped page count (mapcount > PAGE_MAPCOUNT_BIAS)
    pub mapped_pages: usize,
    /// Dirty page count (Dirty flag)
    pub dirty_pages: usize,
    /// COW page count (Cow flag)
    pub cow_pages: usize,
    /// Anonymous page count (Anonymous flag)
    pub anonymous_pages: usize,
}

/// Get page descriptor statistics
pub fn page_desc_stats() -> PageDescStats {
    let mut stats = PageDescStats {
        total_pages: MAX_PAGES,
        ..Default::default()
    };

    let base_pfn = MIN_PFN;
    for i in 0..MAX_PAGES {
        let page = pfn_to_page(base_pfn + i);
        if page.is_null() {
            continue;
        }
        unsafe {
            if (*page).refcount() == 0 {
                stats.free_pages += 1;
            } else {
                stats.used_pages += 1;
            }

            if (*page).is_reserved() {
                stats.reserved_pages += 1;
            }

            if (*page).is_mapped() {
                stats.mapped_pages += 1;
            }

            if (*page).is_dirty() {
                stats.dirty_pages += 1;
            }

            if (*page).is_cow() {
                stats.cow_pages += 1;
            }

            if (*page).is_anonymous() {
                stats.anonymous_pages += 1;
            }
        }
    }

    stats
}

/// Get Page reference for a physical frame
///
/// # Safety
/// Caller must ensure pfn is in valid range
#[inline]
pub unsafe fn get_page(pfn: PhysFrameNr) -> &'static Page {
    &*pfn_to_page(pfn)
}

/// Get mutable Page reference for a physical frame
///
/// # Safety
/// Caller must ensure pfn is in valid range and there are no other references
#[inline]
pub unsafe fn get_page_mut(pfn: PhysFrameNr) -> &'static mut Page {
    &mut *pfn_to_page_mut(pfn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_page_flags() {
        let flags = PageFlags::new();

        assert!(!flags.test(PageFlag::Locked));
        assert!(!flags.test(PageFlag::Dirty));

        flags.set(PageFlag::Locked);
        assert!(flags.test(PageFlag::Locked));

        flags.set(PageFlag::Dirty);
        assert!(flags.test(PageFlag::Dirty));

        flags.clear(PageFlag::Locked);
        assert!(!flags.test(PageFlag::Locked));
        assert!(flags.test(PageFlag::Dirty));
    }

    #[test_case]
    fn test_page_refcount() {
        let page = Page::new();

        assert_eq!(page.refcount(), 0);

        page.get_page();
        assert_eq!(page.refcount(), 1);

        page.get_page();
        assert_eq!(page.refcount(), 2);

        page.put_page();
        assert_eq!(page.refcount(), 1);

        page.put_page();
        assert_eq!(page.refcount(), 0);
    }

    #[test_case]
    fn test_page_mapcount() {
        let page = Page::new();

        // Initial map count is -1 (unmapped)
        assert_eq!(page.mapcount(), -1);
        assert!(!page.is_mapped());

        page.add_mapcount();
        assert_eq!(page.mapcount(), 0);
        assert!(page.is_mapped());

        page.add_mapcount();
        assert_eq!(page.mapcount(), 1);

        page.sub_mapcount();
        assert_eq!(page.mapcount(), 0);
    }
}
