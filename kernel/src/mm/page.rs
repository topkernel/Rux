//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Page Frame Management
//!
//! Basic types for physical and virtual page management.
//! Page allocation is handled by the zone allocator (page_alloc.rs).

pub const PAGE_SIZE: usize = 4096;

pub const PAGE_MASK: usize = PAGE_SIZE - 1;

pub type PhysFrameNr = usize;

pub type VirtPageNr = usize;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub usize);

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub usize);

impl PhysAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn frame_number(&self) -> PhysFrameNr {
        self.0 / PAGE_SIZE
    }

    /// Get physical page number (PPN)
    pub fn ppn(&self) -> usize {
        self.0 / PAGE_SIZE
    }
}

impl VirtAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn page_number(&self) -> VirtPageNr {
        self.0 / PAGE_SIZE
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PhysFrame {
    pub number: PhysFrameNr,
}

impl PhysFrame {
    pub const fn new(number: PhysFrameNr) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: PhysAddr) -> Self {
        Self::new(addr.frame_number())
    }

    pub fn start_address(&self) -> PhysAddr {
        PhysAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<PhysAddr> {
        let start = self.start_address();
        let end = PhysAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct VirtPage {
    pub number: VirtPageNr,
}

impl VirtPage {
    pub const fn new(number: VirtPageNr) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: VirtAddr) -> Self {
        Self::new(addr.page_number())
    }

    pub fn start_address(&self) -> VirtAddr {
        VirtAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<VirtAddr> {
        let start = self.start_address();
        let end = VirtAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

/// Initialize page descriptors
///
/// This must be called after memblock is initialized.
/// Page allocation uses the zone allocator (init_zone_system).
pub fn init_page_descriptors(start_frame: PhysFrameNr, nr_pages: usize) {
    // Initialize page descriptor array
    super::page_desc::init_mem_map(start_frame, nr_pages);
}

/// Get Page descriptor for a frame
pub fn frame_to_page(frame: PhysFrame) -> *const super::page_desc::Page {
    super::page_desc::frame_to_page(frame)
}

/// Get mutable Page descriptor for a frame
pub fn frame_to_page_mut(frame: PhysFrame) -> *mut super::page_desc::Page {
    super::page_desc::frame_to_page_mut(frame)
}
