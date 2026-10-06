//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 MMU initialization and page mapping.
//!
//! X86-TODO(agent x86-mm): full implementation. This stub pins the arch
//! interface contract so the whole tree compiles with `--features x86_64`.
//!
//! Bring-up contract:
//! - `boot.S` installs bootstrap tables (identity low + higher-half 2MB map)
//! - `init()` runs while those are still active; it may parse the multiboot
//!   memory map (arch::boot::boot_memory_regions)
//! - `setup_linear_mapping()` builds the real kernel PML4: linear map at
//!   PAGE_OFFSET, kernel image at KERNEL_LINK_BASE, vmemmap, fixmap; the
//!   switch to the new CR3 must keep the higher-half text mapping live
//!   (identity low map may be dropped after the switch)

use super::memory_layout::*;
use super::pagetable::*;
use crate::println;

/// Kernel root page table (PML4) — the Rust-built one
pub static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

/// Physical address of the currently active root (starts at the bootstrap
/// PML4 built by boot.S)
pub unsafe fn root_page_table_ppn() -> u64 {
    crate::arch::cpu::read_cr3() >> 12
}

/// Page-table allocation stage (early = memblock bump, late = zone allocator)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocStage {
    Early,
    Fixmap,
    Late,
}

static ALLOC_STAGE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub fn get_alloc_stage() -> AllocStage {
    match ALLOC_STAGE.load(core::sync::atomic::Ordering::Acquire) {
        0 => AllocStage::Early,
        1 => AllocStage::Fixmap,
        _ => AllocStage::Late,
    }
}

pub fn alloc_stage_is_late() -> bool {
    matches!(get_alloc_stage(), AllocStage::Late)
}

pub fn pt_ops_set_fixmap() {
    ALLOC_STAGE.store(1, core::sync::atomic::Ordering::Release);
}

pub fn pt_ops_set_late() {
    ALLOC_STAGE.store(2, core::sync::atomic::Ordering::Release);
}

/// Allocate one page-table page. X86-TODO: early-stage memblock bump +
/// late-stage zone allocation with PtLedger accounting (see riscv64 twin).
pub unsafe fn alloc_page_table() -> Option<u64> {
    None
}

/// Physical page-table address → kernel virtual (linear map)
pub unsafe fn get_page_table_virt(phys_addr: u64) -> *mut PageTable {
    phys_to_virt(PhysAddr::new(phys_addr)).0 as *mut PageTable
}

// ==================== PtLedger (live-table accounting) ====================

/// Page-table ledger: tracks which physical pages are live page tables so
/// freeing user page tables never frees data pages and vice versa.
/// X86-TODO(agent x86-mm): port the bitmap + ring forensics from riscv64.
pub struct PtLedger {
    pub tracked: core::sync::atomic::AtomicUsize,
}

impl PtLedger {
    pub const fn new() -> Self {
        PtLedger { tracked: core::sync::atomic::AtomicUsize::new(0) }
    }

    /// Is this physical page a live page table?
    pub fn is_live_table(ppn: u64) -> bool {
        let _ = ppn;
        false // X86-TODO: real ledger
    }
}

pub static PT_LEDGER: PtLedger = PtLedger::new();

/// Free-table forensic ring entry (kept for interface parity)
pub struct PtFreeEntry;
impl PtFreeEntry {
    pub const NEW: Self = PtFreeEntry;
}
pub static PT_FREE_RING: [PtFreeEntry; 1] = [PtFreeEntry::NEW];
pub static PT_FREE_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
pub static PT_STAMP_RING: [PtFreeEntry; 1] = [PtFreeEntry::NEW];
pub static PT_STAMP_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Fork-root registry: user page-table roots created by fork, so teardown
/// can distinguish them from kernel roots.
pub struct ForkRootReg {
    pub root_ppn: core::sync::atomic::AtomicU64,
    pub owner_pid: core::sync::atomic::AtomicU32,
}
impl ForkRootReg {
    pub const NEW: Self = ForkRootReg {
        root_ppn: core::sync::atomic::AtomicU64::new(0),
        owner_pid: core::sync::atomic::AtomicU32::new(0),
    };
}
pub static FORK_ROOTS: [ForkRootReg; 256] = [ForkRootReg::NEW; 256];
pub static FORK_ROOT_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub fn register_fork_root(root_ppn: u64, owner_pid: u32) {
    let _ = (root_ppn, owner_pid); // X86-TODO
}

/// Free-user-table forensic ring (interface parity)
pub struct FutEntry;
impl FutEntry {
    pub const NEW: Self = FutEntry;
}
pub static FUT_RING: [FutEntry; 1] = [FutEntry::NEW];
pub static FUT_RING_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
pub static FUT_REPEAT_REPORTS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// PTE-install forensic ring (interface parity)
pub struct PteInstall;
impl PteInstall {
    pub const NEW: Self = PteInstall;
}
pub static PTEI_RING: [PteInstall; 1] = [PteInstall::NEW];
pub static PTEI_CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub fn pte_install_log(root_ppn: u64, va: u64, ppn: u64) {
    let _ = (root_ppn, va, ppn); // X86-TODO
}

/// Free all page-table pages of a user address space.
/// X86-TODO(agent x86-mm): 4-level walk + ledger-checked free.
pub unsafe fn free_user_page_tables(root_ppn: u64) {
    let _ = root_ppn;
}

/// Map one page into the given root (flags: PageTableEntry semantic bits).
/// X86-TODO(agent x86-mm): 4-level walk with intermediate allocation.
pub unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let _ = (root_ppn, virt, phys, flags);
}

/// Map into the kernel root
pub unsafe fn map_kernel_page(virt: u64, phys: u64, flags: u64) {
    map_page(root_page_table_ppn(), VirtAddr::new(virt), PhysAddr::new(phys), flags);
}

pub unsafe fn map_kernel_region(virt: u64, phys: u64, size: u64, flags: u64) {
    let mut v = virt & !(PAGE_SIZE - 1);
    let mut p = phys & !(PAGE_SIZE - 1);
    let end = virt + size;
    while v < end {
        map_kernel_page(v, p, flags);
        v += PAGE_SIZE;
        p += PAGE_SIZE;
    }
}

/// PTE of the kernel device window at `va` (fixed ECAM/IOAPIC mappings)
pub fn kernel_device_window_pte(va: u64) -> Option<u64> {
    let _ = va;
    None // X86-TODO
}

/// arch::mm::init() — called from main.rs early.
pub fn init() {
    println!("mm: x86_64 4-level page tables (bootstrap tables active)");
    // X86-TODO(agent x86-mm): build the real kernel PML4 here.
}

/// Build the real linear/device mappings (called from main.rs with the
/// multiboot memory regions).
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    // X86-TODO(agent x86-mm): linear map all usable RAM at PAGE_OFFSET with
    // 2MB pages, map kernel image window, install vmemmap/fixmap PTEs,
    // then switch CR3.
    for r in memory_regions {
        println!("memblock: base={:#x} size={:#x}", r.base, r.size);
    }
}

/// Device/ECAM mappings (q35: MMCONFIG @ 0xb0000000, IOAPIC, LAPIC)
pub fn setup_device_mappings() {
    // X86-TODO(agent x86-mm): map ECAM + IOAPIC + LAPIC uncached (IO bits).
}
