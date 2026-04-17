//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Virtual Memory Area Management - Platform-Independent Part
//!
//!
//! VMA represents a contiguous virtual memory region in a process address space,
//! with the same access permissions and mapping attributes.
//!
//! This module only contains platform-independent data structures:
//! - VmaFlags: VMA flags
//! - Vma: VMA struct
//! - VmaManager: VMA manager
//! - AddressSpace: Platform-independent address space abstraction
//!
//! Architecture-specific implementations (such as page table management,
//! mmap/munmap/brk system calls) should be in arch/*/mm.rs
//!
//! # Safety Invariants — VMA Layout
//!
//! - **INV-VMA-1**: No two VMAs in the same address space overlap
//!   (their `[start, end)` intervals are pairwise disjoint).
//!
//! - **INV-VMA-2**: VMAs are sorted by start address, enforced by the BTreeMap key.
//!
//! - **INV-VMA-3**: `max_end` (if maintained) equals `max(end)` across all VMAs
//!   in the address space.
//!
//! - **INV-VMA-4**: Every VMA's `start` and `end` are `PAGE_SIZE`-aligned.

pub use crate::mm::page::{VirtAddr, PAGE_SIZE};
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaFlags(u32);

impl VmaFlags {
    /// Readable (VM_READ)
    pub const READ: u32 = 0x00000001;
    /// Writable (VM_WRITE)
    pub const WRITE: u32 = 0x00000002;
    /// Executable (VM_EXEC)
    pub const EXEC: u32 = 0x00000004;
    /// Shared mapping (VM_SHARED)
    pub const SHARED: u32 = 0x00000008;
    /// Private mapping (VM_PRIVATE)
    pub const PRIVATE: u32 = 0x00000010;
    /// May extend to heap (VM_GROWSDOWN)
    pub const GROWSDOWN: u32 = 0x00000100;
    /// May extend to stack (VM_GROWSUP)
    pub const GROWSUP: u32 = 0x00000200;
    /// Deny rmap (VM_DENYWRITE)
    pub const DENYWRITE: u32 = 0x00000800;
    /// Executable control/heap (VM_EXECUTABLE)
    pub const EXECUTABLE: u32 = 0x00001000;
    /// Locked memory (VM_LOCKED)
    pub const LOCKED: u32 = 0x00002000;
    /// I/O mapping (VM_IO)
    pub const IO: u32 = 0x00004000;

    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }

    #[inline]
    pub fn contains(&self, flags: u32) -> bool {
        self.0 & flags == flags
    }

    #[inline]
    pub fn insert(&mut self, flags: u32) {
        self.0 |= flags;
    }

    #[inline]
    pub fn remove(&mut self, flags: u32) {
        self.0 &= !flags;
    }

    /// Check if readable
    #[inline]
    pub fn is_readable(&self) -> bool {
        self.0 & Self::READ != 0
    }

    /// Check if writable
    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & Self::WRITE != 0
    }

    /// Check if executable
    #[inline]
    pub fn is_executable(&self) -> bool {
        self.0 & Self::EXEC != 0
    }

    /// Check if shared
    #[inline]
    pub fn is_shared(&self) -> bool {
        self.0 & Self::SHARED != 0
    }

    /// Check if private
    #[inline]
    pub fn is_private(&self) -> bool {
        self.0 & Self::PRIVATE != 0
    }

    /// Convert to page permissions (Perm)
    ///
    /// Infer page table permissions from VMA flags
    ///
    /// Mapping:
    /// - No READ/WRITE/EXEC -> Perm::None
    /// - READ only -> Perm::Read
    /// - READ + WRITE -> Perm::ReadWrite
    /// - READ + WRITE + EXEC -> Perm::ReadWriteExec
    /// - READ + EXEC -> Perm::Read (no ReadExec option, use Read)
    /// - WRITE + EXEC -> Perm::ReadWrite (no WriteExec option, use ReadWrite)
    ///
    pub fn to_page_perm(&self) -> crate::mm::pagemap::Perm {
        use crate::mm::pagemap::Perm;

        let readable = self.is_readable();
        let writable = self.is_writable();
        let executable = self.is_executable();

        match (readable, writable, executable) {
            (false, false, false) => Perm::None,
            (true, false, false) => Perm::Read,
            (true, true, false) => Perm::ReadWrite,
            (true, true, true) => Perm::ReadWriteExec,
            (true, false, true) => Perm::ReadExec,    // Read + Execute
            (false, true, false) => Perm::ReadWrite,   // Write-only (unusual)
            (false, true, true) => Perm::ReadWriteExec, // Write + Execute
            (false, false, true) => Perm::Exec,         // Execute-only (Sv39 supports X=1,R=0)
        }
    }
}

impl Default for VmaFlags {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub struct Vma {
    /// Start virtual address (inclusive)
    start: VirtAddr,

    /// End virtual address (exclusive)
    end: VirtAddr,

    /// Access permissions and attributes
    flags: VmaFlags,

    /// VMA offset (for file mapping)
    offset: usize,

    /// VMA type
    vma_type: VmaType,

    /// File descriptor for file-backed mappings (-1 for anonymous)
    file_fd: i32,

    /// File size for file-backed mappings (0 for anonymous)
    file_size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaType {
    /// Anonymous mapping (heap, stack, private data)
    Anonymous,
    /// File mapping
    FileBacked,
    /// Device mapping (MMIO)
    Device,
    /// Shared memory
    SharedMemory,
}

impl Vma {
    /// Create new VMA
    pub fn new(start: VirtAddr, end: VirtAddr, flags: VmaFlags) -> Self {
        assert!(start.as_usize() < end.as_usize(), "Invalid VMA range");
        assert!(start.as_usize() % PAGE_SIZE == 0, "VMA start not page aligned");
        assert!(end.as_usize() % PAGE_SIZE == 0, "VMA end not page aligned");

        Self {
            start,
            end,
            flags,
            offset: 0,
            vma_type: VmaType::Anonymous,
            file_fd: -1,
            file_size: 0,
        }
    }

    /// Get start address
    #[inline]
    pub fn start(&self) -> VirtAddr {
        self.start
    }

    /// Get end address
    #[inline]
    pub fn end(&self) -> VirtAddr {
        self.end
    }

    /// Get VMA size (bytes)
    #[inline]
    pub fn size(&self) -> usize {
        self.end.as_usize() - self.start.as_usize()
    }

    /// Get VMA size (page count)
    #[inline]
    pub fn page_count(&self) -> usize {
        self.size() / PAGE_SIZE
    }

    /// Get flags
    #[inline]
    pub fn flags(&self) -> VmaFlags {
        self.flags
    }

    /// Set flags
    pub fn set_flags(&mut self, flags: VmaFlags) {
        self.flags = flags;
    }

    /// Get type
    #[inline]
    pub fn vma_type(&self) -> VmaType {
        self.vma_type
    }

    /// Set type
    pub fn set_type(&mut self, vma_type: VmaType) {
        self.vma_type = vma_type;
    }

    /// Check if address is within VMA range
    #[inline]
    pub fn contains(&self, addr: VirtAddr) -> bool {
        addr.as_usize() >= self.start.as_usize() && addr.as_usize() < self.end.as_usize()
    }

    /// Check if two VMAs overlap
    pub fn overlaps(&self, other: &Vma) -> bool {
        self.start.as_usize() < other.end.as_usize()
            && other.start.as_usize() < self.end.as_usize()
    }

    /// Set file offset (for file mapping)
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
    }

    /// Get file offset
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Set file descriptor (for file-backed mapping)
    pub fn set_file_fd(&mut self, fd: i32) {
        self.file_fd = fd;
    }

    /// Get file descriptor
    #[inline]
    pub fn file_fd(&self) -> i32 {
        self.file_fd
    }

    /// Set file size (for file-backed mapping)
    pub fn set_file_size(&mut self, size: u64) {
        self.file_size = size;
    }

    /// Get file size
    #[inline]
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Split VMA at specified address
    ///
    /// Returns (first half, second half) or None if address not in range
    pub fn split(&self, addr: VirtAddr) -> Option<(Vma, Vma)> {
        if !self.contains(addr) {
            return None;
        }

        // Ensure split address is page aligned
        let aligned_addr = VirtAddr::new(addr.as_usize() & !(PAGE_SIZE - 1));
        if aligned_addr.as_usize() <= self.start.as_usize()
            || aligned_addr.as_usize() >= self.end.as_usize()
        {
            return None;
        }

        let first = Vma {
            start: self.start,
            end: aligned_addr,
            flags: self.flags,
            offset: self.offset,
            vma_type: self.vma_type,
            file_fd: self.file_fd,
            file_size: self.file_size,
        };

        let second = Vma {
            start: aligned_addr,
            end: self.end,
            flags: self.flags,
            offset: self.offset + (aligned_addr.as_usize() - self.start.as_usize()),
            vma_type: self.vma_type,
            file_fd: self.file_fd,
            file_size: self.file_size,
        };

        Some((first, second))
    }

    /// Can merge with another VMA?
    pub fn can_merge(&self, other: &Vma) -> bool {
        // Must be adjacent and have same attributes
        self.end.as_usize() == other.start.as_usize()
            && self.flags.bits() == other.flags.bits()
            && self.vma_type == other.vma_type
    }

    /// Merge with another VMA
    pub fn merge(&mut self, other: Vma) -> bool {
        if self.can_merge(&other) {
            self.end = other.end;
            true
        } else {
            false
        }
    }

    /// Extend end to cover an adjacent VMA (for forward merge in VmaManager::add)
    pub fn merge_at_end(&mut self, new_end: VirtAddr) {
        self.end = new_end;
    }
}

impl core::fmt::Debug for Vma {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Vma")
            .field("range", &format_args!("0x{:x}-0x{:x}", self.start.as_usize(), self.end.as_usize()))
            .field("size", &self.size())
            .field("flags", &self.flags)
            .field("type", &self.vma_type)
            .finish()
    }
}

/// VMA Manager
///
/// Uses BTreeMap to store VMAs, sorted by start address
/// - O(log n) lookup, insert, delete
/// - Dynamic expansion, no limit on count
pub struct VmaManager {
    /// VMA map (sorted by start address)
    vmas: BTreeMap<VirtAddr, Vma>,

    /// Cached maximum end address (for fast overlap detection)
    max_end: VirtAddr,

    /// VMA count (for compatibility)
    count: AtomicU32,
}

impl VmaManager {
    /// Create new VMA manager
    pub fn new() -> Self {
        Self {
            vmas: BTreeMap::new(),
            max_end: VirtAddr::new(0),
            count: AtomicU32::new(0),
        }
    }

    /// Add VMA
    ///
    /// # Parameters
    /// - `vma`: VMA to add
    ///
    /// # Returns
    /// - `Ok(())`: Added successfully
    /// - `Err(VmaError::Overlap)`: Overlaps with existing VMA
    ///
    /// # Performance
    /// O(log n) overlap check + O(log n) insert
    pub fn add(&mut self, vma: Vma) -> Result<(), VmaError> {
        let start = vma.start();
        let end = vma.end();

        // Optimization 1: Only check potentially overlapping VMAs
        // Since VMAs are sorted by start address, only need to check:
        // - Previous VMA (may extend into new VMA range)
        // - All VMAs with start address within new VMA range

        // Check if previous VMA overlaps
        if let Some((_, prev_vma)) = self.vmas.range(..start).next_back() {
            if prev_vma.end().as_usize() > start.as_usize() {
                return Err(VmaError::Overlap);
            }

            // Try to merge with previous VMA (same flags, adjacent)
            if prev_vma.can_merge(&vma) {
                if let Some(prev) = self.vmas.get_mut(&prev_vma.start()) {
                    if prev.merge(vma) {
                        // Merged — update max_end, no count change needed
                        if prev.end().as_usize() > self.max_end.as_usize() {
                            self.max_end = prev.end();
                        }
                        return Ok(());
                    }
                }
            }
        }

        // Check VMAs with start address in new VMA range
        if let Some((_, next_vma)) = self.vmas.range(start..=end).next() {
            if next_vma.start().as_usize() == end.as_usize() && vma.can_merge(next_vma) {
                // Merge with next VMA: remove next, extend new vma to cover it
                let next_end = next_vma.end();
                let next_start = next_vma.start();
                let mut merged_vma = vma;
                merged_vma.merge_at_end(next_end);
                self.vmas.remove(&next_start);
                self.vmas.insert(start, merged_vma);
                self.count.fetch_sub(1, Ordering::Release);
                if next_end.as_usize() > self.max_end.as_usize() {
                    self.max_end = next_end;
                }
                return Ok(());
            }
            // If VMA exists with start address in [start, end) range, then overlap
            if next_vma.start().as_usize() < end.as_usize() {
                return Err(VmaError::Overlap);
            }
        }

        // Update maximum end address
        if end.as_usize() > self.max_end.as_usize() {
            self.max_end = end;
        }

        // Insert into BTreeMap
        self.vmas.insert(start, vma);
        self.count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Find VMA containing specified address
    ///
    /// # Performance
    /// O(log n) using BTreeMap range lookup
    pub fn find(&self, addr: VirtAddr) -> Option<&Vma> {
        // Fast path: if address >= maximum end address, cannot find
        if addr.as_usize() >= self.max_end.as_usize() {
            return None;
        }

        // Use BTreeMap range lookup
        // Find largest VMA with start address <= addr
        // range(..=addr) returns all elements with key <= addr
        if let Some((_, vma)) = self.vmas.range(..=addr).next_back() {
            // Check if address is within this VMA range
            if vma.contains(addr) {
                return Some(vma);
            }
        }

        None
    }

    /// Find VMA containing specified address (mutable reference)
    pub fn find_mut(&mut self, addr: VirtAddr) -> Option<&mut Vma> {
        // Fast path
        if addr.as_usize() >= self.max_end.as_usize() {
            return None;
        }

        // Find VMA that may contain this address
        let start_addr = if let Some((&key, _)) = self.vmas.range(..=addr).next_back() {
            key
        } else {
            return None;
        };

        // Get mutable reference and check
        let vma = self.vmas.get_mut(&start_addr)?;
        if vma.contains(addr) {
            Some(vma)
        } else {
            None
        }
    }

    /// Remove VMA
    ///
    /// # Parameters
    /// - `start`: VMA start address
    pub fn remove(&mut self, start: VirtAddr) -> Result<(), VmaError> {
        if let Some(removed) = self.vmas.remove(&start) {
            // If removed VMA had maximum end address, need to recalculate
            if removed.end() == self.max_end {
                self.max_end = self.vmas.values()
                    .map(|v| v.end())
                    .max()
                    .unwrap_or(VirtAddr::new(0));
            }
            self.count.fetch_sub(1, Ordering::Release);
            Ok(())
        } else {
            Err(VmaError::NotFound)
        }
    }

    /// Get iterator over all VMAs
    pub fn iter(&self) -> impl Iterator<Item = &Vma> {
        self.vmas.values()
    }

    /// Get VMA count
    #[inline]
    pub fn count(&self) -> usize {
        self.vmas.len()
    }

    /// Find VMA at specified start address
    pub fn get(&self, start: VirtAddr) -> Option<&Vma> {
        self.vmas.get(&start)
    }

    /// Find VMA at specified start address (mutable reference)
    pub fn get_mut(&mut self, start: VirtAddr) -> Option<&mut Vma> {
        self.vmas.get_mut(&start)
    }

    /// Find first VMA
    pub fn first(&self) -> Option<&Vma> {
        self.vmas.values().next()
    }

    /// Find last VMA
    pub fn last(&self) -> Option<&Vma> {
        self.vmas.values().next_back()
    }

    /// Find first VMA with start address >= addr
    pub fn find_vma_after(&self, addr: VirtAddr) -> Option<&Vma> {
        self.vmas.range(addr..).next().map(|(_, vma)| vma)
    }

    /// Clear all VMAs
    pub fn clear(&mut self) {
        self.vmas.clear();
        self.max_end = VirtAddr::new(0);
        self.count.store(0, Ordering::Release);
    }

    /// Get maximum end address
    #[inline]
    pub fn max_end(&self) -> VirtAddr {
        self.max_end
    }

    /// Expand VMA downward (for stack expansion)
    ///
    /// This is used for stack growth when accessing an address below the current VMA start.
    /// Expand VMA downwards.
    ///
    /// # Arguments
    /// - `vma_start`: Start address of the VMA to expand
    /// - `new_start`: New start address (must be <= current start, page-aligned)
    ///
    /// # Returns
    /// - `Ok(())`: Expanded successfully
    /// - `Err(VmaError::NotFound)`: VMA not found at given start address
    /// - `Err(VmaError::Overlap)`: Would overlap with another VMA
    /// - `Err(VmaError::Invalid)`: Invalid new_start (not page aligned or above current start)
    pub fn expand_downwards(&mut self, vma_start: VirtAddr, new_start: VirtAddr) -> Result<(), VmaError> {
        // Validate new_start is page aligned
        if new_start.as_usize() % PAGE_SIZE != 0 {
            return Err(VmaError::Invalid);
        }

        // Get the VMA to expand
        let vma = self.vmas.get_mut(&vma_start).ok_or(VmaError::NotFound)?;

        // new_start must be below current start
        if new_start.as_usize() >= vma.start.as_usize() {
            return Err(VmaError::Invalid);
        }

        // Check for overlap with previous VMA
        // The new range is [new_start, vma.start)
        if let Some((_, prev_vma)) = self.vmas.range(..new_start).next_back() {
            if prev_vma.end().as_usize() > new_start.as_usize() {
                // Would overlap with previous VMA
                return Err(VmaError::Overlap);
            }
        }

        // Remove old VMA from BTreeMap (keyed by old start)
        let mut vma = self.vmas.remove(&vma_start).ok_or(VmaError::NotFound)?;

        // Expand the VMA
        vma.start = new_start;

        // Re-insert with new key (new start address)
        self.vmas.insert(new_start, vma);

        Ok(())
    }

    /// Find stack VMA (VMA with GROWSDOWN flag) containing or near the address
    ///
    /// # Arguments
    /// - `addr`: Address to check
    ///
    /// # Returns
    /// - `Some((start_addr, vma))`: Stack VMA found, returns its start address for expansion
    /// - `None`: No stack VMA found
    pub fn find_stack_vma(&self, addr: VirtAddr) -> Option<(VirtAddr, &Vma)> {
        // Look for a GROWSDOWN VMA where addr is just below it
        // Stack grows down, so we're looking for a VMA where:
        // addr < vma.start (or addr is in the VMA)

        for (start, vma) in self.vmas.iter() {
            if vma.flags().contains(VmaFlags::GROWSDOWN) {
                // Check if addr is within this VMA or just below it
                // For stack expansion, addr should be near (but below) the VMA start
                if vma.contains(addr) {
                    return Some((*start, vma));
                }
                // Check if addr is just below VMA start (within one page for expansion)
                // This handles the case where fault address is below current stack bottom
                let page_below_start = start.as_usize().saturating_sub(PAGE_SIZE);
                if addr.as_usize() >= page_below_start && addr.as_usize() < start.as_usize() {
                    return Some((*start, vma));
                }
            }
        }
        None
    }
}

impl Default for VmaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaError {
    /// VMA overlap
    Overlap,
    /// No space (reserved for compatibility)
    NoSpace,
    /// Not found
    NotFound,
    /// Invalid parameter
    Invalid,
}

unsafe impl Send for VmaManager {}
unsafe impl Sync for VmaManager {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vma_creation() {
        let start = VirtAddr::new(0x1000);
        let end = VirtAddr::new(0x2000);
        let flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE);

        let vma = Vma::new(start, end, flags);
        assert_eq!(vma.start(), start);
        assert_eq!(vma.end(), end);
        assert_eq!(vma.size(), 0x1000);
        assert_eq!(vma.page_count(), 1);
    }

    #[test]
    fn test_vma_contains() {
        let start = VirtAddr::new(0x1000);
        let end = VirtAddr::new(0x3000);
        let vma = Vma::new(start, end, VmaFlags::new());

        assert!(vma.contains(VirtAddr::new(0x1000)));
        assert!(vma.contains(VirtAddr::new(0x2000)));
        assert!(!vma.contains(VirtAddr::new(0x3000)));
        assert!(!vma.contains(VirtAddr::new(0xfff)));
    }
}

// ============================================================================
// Address Space Platform-Specific Parameter Interface
// ============================================================================

/// Address Space Platform-Specific Parameters
///
/// Different architectures need to provide their specific address space layout parameters
///
pub trait AddressSpaceLayout {
    /// User address space start address
    fn user_start() -> usize;

    /// User address space end address
    fn user_end() -> usize;

    /// Default stack size
    fn default_stack_size() -> usize;

    /// Default stack top (from user space top, going down)
    fn default_stack_top() -> usize;

    /// Heap start address
    fn heap_start() -> usize;

    /// Heap end address (maximum heap value)
    fn heap_end() -> usize;
}

// ============================================================================
// RISC-V Address Space Layout Implementation
// ============================================================================

/// RISC-V 64-bit Address Space Layout (Sv39 compatible)
///
/// Sv39 Address Space:
/// - User space: 0x0000000000000000 ~ 0x0000003FFFFFFFFF (256GB = TASK_SIZE)
/// - Kernel space: 0xFFFFFFD600000000 ~ 0xFFFFFFFFFFFFFFFF (high canonical)
///
/// User space layout:
/// - 0x0 ~ 0x1000: Null page (unmapped, null pointer guard)
/// - 0x1000+: ELF code/data segments
/// - brk area: follows ELF segments, grows up to TASK_SIZE/3
/// - mmap area: TASK_SIZE/3 ~ TASK_SIZE (top-down allocation)
/// - Stack: TASK_SIZE - stack_size, grows down
#[cfg(target_arch = "riscv64")]
pub struct RiscVAddressSpaceLayout;

#[cfg(target_arch = "riscv64")]
impl AddressSpaceLayout for RiscVAddressSpaceLayout {
    /// User space start address (0, but null page protected)
    #[inline]
    fn user_start() -> usize {
        crate::arch::riscv64::mm::user_addr::USER_START
    }

    /// User space end address = TASK_SIZE = 256GB for Sv39
    #[inline]
    fn user_end() -> usize {
        crate::arch::riscv64::mm::user_addr::TASK_SIZE
    }

    /// Default stack size (8MB)
    #[inline]
    fn default_stack_size() -> usize {
        crate::arch::riscv64::mm::user_addr::STACK_MAX_SIZE
    }

    /// Default stack top (TASK_SIZE, stack grows down)
    #[inline]
    fn default_stack_top() -> usize {
        crate::arch::riscv64::mm::user_addr::STACK_TOP
    }

    /// Heap start address (brk default)
    #[inline]
    fn heap_start() -> usize {
        crate::arch::riscv64::mm::user_addr::BRK_DEFAULT
    }

    /// Heap end address (maximum brk can grow to)
    #[inline]
    fn heap_end() -> usize {
        crate::arch::riscv64::mm::user_addr::BRK_MAX
    }
}
