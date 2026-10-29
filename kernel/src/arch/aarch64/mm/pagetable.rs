//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 Translation Table Structures
//!
//! # Descriptor model
//!
//! `PageTableEntry` IS an AArch64 stage-1 descriptor (the hardware walks
//! these tables), but the generic kernel code composes PERMISSION
//! BITMASKS from riscv64-style named consts (V/R/W/X/U/A/D) and ORs them
//! into PTE flag fields. To keep that code arch-portable, those consts are
//! defined as SOFTWARE bits in the descriptor's upper half — bits the
//! cortex-a72/QEMU hardware ignores (PBHA 63:59 unimplemented, 58:55
//! architecturally software-use). The arch mm code synthesizes the real
//! hardware attributes (AttrIndx, AP, UXN/PXN, AF, nG) from the software
//! bits whenever it installs a descriptor into a live table.
//!
//! Hardware descriptor format (4K granule, stage 1):
//! - [0]     V      - valid
//! - [1]     TYPE   - 0 = table pointer, 1 = block/page (leaf)
//! - [4:2]   AttrIndx - MAIR index (0 normal, 1 device, 2 normal-noncacheable)
//! - [5]     NS     - non-secure (0)
//! - [6]     AP[1]  - 1 = accessible at EL0 (the hardware "user" bit)
//! - [7]     AP[2]  - 1 = read-only
//! - [9:8]   SH     - shareability (3 = inner-shareable)
//! - [10]    AF     - access flag (hardware faults if 0; we always set it
//!                    and track young/dirty in the software A/D bits)
//! - [11]    nG     - non-global
//! - [47:12] output address (PA bits 47:12)
//! - [50]    GP     - not implemented on A72 (RES0)
//! - [51]    DBM    - not implemented on A72 (RES0)
//! - [52]    Contig - contiguous hint (0)
//! - [53]    PXN    - privileged execute-never
//! - [54]    UXN    - unprivileged execute-never
//! - [58:55] software use
//! - [63:59] PBHA   - not implemented on A72 (RES0) — used as software
//!
//! Software permission bits (Rux contract, same semantics as riscv64):
//! - V = 1<<0 (hardware valid, doubles as the software "present" bit)
//! - R = 1<<55, W = 1<<56, X = 1<<63, U = 1<<57, G = 1<<58
//! - A = 1<<59 (young), D = 1<<60 (dirty)
//! - COW/PCACHE (mm_ops::cow_flags) = 1<<61 / 1<<62
//! - NX = UXN (1<<54) — clear-mask users in generic code clear execute-never

use super::memory_layout::{PAGE_SIZE, PAGE_OFFSET_MASK};

// ==================== Page Table Entry ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    // ---- hardware bits ----
    /// V (Valid) - bit 0 (hardware)
    pub const V: u64 = 1 << 0;
    /// Descriptor type encoding (bits [1:0], per ARM DDI / Linux
    /// pgtable-hwdef PMD_TYPE_*): 0b01 = BLOCK mapping (L1 1GB / L2 2MB),
    /// 0b11 = TABLE pointer (L0-L2) or PAGE mapping (L3). (The original
    /// port draft had this polarity INVERTED — the boot-time level-0
    /// translation-fault loop.)
    pub const TYPE_BLOCK: u64 = 0b01;
    pub const TYPE_TABLE: u64 = 0b11;
    pub const TYPE_PAGE: u64 = 0b11;
    /// Type field mask
    pub const TYPE_MASK: u64 = 0b11;
    /// AttrIndx field (bits 4:2)
    pub const ATTR_INDX_MASK: u64 = 0b111 << 2;
    /// Normal cacheable memory (MAIR index 0)
    pub const ATTR_NORMAL: u64 = 0b000 << 2;
    /// Device nGnRE memory (MAIR index 1)
    pub const ATTR_DEVICE: u64 = 0b001 << 2;
    /// Normal non-cacheable (MAIR index 2)
    pub const ATTR_NORMAL_NC: u64 = 0b010 << 2;
    /// AP[1] (bit 6): EL0 access (the hardware user bit)
    pub const AP_EL0: u64 = 1 << 6;
    /// AP[2] (bit 7): read-only
    pub const AP_RO: u64 = 1 << 7;
    /// SH (bits 9:8): inner-shareable
    pub const SH_IS: u64 = 0b11 << 8;
    /// AF (bit 10): access flag — always set at map time
    pub const AF: u64 = 1 << 10;
    /// nG (bit 11): non-global
    pub const NG: u64 = 1 << 11;
    /// PXN (bit 53)
    pub const PXN: u64 = 1 << 53;
    /// UXN (bit 54)
    pub const UXN: u64 = 1 << 54;

    // ---- software permission bits (Rux contract) ----
    /// R (Readable) - software bit 55
    pub const R: u64 = 1 << 55;
    /// W (Writable) - software bit 56
    pub const W: u64 = 1 << 56;
    /// U (User/EL0) - software bit 57
    pub const U: u64 = 1 << 57;
    /// G (Global) - software bit 58
    pub const G: u64 = 1 << 58;
    /// A (Accessed/young) - software bit 59
    pub const A: u64 = 1 << 59;
    /// D (Dirty) - software bit 60
    pub const D: u64 = 1 << 60;
    /// X (Executable request) - software bit 63; synthesized to clearing
    /// PXN|UXN at descriptor install time
    pub const X: u64 = 1 << 63;
    /// NX (execute-never) — alias of the hardware UXN bit; generic code
    /// only ever CLEARS it in masks ("clear NX" = make executable)
    pub const NX: u64 = 1 << 54;

    /// IO/device memory type for MMIO descriptors
    pub const IO: u64 = Self::ATTR_DEVICE;

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

    /// Set raw bits
    #[inline]
    pub fn set_bits(&mut self, bits: u64) {
        self.0 = bits;
    }

    // ---- software permission predicates ----

    /// Check if valid
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.0 & Self::V != 0
    }

    /// Check if readable (software R bit)
    #[inline]
    pub fn is_readable(&self) -> bool {
        self.0 & Self::R != 0
    }

    /// Check if writable (software W bit)
    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & Self::W != 0
    }

    /// Check if executable (software X bit and UXN clear)
    #[inline]
    pub fn is_executable(&self) -> bool {
        self.0 & Self::X != 0 && self.0 & Self::UXN == 0
    }

    /// Check if user page (software U bit)
    #[inline]
    pub fn is_user(&self) -> bool {
        self.0 & Self::U != 0
    }

    /// Check if accessed/young (software A bit)
    #[inline]
    pub fn is_accessed(&self) -> bool {
        self.0 & Self::A != 0
    }

    /// Check if dirty (software D bit)
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.0 & Self::D != 0
    }

    /// Check if leaf BLOCK entry (bits[1:0] == 0b01 — L1/L2 mapping)
    #[inline]
    pub fn is_leaf(&self) -> bool {
        self.0 & Self::TYPE_MASK == Self::TYPE_BLOCK
    }

    /// Check if leaf entry at LEVEL 3 (bits[1:0] == 0b11 = page)
    #[inline]
    pub fn is_page(&self) -> bool {
        self.0 & Self::TYPE_MASK == Self::TYPE_PAGE
    }

    /// Check if table pointer (bits[1:0] == 0b11 at L0-L2)
    #[inline]
    pub fn is_table(&self) -> bool {
        self.is_valid() && self.0 & Self::TYPE_MASK == Self::TYPE_TABLE
    }

    /// Get output physical page number (output address bits [47:12])
    #[inline]
    pub fn ppn(&self) -> u64 {
        (self.0 >> 12) & 0x0000_FFFF_FFFF
    }

    /// Output address (physical address of the mapped frame)
    #[inline]
    pub fn output_addr(&self) -> u64 {
        self.0 & 0x0000_FFFF_FFFF_F000
    }

    /// Get the physical page number for a 2MB (L2 block) mapping —
    /// for block descriptors the low 9 output-address bits [20:12] are
    /// RES0, so ppn() is already the aligned 2MB frame number.
    #[inline]
    pub fn ppn_for_2mb_page(&self) -> u64 {
        self.ppn()
    }

    // ---- constructors: full descriptors synthesized from software flags ----

    /// Build the hardware attribute bits from the Rux software permission
    /// mask (V/R/W/X/U/A/D composed by generic code or arch mm code).
    ///
    /// normal: false → device memory (AttrIndx 1)
    #[inline]
    pub fn hw_attrs_from_flags(flags: u64, normal: bool) -> u64 {
        let mut hw = Self::AF | Self::SH_IS;
        hw |= if normal { Self::ATTR_NORMAL } else { Self::ATTR_DEVICE };
        if flags & Self::U != 0 {
            hw |= Self::AP_EL0;
        }
        if flags & Self::W == 0 {
            hw |= Self::AP_RO;
        }
        if flags & Self::X == 0 {
            hw |= Self::PXN | Self::UXN;
        }
        if flags & Self::G == 0 {
            hw |= Self::NG;
        }
        hw
    }

    /// Create a table-pointer descriptor to the next level
    #[inline]
    pub fn new_table(ppn: u64) -> Self {
        Self((ppn << 12) | Self::V | Self::TYPE_TABLE | Self::PXN_TABLE | Self::UXN_TABLE)
    }

    /// L3 PAGE descriptor (normal memory) from a Rux software flag mask
    /// (kernel RWX default; `flags` carries the Rux permission mask)
    #[inline]
    pub fn new_page_kernel_flags(ppn: u64, flags: u64) -> Self {
        Self((ppn << 12) | Self::V | Self::TYPE_PAGE
            | Self::hw_attrs_from_flags(flags, true)
            | (flags & Self::SW_MASK))
    }

    /// L1/L2 BLOCK descriptor (normal memory) from a Rux software flag mask
    #[inline]
    pub fn new_block_kernel_flags(ppn: u64, flags: u64) -> Self {
        Self((ppn << 12) | Self::V | Self::TYPE_BLOCK
            | Self::hw_attrs_from_flags(flags, true)
            | (flags & Self::SW_MASK))
    }

    /// L1/L2 BLOCK descriptor (device memory)
    #[inline]
    pub fn new_block_device(ppn: u64) -> Self {
        Self((ppn << 12) | Self::V | Self::TYPE_BLOCK
            | Self::hw_attrs_from_flags(Self::V | Self::R | Self::W | Self::G, false)
            | ((Self::V | Self::R | Self::W | Self::A | Self::D | Self::G) & Self::SW_MASK))
    }

    /// Create PTE pointing to physical page (kernel RWX, normal memory) —
    /// riscv64 interface parity
    #[inline]
    pub fn new_page_kernel(ppn: u64) -> Self {
        Self::new_page_kernel_flags(ppn, Self::V | Self::R | Self::W | Self::X | Self::A | Self::D | Self::G)
    }

    /// Create PTE pointing to physical page (user RWX) — riscv64 parity
    #[inline]
    pub fn new_page_user(ppn: u64) -> Self {
        Self::new_page_kernel_flags(ppn, Self::V | Self::R | Self::W | Self::X | Self::U | Self::A | Self::D)
    }

    /// Create PTE pointing to physical page (read-only, executable) —
    /// riscv64 parity
    #[inline]
    pub fn new_page_ro(ppn: u64) -> Self {
        Self::new_page_kernel_flags(ppn, Self::V | Self::R | Self::X | Self::A | Self::G)
    }

    /// Create a device (MMIO) page descriptor with kernel RW access —
    /// L3 PAGE descriptor (device memory)
    #[inline]
    pub fn new_page_device(ppn: u64) -> Self {
        Self((ppn << 12) | Self::V | Self::TYPE_PAGE
            | Self::hw_attrs_from_flags(Self::V | Self::R | Self::W | Self::G, false)
            | ((Self::V | Self::R | Self::W | Self::A | Self::D | Self::G) & Self::SW_MASK))
    }

    /// Mask of all software permission bits (kept alongside hw attrs)
    pub const SW_MASK: u64 = Self::R | Self::W | Self::U | Self::G | Self::A | Self::D | Self::X;

    /// Table-descriptor bits: PXNTable/UXNTable (bits 59:58 of TABLE
    /// descriptors — note these overlap the software-bit space, which is
    /// only used on BLOCK descriptors; table pointers carry no leaf perms)
    pub const PXN_TABLE: u64 = 1 << 59;
    pub const UXN_TABLE: u64 = 1 << 60;
}

impl core::ops::BitOr for PageTableEntry {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for PageTableEntry {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for PageTableEntry {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl core::ops::Not for PageTableEntry {
    type Output = Self;
    #[inline]
    fn not(self) -> Self {
        Self(!self.0)
    }
}

impl Default for PageTableEntry {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Page Table ====================

/// A single translation table (4KB, 512 entries)
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
        self.entries[index & 0x1FF]
    }

    /// Set page table entry at index
    #[inline]
    pub fn set(&mut self, index: usize, entry: PageTableEntry) {
        self.entries[index & 0x1FF] = entry;
    }

    /// Clear page table
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

// ==================== TTBR registers ====================
//
// On AArch64 the "satp" concept splits into TTBR0_EL1 (user root) and
// TTBR1_EL1 (kernel root). Rux keeps the riscv64-style single 64-bit
// "root register" ABSTRACTION: the composed value packs ASID and the
// TTBR0 root PPN the same way satp does, and the arch switch_mm path
// decomposes it into TTBR0_EL1 + ASID writes.
//
// Format (Rux aarch64 convention, mirrors riscv64 satp layout):
// - [63:60] MODE (8 = "long format active"; 0 = translation off)
// - [59:44] ASID
// - [43:0]  root PPN (TTBR0)
//
// NOTE: this composed value is a SOFTWARE abstraction (like the x86_64
// packed-CR3). The hardware ASID tag source is TTBR0_EL1[63:48]
// (TCR_EL1.A1 = 0, 8-bit ASIDs per TCR_EL1.AS = 0); write_satp
// decomposes the composed value into that form.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Satp(pub u64);

impl Satp {
    /// Bare mode (no address translation)
    pub const MODE_BARE: u64 = 0;
    /// Long-format (4K granule, 48-bit VA) active
    pub const MODE_LONG: u64 = 8;

    #[inline]
    pub const fn new(mode: u64, asid: u16, ppn: u64) -> Self {
        Self(((mode as u64) << 60) | ((asid as u64) << 44) | (ppn & 0x0FFFFFFFFFFFFFFF))
    }

    #[inline]
    pub const fn long(ppn: u64, asid: u16) -> Self {
        Self::new(Self::MODE_LONG, asid, ppn)
    }

    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn mode(&self) -> u64 {
        self.0 >> 60
    }

    #[inline]
    pub fn asid(&self) -> u16 {
        ((self.0 >> 44) & 0xFFFF) as u16
    }

    #[inline]
    pub fn ppn(&self) -> u64 {
        self.0 & 0x0FFFFFFFFFFFFFFF
    }

    #[inline]
    pub fn is_bare(&self) -> bool {
        self.mode() == Self::MODE_BARE
    }

    #[inline]
    pub fn is_long(&self) -> bool {
        self.mode() == Self::MODE_LONG
    }
}

/// Get the current composed "satp" (TTBR0_EL1 root + ASID). The ASID is
/// read from TTBR0_EL1[63:48] (the TCR_EL1.A1=0 tag source), the root
/// from TTBR0_EL1[47:1] — not CONTEXTIDR_EL1, which is a trace/debug
/// register and does not drive TLB tags.
pub fn get_satp() -> Satp {
    let ttbr0: u64;
    // SAFETY: reading TTBR0_EL1 is a pure system-register read, always
    // accessible at EL1.
    unsafe {
        core::arch::asm!(
            "mrs {t}, ttbr0_el1",
            t = out(reg) ttbr0,
            options(nomem, nostack, preserves_flags)
        );
    }
    let asid = ((ttbr0 >> 48) & 0xFFFF) as u16;
    Satp::long((ttbr0 & 0x0000_FFFF_FFFF_F000) >> 12, asid)
}
