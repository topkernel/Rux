//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Sv39 Page Table Structures
//!
//! This module contains:
//! - PageTableEntry structure and operations
//! - PageTable structure
//! - Satp CSR handling for MMU control

use core::arch::asm;

use super::memory_layout::{PAGE_SIZE, PAGE_SHIFT, PAGE_OFFSET_MASK};

// ==================== Page Table Entry ====================

/// RISC-V Sv39 Page Table Entry
///
/// PTE format (64 bits):
/// - [9:8]   RSW    - Reserved for software use
/// - [7]     D      - Dirty bit
/// - [6]     A      - Accessed bit
/// - [5]     G      - Global mapping
/// - [4]     U      - User accessible
/// - [3]     X      - Executable
/// - [2]     W      - Writable
/// - [1]     R      - Readable
/// - [0]     V      - Valid
/// - [53:10] PPN    - Physical Page Number
/// - [63:54] Reserved
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

    /// SVPBMT Memory Type bits (bits 62:61)
    /// 00 - PMA: Normal Cacheable
    /// 01 - NC:  Non-cacheable, idempotent, weakly-ordered
    /// 10 - IO:  Non-cacheable, non-idempotent, strongly-ordered I/O memory
    /// 11 - Reserved
    ///
    /// IO memory type for device MMIO registers:
    /// - Non-cacheable: no speculative reads/writes
    /// - Non-idempotent: reads/writes have side effects
    /// - Strongly-ordered: writes complete in program order
    pub const IO: u64 = 1 << 62;  // SVPBMT IO memory type

    /// Create empty page table entry
    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    /// Create from raw bits
    #[inline]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Get raw bits value
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

    /// Get physical page number (PPN, bits [53:10])
    /// For Sv39: PPN[2] = bits [53:28], PPN[1] = bits [27:19], PPN[0] = bits [18:10]
    #[inline]
    pub fn ppn(&self) -> u64 {
        (self.0 >> 10) & 0x00FFFFFFFFFFFFFF
    }

    /// Get physical page number for huge page at PMD level (2MB page)
    /// For Sv39 PMD leaf: PPN[2] = bits [53:28], PPN[1] = bits [27:19]
    /// The physical address is {PPN[2], PPN[1], page_offset[20:0]}
    /// PPN[0] in the PTE is reserved/WI for 2MB pages
    #[inline]
    pub fn ppn_for_2mb_page(&self) -> u64 {
        // Extract PPN[2] (bits 53:28) - 26 bits
        let ppn2 = (self.0 >> 28) & 0x3FFFFFF;
        // Extract PPN[1] (bits 27:19) - 9 bits
        let ppn1 = (self.0 >> 19) & 0x1FF;
        // For 2MB pages: physical page number = {PPN[2], PPN[1]}
        // This is a 35-bit value: bits 34:9 = PPN[2], bits 8:0 = PPN[1]
        (ppn2 << 9) | ppn1
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

// ==================== Page Table ====================

/// A single page table (4KB, 512 entries)
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

    /// Get page table entry at index
    #[inline]
    pub fn get(&self, index: usize) -> PageTableEntry {
        self.entries[index]
    }

    /// Set page table entry at index
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

/// Supervisor Address Translation and Protection (satp) register
///
/// Format (64-bit):
/// - [63:60] MODE   - Address translation mode
/// - [59:44] ASID   - Address Space Identifier
/// - [43:0]  PPN    - Root page table physical page number
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Satp(pub u64);

impl Satp {
    /// Bare mode (no address translation)
    pub const MODE_BARE: u64 = 0;

    /// Sv39 mode (39-bit virtual address)
    pub const MODE_SV39: u64 = 8;

    /// Create satp value from components
    #[inline]
    pub const fn new(mode: u64, asid: u16, ppn: u64) -> Self {
        Self(((mode as u64) << 60) | ((asid as u64) << 44) | (ppn & 0x0FFFFFFFFFFFFFFF))
    }

    /// Create Sv39 satp
    #[inline]
    pub const fn sv39(ppn: u64, asid: u16) -> Self {
        Self::new(Self::MODE_SV39, asid, ppn)
    }

    /// Get raw bits value
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    /// Get translation mode
    #[inline]
    pub fn mode(&self) -> u64 {
        self.0 >> 60
    }

    /// Get ASID
    #[inline]
    pub fn asid(&self) -> u16 {
        ((self.0 >> 44) & 0xFFFF) as u16
    }

    /// Get root PPN
    #[inline]
    pub fn ppn(&self) -> u64 {
        self.0 & 0x0FFFFFFFFFFFFFFF
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

/// Get current satp value
pub fn get_satp() -> Satp {
    unsafe {
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);
        Satp(satp)
    }
}
