//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 page-table entries — typed codec (verification-shaped: pure
//! encode/decode functions, per docs/development/rust-kernel-best-practices.md §7).
//!
//! x86 PTE bits vs the arch-interface semantic names:
//!
//! | Interface name | x86 bit | Note |
//! |---|---|---|
//! | `V` | PRESENT (1<<0) | |
//! | `R` | (0) | present ⇒ readable on x86; alias is a no-op so shared flag-OR code works |
//! | `W` | RW (1<<1) | |
//! | `U` | USER (1<<2) | |
//! | `G` | GLOBAL (1<<8) | |
//! | `A` | ACCESSED (1<<5) | |
//! | `D` | DIRTY (1<<6) | |
//! | `X` | (0) | executable unless NX; non-exec is expressed with `NX` |
//! | `IO` | PWT\|PCD | uncached device memory |

/// Page table entry (also page-directory entries: non-leaf PTEs point down a level)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    // ---- raw x86 bits ----
    pub const P: u64 = 1 << 0;
    pub const RW: u64 = 1 << 1;
    pub const US: u64 = 1 << 2;
    pub const PWT: u64 = 1 << 3;
    pub const PCD: u64 = 1 << 4;
    pub const ACCESSED: u64 = 1 << 5;
    pub const DIRTY: u64 = 1 << 6;
    pub const PS: u64 = 1 << 7;      // large page at PMD/PUD level
    pub const GLOBAL: u64 = 1 << 8;
    pub const NX: u64 = 1 << 63;

    // ---- arch-interface semantic aliases ----
    pub const V: u64 = Self::P;
    pub const R: u64 = 0;            // readable is implied by PRESENT
    pub const W: u64 = Self::RW;
    pub const U: u64 = Self::US;
    pub const G: u64 = Self::GLOBAL;
    pub const A: u64 = Self::ACCESSED;
    pub const D: u64 = Self::DIRTY;
    pub const X: u64 = 0;            // exec unless NX
    /// Device/uncached memory
    pub const IO: u64 = Self::PWT | Self::PCD;

    /// Physical-address bits [51:12] mask
    const PHYS_MASK: u64 = 0x000f_ffff_ffff_f000;

    /// Public alias of the physical-address bits mask (read-only codec
    /// detail exposed for the mapping/walk code in mmu_init/mm_ops).
    pub const PHYS_MASK_PUBLIC: u64 = Self::PHYS_MASK;

    pub const fn new() -> Self {
        PageTableEntry(0)
    }

    pub const fn from_bits(bits: u64) -> Self {
        PageTableEntry(bits)
    }

    pub fn bits(&self) -> u64 {
        self.0
    }

    pub fn is_valid(&self) -> bool {
        self.0 & Self::P != 0
    }

    pub fn is_readable(&self) -> bool {
        self.is_valid()
    }

    pub fn is_writable(&self) -> bool {
        self.0 & Self::RW != 0
    }

    pub fn is_executable(&self) -> bool {
        self.is_valid() && self.0 & Self::NX == 0
    }

    pub fn is_user(&self) -> bool {
        self.0 & Self::US != 0
    }

    /// Is this a leaf mapping? (At PML4/PUD-without-PS level it is a link.)
    pub fn is_leaf(&self) -> bool {
        self.is_valid() && self.0 & Self::PS != 0
    }

    /// Is this a link to the next level?
    pub fn is_table(&self) -> bool {
        self.is_valid() && self.0 & Self::PS == 0
    }

    /// Page frame number (physical address >> 12)
    pub fn ppn(&self) -> u64 {
        (self.0 & Self::PHYS_MASK) >> 12
    }

    /// Physical address of the mapped frame
    pub fn phys_addr(&self) -> u64 {
        self.0 & Self::PHYS_MASK
    }

    /// Link entry pointing at the next-level table at `ppn`
    ///
    /// Carries U (US) in addition to P|RW: SDM 4.6 grants a CPL3 access
    /// only when U/S = 1 in EVERY paging-structure entry of the
    /// translation, so an intermediate link without U makes the whole
    /// subtree supervisor-only even when the leaf has U — the user
    /// image at 0x400000 faulted e=5 (present, user access denied)
    /// forever on exactly this.  Kernel tables are unaffected: their
    /// leaves keep U=0, and the combined rule still denies user access
    /// one level below the link.
    pub const fn new_table(ppn: u64) -> Self {
        PageTableEntry((ppn << 12) | Self::P | Self::RW | Self::US)
    }

    /// Kernel read/write page mapping
    pub const fn new_page_kernel(ppn: u64) -> Self {
        PageTableEntry((ppn << 12) | Self::P | Self::RW | Self::ACCESSED | Self::DIRTY)
    }

    /// User read/write page mapping
    pub const fn new_page_user(ppn: u64) -> Self {
        PageTableEntry((ppn << 12) | Self::P | Self::RW | Self::US | Self::ACCESSED | Self::DIRTY)
    }

    /// User read-only page mapping
    pub const fn new_page_ro(ppn: u64) -> Self {
        PageTableEntry((ppn << 12) | Self::P | Self::US | Self::ACCESSED)
    }
}

// ==================== Page table ====================

/// One page-table page (512 entries of 8 bytes)
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; 512],
}

impl PageTable {
    pub const fn new() -> Self {
        PageTable { entries: [0; 512] }
    }

    pub fn get(&self, index: usize) -> PageTableEntry {
        PageTableEntry::from_bits(self.entries[index])
    }

    pub fn set(&mut self, index: usize, entry: PageTableEntry) {
        self.entries[index] = entry.bits();
    }

    pub fn zero(&mut self) {
        self.entries.fill(0);
    }
}

// ==================== CR3 (the x86 "satp") ====================

/// CR3 register value (page-table root)
#[derive(Debug, Clone, Copy)]
pub struct Satp(pub u64);

impl Satp {
    /// No mode bits on x86: CR3 low bits are PCID/flags; we run PCID-off
    pub const fn new(_mode: u64, _asid: u16, ppn: u64) -> Self {
        Satp(ppn << 12)
    }

    pub fn bits(&self) -> u64 {
        self.0
    }

    pub fn ppn(&self) -> u64 {
        self.0 >> 12
    }

    pub fn asid(&self) -> u16 {
        0
    }
}

/// Read the current CR3 as a Satp-shaped value
pub fn get_satp() -> Satp {
    Satp(super::super::cpu::read_cr3())
}
