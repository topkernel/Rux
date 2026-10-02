//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V MMU Initialization and Page Mapping
//!
//! This module contains:
//! - Page table allocation (early/fixmap/late stages)
//! - MMU initialization functions
//! - Page mapping functions (map_page, map_region, etc.)
//! - Linear mapping setup
//! - Device mapping functions

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use super::memory_layout::*;
use super::pagetable::*;
use crate::mm::{MmStruct, alloc_pages, free_pages, GfpFlags};
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};

// ==================== Assembly Page Tables (defined in boot.S) ====================

/// Trampoline page directory - maps only first 2MB of kernel
extern "C" {
    /// Trampoline PGD - minimal mapping for MMU enable
    pub static trampoline_pg_dir: [u8; 4096];
    /// Trampoline PMD - contains 2MB kernel mapping
    pub static trampoline_pmd: [u8; 4096];
    /// Early PGD - full early mapping
    pub static early_pg_dir: [u8; 4096];
    /// Early PMD for kernel region
    pub static early_pmd: [u8; 4096];
    /// Early PMD for device region
    pub static early_pmd_dev: [u8; 4096];
}

// ==================== Root Page Table ====================

#[link_section = ".bss"]
pub static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

/// Get the physical page number of the root page table.
/// ROOT_PAGE_TABLE is at KERNEL_LINK_ADDR (virtual), so we must convert to physical.
#[inline]
pub unsafe fn root_page_table_ppn() -> u64 {
    let root_virt = &raw mut ROOT_PAGE_TABLE as u64;
    let root_phys = root_virt.wrapping_sub(KERNEL_MAP.va_kernel_pa_offset as u64);
    root_phys / PAGE_SIZE
}

static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

// TRAP_STACKS + get_trap_stack() REMOVED (wave-6, review 2.6): 16KB x
// MAX_CPUS of .bss with zero callers (kernel traps run on the current
// task stack; IRQ stacks live in the interrupt subsystem).

// ==================== Page Table Allocation ====================
//
// Three-stage page table allocation:
// 1. Early: Static arrays (MMU not enabled yet, identity mapping)
// 2. Fixmap: memblock allocation (MMU enabled, but buddy not ready)
// 3. Late: Buddy allocator (full memory management available)

/// Number of early page tables
/// For 2GB memory with linear mapping, we need up to 512 PMD entries (2GB / 2MB = 1024)
/// Each PMD page table holds 512 entries, so we need 2 PMD tables for full 2GB coverage
/// But we also need page tables for vmemmap and other mappings
const NUM_EARLY_PMD: usize = 8;   // L1 page tables (covers 8GB virtual space)
const NUM_EARLY_PTE: usize = 128; // L0 page tables (covers 256MB mapped space)

/// Early page tables for boot
#[link_section = ".bss"]
static mut EARLY_PMD: [PageTable; NUM_EARLY_PMD] = [PageTable::new(); NUM_EARLY_PMD];
#[link_section = ".bss"]
static mut EARLY_PTE: [PageTable; NUM_EARLY_PTE] = [PageTable::new(); NUM_EARLY_PTE];

/// Counter for early page table allocation
static EARLY_PMD_NEXT: AtomicUsize = AtomicUsize::new(0);
static EARLY_PTE_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Allocation stage tracking
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AllocStage {
    /// Early boot: MMU not fully enabled, use static arrays with identity mapping
    Early,
    /// Fixmap stage: MMU enabled, use memblock allocation
    Fixmap,
    /// Late stage: Buddy allocator ready, use normal page allocation
    Late,
}

impl AllocStage {
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => AllocStage::Early,
            1 => AllocStage::Fixmap,
            2 => AllocStage::Late,
            _ => AllocStage::Late,
        }
    }
}

/// Current allocation stage
static ALLOC_STAGE: AtomicU8 = AtomicU8::new(AllocStage::Early as u8);

/// Get current allocation stage
pub fn get_alloc_stage() -> AllocStage {
    AllocStage::from_u8(ALLOC_STAGE.load(Ordering::Acquire))
}

/// Transition to fixmap stage (MMU enabled, can use memblock)
pub fn pt_ops_set_fixmap() {
    ALLOC_STAGE.store(AllocStage::Fixmap as u8, Ordering::Release);
}

/// Transition to late stage (buddy allocator ready)
pub fn pt_ops_set_late() {
    ALLOC_STAGE.store(AllocStage::Late as u8, Ordering::Release);
}

/// Check if frame allocator is ready
#[inline]
fn is_frame_allocator_ready() -> bool {
    get_alloc_stage() == AllocStage::Late
}

/// Allocate a page table and return its physical address
///
/// Three-stage allocation:
/// - Early: static arrays (identity mapped)
/// - Fixmap: memblock allocation (linear mapped)
/// - Late: buddy allocator (linear mapped)
pub unsafe fn alloc_page_table() -> Option<u64> {
    let stage = get_alloc_stage();
    match stage {
        AllocStage::Early => {
            // Early boot: use static arrays in BSS (at KERNEL_LINK_ADDR)
            // Convert virtual address to physical: phys = virt - va_kernel_pa_offset
            let offset = KERNEL_MAP.va_kernel_pa_offset as u64;
            let pmd_idx = EARLY_PMD_NEXT.fetch_add(1, Ordering::AcqRel);
            if pmd_idx < NUM_EARLY_PMD {
                let table_virt = &EARLY_PMD[pmd_idx] as *const PageTable as u64;
                let table_phys = table_virt.wrapping_sub(offset);
                core::ptr::write_bytes(table_virt as *mut u8, 0, PAGE_SIZE as usize);
                return Some(table_phys);
            }

            let pte_idx = EARLY_PTE_NEXT.fetch_add(1, Ordering::AcqRel);
            if pte_idx < NUM_EARLY_PTE {
                let table_virt = &EARLY_PTE[pte_idx] as *const PageTable as u64;
                let table_phys = table_virt.wrapping_sub(offset);
                core::ptr::write_bytes(table_virt as *mut u8, 0, PAGE_SIZE as usize);
                return Some(table_phys);
            }

            panic!("mm: Out of early page tables (PMD: {}, PTE: {})", pmd_idx, pte_idx);
        }
        AllocStage::Fixmap => {
            // Fixmap stage: use memblock allocation
            let phys_addr = crate::mm::memblock::memblock_phys_alloc()?;

            // FORENSIC→FIX: boot-stage tables are shared by early mms —
            // mark permanent so teardown never frees them.
            PT_LEDGER.stamp_boot(phys_addr as u64 >> PAGE_SHIFT);

            // Use linear mapping (must be available at this point)
            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(virt_addr.bits() as *mut u8, 0, PAGE_SIZE as usize);
            Some(phys_addr as u64)
        }
        AllocStage::Late => {
            // Late stage: use zone allocator
            use crate::mm::zone::ZoneType;

            let phys_addr = if let Some(node) = crate::mm::pglist::first_online_node_mut() {
                if let Some(zone) = node.zone_mut(ZoneType::ZoneNormal) {
                    if zone.is_initialized() {
                        if let Some(pfn) = zone.alloc_pages(0) {
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

            // DFX memwatch: page-table frames bypass page_alloc::alloc_pages
            // (direct zone.alloc_pages above) — feed them to the page-level
            // site table from THIS frame so the chain names the mapping
            // path (map_page_noflush / create_user_address_space / ...).
            if crate::dfx::memwatch::armed() {
                let mut mw_frames: [u64; crate::dfx::memwatch::SITE_FRAMES] =
                    [0; crate::dfx::memwatch::SITE_FRAMES];
                let mw_s0: u64;
                unsafe {
                    core::arch::asm!("mv {s}, s0", s = out(reg) mw_s0, options(nomem, nostack));
                    crate::dfx::memwatch::walk_fp_chain(mw_s0, &mut mw_frames);
                }
                crate::dfx::memwatch::note_page_alloc(phys_addr as usize, 0, &mw_frames);
            }

            // FORENSIC ledger: stamp every Late-stage table allocation.
            PT_LEDGER.stamp(phys_addr >> PAGE_SHIFT, crate::sched::get_current_pid());

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
            // Early stage: EARLY_PMD/PTE are static arrays in BSS (at KERNEL_LINK_ADDR).
            // alloc_page_table returns their physical addresses.
            // Convert using va_kernel_pa_offset (= KERNEL_LINK_ADDR - KERNEL_PHYS).
            let offset = unsafe { KERNEL_MAP.va_kernel_pa_offset } as u64;
            let virt_addr = phys_addr.wrapping_add(offset);
            virt_addr as *mut PageTable
        }
        AllocStage::Fixmap => {
            // Linear mapping should be available at this point
            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            virt_addr.bits() as *mut PageTable
        }
        AllocStage::Late => {
            // Use linear mapping after buddy allocator is ready
            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            virt_addr.bits() as *mut PageTable
        }
    }
}

unsafe fn free_page_table_checked(phys_addr: u64, site: &str) {
    if get_alloc_stage() == AllocStage::Late {
        stamp_kernel_tree_boot();
        if !PT_LEDGER.take_returns(phys_addr >> PAGE_SHIFT) {
            return; // boot-shared table: never free, never reuse
        }
        if !PT_LEDGER.take(phys_addr >> PAGE_SHIFT, crate::sched::get_current_pid(), site) {
            return; // double/foreign free: refuse, keep the frame leaked
                    // rather than wired into two live trees
        }
    }
    crate::dfx::memwatch::FUT_TABLES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    free_page_table(phys_addr)
}

static BOOT_TREE_STAMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// One-time: mark every page-table frame wired into the kernel's ROOT page
/// table as boot-permanent. Early boot builds tables by hand (not through
/// alloc_page_table), and user mms forked from the initial template can
/// inherit references to them; their teardown then freed those SHARED
/// frames (observed: the 0x8cf62+ consecutive "no live stamp" double-free
/// block). Called from the teardown path (PTE_MODIFY_LOCK held) before any
/// table bookkeeping.
unsafe fn stamp_kernel_tree_boot() {
    use core::sync::atomic::Ordering;
    use crate::mm::phys_valid;
    if BOOT_TREE_STAMPED.load(Ordering::Acquire) {
        return;
    }
    BOOT_TREE_STAMPED.store(true, Ordering::Release);
    // Boot-permanent tables are exactly the frames reachable from the
    // kernel ROOT tree below (plus the Fixmap-stage tables stamped at
    // their allocation, and the EARLY_PMD/PTE static arrays range-checked
    // in free_page_table).
    //
    // The loop that used to be here stamped EVERY memblock-reserved frame
    // (kernel image, 128MB kernel heap, slab region, vmemmap — tens of
    // thousands of pages) into the ledger's 16384-slot boot mask. The mask
    // is a direct hash (slot = ppn & 16383), so it was 100% saturated: the
    // kernel heap alone spans every slot twice. take_returns() then
    // returned false for EVERY Late-stage page-table frame and
    // free_page_table_checked silently refused every table free — every
    // user mm leaked all of its page-table pages at exit (the exec/fork
    // baseline leak: ~100KB per exec, 2GB gone in ~21 min under load;
    // memwatch evidence: fut_calls=618, fut_tables=0).
    let root_virt = &raw mut ROOT_PAGE_TABLE as u64;
    let root_phys = root_virt.wrapping_sub(KERNEL_MAP.va_kernel_pa_offset as u64);
    for vpn2 in 0..512usize {
        let pte2 = ROOT_PAGE_TABLE.get(vpn2);
        if !pte2.is_valid() || pte2.is_leaf() {
            continue;
        }
        let t1_phys = pte2.ppn() << PAGE_SHIFT;
        if !phys_valid(t1_phys as usize) || t1_phys == root_phys {
            continue;
        }
        PT_LEDGER.stamp_boot(pte2.ppn());
        let t1 = get_page_table_virt(t1_phys);
        for vpn1 in 0..512usize {
            let pte1 = (*t1).get(vpn1);
            if !pte1.is_valid() || pte1.is_leaf() {
                continue;
            }
            let t0_phys = pte1.ppn() << PAGE_SHIFT;
            if !phys_valid(t0_phys as usize) || t0_phys == root_phys {
                continue;
            }
            PT_LEDGER.stamp_boot(pte1.ppn());
        }
    }
}

/// Free a page table (only valid for late stage allocations)
unsafe fn free_page_table(phys_addr: u64) {
    if get_alloc_stage() != AllocStage::Late {
        return;
    }

    // NOTE: the PT_LEDGER.take() that used to live here was the SECOND
    // take of the same frame (free_page_table_checked already took it);
    // its guaranteed failure printed a bogus "DOUBLE-FREE ... at
    // free_page_table" for every single legitimately-freed table.

    // Check if it's from early static region
    // Early tables live in BSS at KERNEL_LINK_ADDR; convert VA→PA using
    // va_kernel_pa_offset (same pattern as alloc_page_table).
    let offset = KERNEL_MAP.va_kernel_pa_offset as u64;
    let early_pmd_start = (&EARLY_PMD as *const _ as u64).wrapping_sub(offset);
    let early_pmd_end = early_pmd_start + (NUM_EARLY_PMD * PAGE_SIZE as usize) as u64;
    let early_pte_start = (&EARLY_PTE as *const _ as u64).wrapping_sub(offset);
    let early_pte_end = early_pte_start + (NUM_EARLY_PTE * PAGE_SIZE as usize) as u64;

    if (phys_addr >= early_pmd_start && phys_addr < early_pmd_end) ||
       (phys_addr >= early_pte_start && phys_addr < early_pte_end) {
        // Don't free early page tables
        return;
    }

    // Use zone allocator to free
    crate::mm::page_alloc::free_pages(phys_addr as usize, 0);
}

// FORENSIC (temporary): page-table frame ledger — every Late-stage
// alloc_page_table stamps the PPN; free_page_table clears it. A stamp on
// an already-stamped PPN = double allocation; clearing an unstamped or
// already-cleared PPN = double free / foreign free. Both are silent
// address-space corruptors under concurrent fork/exec.
pub struct PtLedger {
    // Direct-hash: slot = ppn & MASK, value = ppn+1 (0 = empty). O(1),
    // no eviction churn; a slot is reused only when the previous ppn was
    // freed (cleared) or a genuine double-alloc collides.
    slots: [core::sync::atomic::AtomicU64; 16384],
    // Boot-permanent tables (kernel root tree + Early static + Fixmap):
    // referenced by every early mm — freeing them per-mm tears down shared
    // state and feeds the frames back for reuse as live page tables (the
    // concurrent fork/exec corruption family).
    //
    // EXACT membership (array of ppn+1, linear scan), NOT a hash: the old
    // `ppn & 16383` bool mask collided with unrelated user tables ~0.3% of
    // the time. A false "boot" on a USER table made fork share that
    // subtree between parent and child (cross-process PTE aliasing) and
    // made teardown skip its pages. The boot set is tiny (the kernel
    // tree's own tables, ~dozens), so the scan is cheap.
    boot: [core::sync::atomic::AtomicU64; 512],
    reported: core::sync::atomic::AtomicUsize,
    freed_by: [core::sync::atomic::AtomicU32; 16384],
}
impl PtLedger {
    const fn new() -> Self {
        Self {
            slots: [const { core::sync::atomic::AtomicU64::new(0) }; 16384],
            boot: [const { core::sync::atomic::AtomicU64::new(0) }; 512],
            reported: core::sync::atomic::AtomicUsize::new(0),
            freed_by: [const { core::sync::atomic::AtomicU32::new(0) }; 16384],
        }
    }
    fn stamp_boot(&self, ppn: u64) {
        for slot in self.boot.iter() {
            let v = slot.load(core::sync::atomic::Ordering::Relaxed);
            if v == ppn + 1 {
                return; // already stamped
            }
            if v == 0 {
                slot.store(ppn + 1, core::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
        // Table full (kernel tree is ~dozens of tables; 512 slots): refuse
        // to pretend — drop the stamp and let the frame be treated as
        // normal. Should be unreachable; loudly noted if it ever happens.
        crate::pr_err!("PTLEDGER: boot table overflow, ppn={:#x}", ppn);
    }
    fn take_returns(&self, ppn: u64) -> bool {
        for slot in self.boot.iter() {
            let v = slot.load(core::sync::atomic::Ordering::Relaxed);
            if v == 0 {
                break;
            }
            if v == ppn + 1 {
                return false;
            }
        }
        true
    }
    fn stamp(&self, ppn: u64, pid: u32) {
        use core::sync::atomic::Ordering::Relaxed;
        let slot = &self.slots[(ppn as usize) & (self.slots.len() - 1)];
        let v = slot.load(Relaxed);
        if v != 0 && v - 1 == ppn {
            crate::pr_err!(
                "PTLEDGER: DOUBLE-ALLOC ppn={:#x} by pid={} (still live)",
                ppn, pid
            );
        }
        slot.store(ppn + 1, Relaxed);
    }
    /// Returns true if the frame had a live stamp (a legitimate first
    /// free) and was cleared; false for double/foreign frees — the caller
    /// must then REFUSE to return the frame to the zone. A table frame
    /// entering the free list twice is what wires one physical page into
    /// two live page-table trees (the shared-pgd corruption family).
    fn take(&self, ppn: u64, pid: u32, site: &str) -> bool {
        use core::sync::atomic::Ordering::Relaxed;
        let i = (ppn as usize) & (self.slots.len() - 1);
        let slot = &self.slots[i];
        let v = slot.load(Relaxed);
        if v != 0 && v - 1 == ppn {
            slot.store(0, Relaxed);
            self.freed_by[i].store(pid, Relaxed);
            return true;
        }
        if self.reported.fetch_add(1, Relaxed) < 8 {
            crate::pr_err!(
                "PTLEDGER: DOUBLE-FREE ppn={:#x} at {} by pid={} REFUSED (prev-free pid={})",
                ppn, site, pid, self.freed_by[i].load(Relaxed)
            );
            let cur = FUT_RING_CURSOR.load(core::sync::atomic::Ordering::Relaxed);
            for k in 0..FUT_RING.len() {
                let idx = (cur + FUT_RING.len() - 1 - k) % FUT_RING.len();
                let r = FUT_RING[idx].root.load(core::sync::atomic::Ordering::Relaxed);
                let p = FUT_RING[idx].pid.load(core::sync::atomic::Ordering::Relaxed);
                if r != 0 {
                    crate::pr_err!("  FUT ring[{}] root={:#x} pid={}", idx, r, p);
                }
            }
        }
        false
    }
}
pub static PT_LEDGER: PtLedger = PtLedger::new();

impl PtLedger {
    /// Stamp a page-table root allocated OUTSIDE alloc_page_table
    /// (create_user_address_space draws the root from alloc_pages). Without
    /// this stamp the teardown's ledger take() refuses the root free and the
    /// root frame leaks with every mm.
    pub fn stamp_root(&self, ppn: u64) {
        self.stamp(ppn, crate::sched::get_current_pid());
    }

    /// Read-only: is this frame boot-permanent (shared kernel tree)?
    /// Fork and teardown use this to share/skip instead of copy/free.
    pub fn is_boot(&self, ppn: u64) -> bool {
        !self.take_returns(ppn)
    }
}

pub struct FutEntry {
    pub root: core::sync::atomic::AtomicU64,
    pub pid: core::sync::atomic::AtomicU64,
}
impl FutEntry {
    const fn new() -> Self {
        Self {
            root: core::sync::atomic::AtomicU64::new(0),
            pid: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
const FUT_NEW: FutEntry = FutEntry::new();
pub static FUT_RING: [FutEntry; 32] = [FUT_NEW; 32];
pub static FUT_RING_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Free all page tables and user data pages used by a user address space
///
/// Only frees USER space page tables (VPN2 0-255 with U=1).
/// Kernel mappings (U=0) are shared and should NOT be freed.
///
/// IMPORTANT: For non-leaf L2 entries, U bit is not meaningful (R/W/X=0).
/// We must walk all valid user-space L2 entries, not skip them based on U bit.

#[inline]
fn is_framebuffer_frame(phys: u64) -> bool {
    // Device frames (virtio-gpu framebuffer) are NOT RAM pages: tearing
    // them down through put_page/free_pages corrupts the page_desc
    // accounting (mapcount/refcount of an unrelated descriptor) — the
    // recycled-frame family's amplifier.
    if let Some(info) = crate::drivers::gpu::get_framebuffer_info() {
        return phys >= info.addr && phys < info.addr + info.size as u64;
    }
    false
}

// FORENSIC: PTE-install ledger — every user PTE installation records
// (root, va, ppn). At a crash we replay which roots EVER mapped the
// victim's frame: an alias installed without a matching allocation.
pub struct PteInstall {
    pub root: core::sync::atomic::AtomicU64,
    pub va: core::sync::atomic::AtomicU64,
    pub ppn: core::sync::atomic::AtomicU64,
}
impl PteInstall {
    const fn new() -> Self {
        Self {
            root: core::sync::atomic::AtomicU64::new(0),
            va: core::sync::atomic::AtomicU64::new(0),
            ppn: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
const PTEI_NEW: PteInstall = PteInstall::new();
pub static PTEI_RING: [PteInstall; 65536] = [PTEI_NEW; 65536];
pub static PTEI_CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[inline]
pub fn pte_install_log(root_ppn: u64, va: u64, ppn: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let i = PTEI_CUR.fetch_add(1, Relaxed) % PTEI_RING.len();
    PTEI_RING[i].root.store(root_ppn, Relaxed);
    PTEI_RING[i].va.store(va, Relaxed);
    PTEI_RING[i].ppn.store(ppn, Relaxed);
}

pub unsafe fn free_user_page_tables(root_ppn: u64) {
    use crate::mm::{pfn_to_page, pfn_to_page_mut, phys_to_pfn, phys_valid, page_desc::PageFlag, free_pages};
    crate::dfx::memwatch::FUT_CALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Double-teardown detector: the same root walked twice means two
    // MmStructs ended up owning one page-table tree — the second walk
    // chases recycled frames full of foreign data (the corruption family
    // behind do_wait children-list panics). Ring is small; a hit here is
    // not proof for old trees, but a fresh repeat IS.
    {
        use core::sync::atomic::Ordering::Relaxed;
        for k in 0..FUT_RING.len() {
            if FUT_RING[k].root.load(Relaxed) == root_ppn && root_ppn != 0 {
                crate::pr_err!(
                    "FUT: REPEAT teardown of root ppn={:#x} (ring[{}])",
                    root_ppn, k
                );
                break;
            }
        }
        let idx = FUT_RING_CURSOR.fetch_add(1, Relaxed) % FUT_RING.len();
        FUT_RING[idx].root.store(root_ppn, Relaxed);
        FUT_RING[idx].pid.store(crate::sched::get_current_pid() as u64, Relaxed);
    }
    // Serialize against concurrent fork copies / COW faults on ANY mm: the
    // pages freed here can be immediately reallocated as page tables or
    // COW copies by another CPU (PTE_MODIFY_LOCK, NEW2 class).
    let _pte_guard = super::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();

    let root_phys = root_ppn << PAGE_SHIFT;
    let root_table = get_page_table_virt(root_phys);

    // Walk and free all levels (only user space: VPN2 0-255)
    for vpn2 in 0..256 {
        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() {
            continue;
        }

        // Check if L2 is a leaf (1GB huge page)
        // For leaf entries, check U bit to skip kernel pages
        // For non-leaf entries, we must walk further (U bit not meaningful)
        let is_l2_leaf = pte2.is_leaf();

        if is_l2_leaf && !pte2.is_user() {
            // Kernel leaf page in user region - shouldn't happen, but skip
            continue;
        }

        if is_l2_leaf {
            let phys_addr = pte2.ppn() << PAGE_SHIFT;
            if is_framebuffer_frame(phys_addr) {
                continue;
            }
            if !phys_valid(phys_addr as usize) || phys_addr < 0x80000000 {
                continue;
            }
            let pfn = phys_to_pfn(phys_addr as usize);
            let page = pfn_to_page(pfn);
            if !page.is_null() {
                if (*page).is_mapped() {
                    crate::mm::rmap::page_remove_rmap(&*page);
                }
                let new_ref = (*page).put_page();
                if new_ref == 0 {
                    crate::dfx::memwatch::FUT_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    free_pages(phys_addr as usize, 0);
                }
            }
            continue;
        }

        let ppn1 = pte2.ppn();
        let table1_phys = ppn1 << PAGE_SHIFT;

        if !phys_valid(table1_phys as usize) {
            continue;
        }

        if table1_phys == root_phys {
            continue;
        }

        // Shared kernel subtree (MMIO/ECAM tables shared into every mm by
        // copy_kernel_mappings): nothing under it belongs to THIS mm — skip
        // the descent entirely (also saves walking ~140 tables' worth of
        // kernel-only entries per teardown).
        if get_alloc_stage() == AllocStage::Late && PT_LEDGER.is_boot(ppn1) {
            continue;
        }

        let table1 = get_page_table_virt(table1_phys);

        for vpn1 in 0..512 {
            let pte1 = (*table1).get(vpn1);
            if !pte1.is_valid() {
                continue;
            }

            if pte1.is_leaf() {
                // Skip kernel pages (shouldn't happen in user space, but check anyway)
                if !pte1.is_user() {
                    continue;
                }
                let phys_addr = pte1.ppn() << PAGE_SHIFT;
                if is_framebuffer_frame(phys_addr) {
                    continue;
                }
                if !phys_valid(phys_addr as usize) || phys_addr < 0x80000000 {
                    continue;
                }
                let pfn = phys_to_pfn(phys_addr as usize);
                let page = pfn_to_page(pfn);
                if !page.is_null() {
                    if (*page).is_mapped() {
                        crate::mm::rmap::page_remove_rmap(&*page);
                    }
                    let new_ref = (*page).put_page();
                    if new_ref == 0 {
                        crate::dfx::memwatch::FUT_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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

                // Swap entry (V=0 leaf with the swap signature): the page
                // lives on the swap device — free the slot. Without this,
                // every swapped-out page of a dying task leaks its slot
                // for the lifetime of the swap area.
                if !pte0.is_valid() && crate::mm::swap::is_swap_entry(pte0.bits()) {
                    crate::mm::swap::swap_free_slot(
                        crate::mm::swap::swap_entry_type(pte0.bits()),
                        crate::mm::swap::swap_entry_offset(pte0.bits()),
                    );
                    continue;
                }

                if !pte0.is_valid() || !pte0.is_leaf() {
                    continue;
                }

                // Skip kernel pages
                if !pte0.is_user() {
                    continue;
                }
                let phys_addr = pte0.ppn() << PAGE_SHIFT;
                if is_framebuffer_frame(phys_addr) {
                    continue;
                }
                let pfn = phys_to_pfn(phys_addr as usize);
                let page = pfn_to_page(pfn);

                if page.is_null() || !phys_valid(phys_addr as usize) || phys_addr < 0x80000000 {
                    continue;
                }

                if (*page).is_mapped() {
                    crate::mm::rmap::page_remove_rmap(&*page);
                }

                let new_ref = (*page).put_page();
                if new_ref == 0 {
                    crate::dfx::memwatch::FUT_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    free_pages(phys_addr as usize, 0);
                }
            }
            free_page_table_checked(table0_phys, "l0");
        }
        free_page_table_checked(table1_phys, "l1");
    }

    // Free root table (L2)
    free_page_table_checked(root_phys, "root");
}

// ==================== Page Mapping Functions ====================

/// Map a single page in page table WITHOUT flushing the TLB.
///
/// Internal batch primitive (review PERF: map_page used to issue a full
/// `sfence.vma` per 4K page, so boot-time region loops paid ~1000 global
/// TLB flushes). Region/batch callers fence ONCE after their loop; single
/// page callers use the public `map_page`, which keeps its fence so a
/// freshly faulted-in page is immediately usable.
///
/// # Arguments
/// - root_ppn: Root page table physical page number
/// - virt: Virtual address
/// - phys: Physical address
/// - flags: Page table entry flags
unsafe fn map_page_noflush(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    // Extract virtual page numbers (VPN2, VPN1, VPN0)
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // DFX memwatch: catch writes into the KERNEL root's low half. Every
    // non-leaf entry that lands at vpn2[0..1] of the kernel root is cloned
    // into EVERY future address space by copy_kernel_mappings (the
    // exec-leak amplifier). One-shot caller chain, raw UART.
    if vpn2 < 2 && root_ppn == root_page_table_ppn() && crate::dfx::memwatch::armed() {
        static POLLUTED: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if POLLUTED.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 6 {
            let mut frames: [u64; 8] = [0; 8];
            let s0: u64;
            unsafe {
                core::arch::asm!("mv {s}, s0", s = out(reg) s0, options(nomem, nostack));
                crate::dfx::memwatch::walk_fp_chain(s0, &mut frames);
            }
            crate::dfx::taskdump::taskdump_raw_line(b"KROOT-POLLUTE va=0x");
            let mut sh: i32 = 64;
            while sh > 0 {
                sh -= 4;
                let nb = ((virt_addr >> sh) & 0xF) as u8;
                crate::dfx::taskdump::taskdump_raw_line(&[(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 })]);
            }
            for f in frames.iter() {
                if *f == 0 { break; }
                crate::dfx::taskdump::taskdump_raw_line(b" ");
                let mut sh2: i32 = 64;
                crate::dfx::taskdump::taskdump_raw_line(b"0x");
                while sh2 > 0 {
                    sh2 -= 4;
                    let nb = ((*f >> sh2) & 0xF) as u8;
                    crate::dfx::taskdump::taskdump_raw_line(&[(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 })]);
                }
            }
            crate::dfx::taskdump::taskdump_raw_line(b"\n");
        }
    }

    // Get root page table (L2)
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
    // A 2MB megapage leaf here cannot host a 4K mapping — treating its
    // PPN as an L0 table pointer would corrupt whatever frame it names.
    // No caller maps 4K inside the megapaged device windows today; refuse
    // loudly to keep that invariant explicit.
    if pte1.is_valid() && pte1.is_leaf() {
        crate::pr_err!(
            "map_page: 4K map at {:#x} collides with 2MB megapage — refused",
            virt_addr
        );
        return;
    }
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
    pte_install_log(root_ppn, virt_addr, ppn);
}

/// Map a single page in page table
///
/// # Arguments
/// - root_ppn: Root page table physical page number
/// - virt: Virtual address
/// - phys: Physical address
/// - flags: Page table entry flags
pub unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    map_page_noflush(root_ppn, virt, phys, flags);

    // Flush TLB (single-page callers — page faults, mremap, io_uring —
    // need the new translation visible before the retrying access).
    asm!("sfence.vma");
}

/// Map a region with identity mapping
pub(crate) unsafe fn map_region(root_ppn: u64, start: u64, size: u64, flags: u64) {
    let virt_start = VirtAddr::new(start);
    let phys_start = PhysAddr::new(start);
    let virt_end = VirtAddr::new(start + size);

    let mut virt = virt_start.floor();
    let end = virt_end.ceil();

    while virt.bits() < end.bits() {
        let offset = virt.bits() - virt_start.bits();
        let phys = PhysAddr::new(phys_start.bits() + offset);
        map_page_noflush(root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
    // One fence for the whole region (batch aggregation).
    asm!("sfence.vma zero, zero", options(nomem, nostack));
}

/// Map a 2MB huge page using PMD leaf entry
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
        asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Create PMD leaf entry (2MB huge page)
    // For 2MB huge page at L1 level:
    // - PPN[2] (bits 53:28 of PTE) = PA[55:30]
    // - PPN[1] (bits 27:19 of PTE) = PA[29:21]
    // - PPN[0] (bits 18:10 of PTE) = 0 (must be zero for 2MB alignment)
    //
    // PTE format: [PPN[2]][PPN[1]][PPN[0]][RSW][D][A][G][U][X][W][R][V]
    //             [53:28] [27:19] [18:10] [9:8][7][6][5][4][3][2][1][0]
    //
    // For phys = 0x80200000:
    // - PA[55:30] = 0x200
    // - PA[29:21] = 0x1
    // - PTE = (0x200 << 28) | (0x1 << 19) | flags = 0x20080000 | flags
    //
    // Generic formula: PTE = ((phys >> 30) << 28) | ((phys >> 21) & 0x1FF) << 19) | flags
    //                = (phys >> 2) | flags  (simplified when phys is 2MB aligned)

    assert!(phys % (2 * 1024 * 1024) == 0, "phys must be 2MB aligned for huge page");

    let ppn2 = (phys >> 30) as u64;  // PA[55:30]
    let ppn1_val = ((phys >> 21) & 0x1FF) as u64;  // PA[29:21]
    let entry_bits = (ppn2 << 28) | (ppn1_val << 19) | flags;

    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    (*table1).set(vpn1, PageTableEntry::from_bits(entry_bits));
}

/// Map a kernel virtual page to a physical page WITHOUT flushing the TLB.
///
/// Internal batch primitive — see map_page_noflush. Region callers fence
/// once after their loop via map_kernel_region.
unsafe fn map_kernel_page_noflush(virt: u64, phys: u64, flags: u64) {
    let vpn2 = ((virt >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt >> 12) & 0x1FF) as usize;

    // Get root page table from current satp
    let satp: u64;
    asm!("csrr {}, satp", out(reg) satp);
    let root_ppn = satp & 0xFFFFFFFFFFFFF;
    let root_phys = root_ppn << PAGE_SHIFT;
    let root = get_page_table_virt(root_phys) as *mut PageTable;
    let root = &mut *root;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_kernel_page: failed to allocate L1 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        asm!("sfence.vma zero, zero", options(nomem, nostack));
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
        asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Level 0 -> Physical page
    let table0_phys = ppn0 << PAGE_SHIFT;
    let table0 = get_page_table_virt(table0_phys);
    let table0_ref = &mut *table0;
    let ppn: u64 = phys >> PAGE_SHIFT;
    let pte_bits: u64 = (ppn << 10) | flags;

    table0_ref.set(vpn0, PageTableEntry::from_bits(pte_bits));
}

/// Map a kernel virtual page to a physical page
///
/// Used for vmemmap and other kernel mappings that need 4KB page granularity.
pub unsafe fn map_kernel_page(virt: u64, phys: u64, flags: u64) {
    map_kernel_page_noflush(virt, phys, flags);
    asm!("sfence.vma zero, zero", options(nomem, nostack));
}

/// Map a region of kernel virtual pages to physical pages (identity mapped MMIO)
///
/// Uses current satp's page table. Maps each 4KB page individually, then
/// flushes the TLB ONCE for the whole region (per-page flushes in the loop
/// cost hundreds of global sfence.vma at boot — review PERF).
///
/// PERF (superpage MMIO): identity windows that are 2MB-aligned and at
/// least one PMD apart are mapped as L1 2MB leaf PTEs instead of 512
/// individual 4KB PTEs. The QEMU virt platform's PCI MMIO hole is 256MB
/// and ECAM 8MB: as 4KB pages that is 68K PTEs + 139 L0 tables in EVERY
/// address space (copy_kernel_mappings duplicates them per process,
/// copy_page_table_cow walks/copies them per fork, free_user_page_tables
/// walks them per exit — measured ~85ms per fork+exit under TCG). With
/// L1 leaves the same windows cost ~132 PTEs and zero L0 tables.
/// Unaligned head/tail pages still map at 4KB granularity.
pub unsafe fn map_kernel_region(virt: u64, phys: u64, size: u64, flags: u64) {
    use crate::arch::riscv64::mm::memory_layout::PMD_SIZE;

    let mut v = virt;
    let mut p = phys;
    let end = virt + size;
    while v < end {
        let remain = end - v;
        if v % PMD_SIZE == 0 && p % PMD_SIZE == 0 && remain >= PMD_SIZE {
            // 2MB superpage: install one L1 leaf PTE (identity v==p here).
            // PPN field stays in 4KB units ((p>>12)<<10); the 2MB-aligned
            // p makes PPN[0] zero as Sv39 requires for a PMD leaf, and the
            // RWX bits in `flags` make the entry a leaf.
            let pte_bits = ((p >> PAGE_SHIFT) << 10) | flags;
            map_pmd_huge_page(v as usize, p as usize, flags);
            v += PMD_SIZE;
            p += PMD_SIZE;
        } else {
            map_kernel_page_noflush(v, p, flags);
            v += PAGE_SIZE;
            p += PAGE_SIZE;
        }
    }
    asm!("sfence.vma zero, zero", options(nomem, nostack));
}

/// Map a kernel device region using 2MB megapage leaf entries where the
/// range permits (interior 2MB-aligned span), falling back to 4K pages for
/// the head/tail. Device windows mapped this way cost ONE L1 leaf entry
/// per 2MB instead of an L0 table per 2MB — copy_kernel_mappings clones
/// leaf entries in O(1), so every exec/fork stops allocating one L0 table
/// per 2MB of device window (the 256MB PCI MMIO window alone was 128
/// tables per exec).
///
/// Must only be used for device memory that is mapped once at boot and
/// never re-mapped at 4K granularity later (map_page cannot descend below
/// an L1 leaf).
pub unsafe fn map_kernel_region_huge(virt: u64, size: u64, flags: u64) {
    const PMD: u64 = 0x20_0000;
    let mut v = virt;
    let end = virt + size;
    // Head: 4K pages up to the next 2MB boundary
    while v < end && v % PMD != 0 {
        map_kernel_page_noflush(v, v, flags);
        v += PAGE_SIZE;
    }
    // Interior: 2MB megapage leaf entries
    while v + PMD <= end {
        map_pmd_huge_page(v as usize, v as usize, flags);
        v += PMD;
    }
    // Tail: 4K pages
    while v < end {
        map_kernel_page_noflush(v, v, flags);
        v += PAGE_SIZE;
    }
    asm!("sfence.vma zero, zero", options(nomem, nostack));
}

/// Map device memory page to user space
pub fn map_device_page(virt: usize, phys: usize, flags: u64) {
    let vpn2 = (virt >> 30) & 0x1FF;
    let ppn = (phys >> 12) as u64;
    let entry_bits = (ppn << 10) | flags;

    unsafe {
        ROOT_PAGE_TABLE.set(vpn2 as usize, PageTableEntry::from_bits(entry_bits));
    }

    unsafe {
        asm!("sfence.vma", options(nomem, nostack));
    }
}

/// Select best mapping size
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

// ==================== MMU Initialization ====================

/// Setup early page tables for MMU enable
///
/// This function is called from boot.S before relocate_enable_mmu.
#[no_mangle]
pub unsafe extern "C" fn setup_vm() {
    // Get the VA-PA offset by comparing linked address with runtime address
    let va_pa_offset: u64;
    asm!(
        "1:",
        "auipc {offset}, 0",
        "la {virt}, 1b",
        "sub {offset}, {virt}, {offset}",
        offset = out(reg) va_pa_offset,
        virt = out(reg) _,
        options(nostack),
    );

    extern "C" {
        static mut early_pg_dir: [u8; 4096];
    }

    let early_pg_dir_va = &raw mut early_pg_dir as *mut u8 as u64;
    let early_pg_dir_pa = early_pg_dir_va - va_pa_offset;
    let early_pg_dir_ptr = early_pg_dir_pa as *mut PageTable;
    let early_pg_dir_ref = &mut *early_pg_dir_ptr;

    early_pg_dir_ref.zero();

    let early_ppn = early_pg_dir_pa / PAGE_SIZE;

    let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                       PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;
    // Early boot: use basic flags (SVPBMT may not be available yet)
    let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                       PageTableEntry::A | PageTableEntry::D;

    // Map kernel with identity mapping
    let kernel_phys = KERNEL_ENTRY;
    let kernel_virt = KERNEL_ENTRY + VA_PA_OFFSET as u64;
    let kernel_size = KERNEL_SIZE;

    let mut phys = kernel_phys;
    let mut virt = kernel_phys;
    let end_phys = kernel_phys + kernel_size;

    // No per-page flushes here: this runs BEFORE the MMU is enabled —
    // boot.S's "sfence.vma; csrw satp" trampoline sequence publishes the
    // tables once (review PERF aggregation).
    while phys < end_phys {
        map_page_noflush(early_ppn, VirtAddr::new(virt), PhysAddr::new(phys), kernel_flags);
        phys += PAGE_SIZE;
        virt += PAGE_SIZE;
    }

    // Map kernel at virtual address
    phys = kernel_phys;
    virt = kernel_virt;
    while phys < end_phys {
        map_page_noflush(early_ppn, VirtAddr::new(virt), PhysAddr::new(phys), kernel_flags);
        phys += PAGE_SIZE;
        virt += PAGE_SIZE;
    }

    // Map UART (identity mapping)
    map_region(early_ppn, UART_BASE, 0x1000, device_flags);

    // Map DTB area (identity mapping) - use actual DTB pointer from OpenSBI
    let dtb_addr = crate::arch::riscv64::boot::get_dtb_pointer();
    if dtb_addr != 0 {
        // Align down to page boundary
        let dtb_page = dtb_addr & !0xFFF;
        map_region(early_ppn, dtb_page, 0x200000, device_flags);  // Map 2MB to cover DTB
    }
}

/// Initialize MMU
///
/// Called from rust_main(). MMU is already enabled by boot.S (trampoline).
/// This function creates the permanent kernel page table (ROOT_PAGE_TABLE)
/// and switches to it.
///
/// The permanent page table contains:
/// - Kernel mapping at KERNEL_LINK_ADDR (VPN2[510]) - for kernel code/data/BSS
/// - UART identity mapping (VPN2[0]) - for early boot UART access
/// - DTB mapping at linear mapping address
/// - Fixmap for UART
///
/// Later, setup_linear_mapping() adds the full linear mapping at PAGE_OFFSET.
pub fn init() {
    unsafe {
        // MMU is already enabled by boot.S trampoline
        // Stay in Early stage for initial page table setup (uses static BSS arrays)
        // Don't switch to Fixmap yet — phys_to_virt won't work until linear mapping is set up
        // pt_ops_set_fixmap() will be called after setup_linear_mapping()

        // Initialize root page table
        ROOT_PAGE_TABLE.zero();

        let root_ppn = root_page_table_ppn();

        let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;

        // Map kernel at KERNEL_LINK_ADDR (VPN2[510]) using 2MB huge pages
        // This avoids allocating many L0 page tables during early boot
        let kernel_virt = KERNEL_LINK_ADDR as u64;
        let kernel_phys = KERNEL_ENTRY;
        let kernel_size = KERNEL_SIZE;

        let mut phys = kernel_phys;
        let mut virt = kernel_virt;
        let end_phys = kernel_phys + kernel_size;

        while phys < end_phys {
            let remaining = end_phys - phys;
            if remaining >= PMD_SIZE as u64 && (phys & (PMD_SIZE as u64 - 1)) == 0 {
                map_pmd_huge_page(virt as usize, phys as usize, kernel_flags);
                phys += PMD_SIZE as u64;
                virt += PMD_SIZE as u64;
            } else {
                // No per-page flush — addr_space.enable() below fences the
                // whole table once after the switch (review PERF aggregation).
                map_page_noflush(root_ppn, VirtAddr::new(virt), PhysAddr::new(phys), kernel_flags);
                phys += PAGE_SIZE;
                virt += PAGE_SIZE;
            }
        }

        // Map UART at physical address (for early boot before fixmap is used by console)
        // Note: This is in VPN2[0] which is user-space range, but U=0 so only kernel can access.
        // This is temporary - will be removed once console uses fixmap exclusively.
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, UART_BASE, 0x1000, device_flags);

        // Map DTB at linear mapping address
        let dtb_addr = crate::arch::riscv64::boot::get_dtb_pointer();
        if dtb_addr != 0 {
            let dtb_page = dtb_addr & !0xFFF;
            let dtb_virt = phys_to_virt(PhysAddr::new(dtb_page));
            let dtb_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                           PageTableEntry::A | PageTableEntry::D;
            let mut phys = dtb_page;
            let end_phys = dtb_page + 0x200000;
            while phys < end_phys {
                map_pmd_huge_page(dtb_virt.bits() as usize + (phys - dtb_page) as usize, phys as usize, dtb_flags);
                phys += PMD_SIZE as u64;
            }
        }

        // Initialize UART fixmap
        super::fixmap::init_uart_fixmap();

        // Switch to permanent page table
        let addr_space = MmStruct::new_kernel(root_ppn);
        addr_space.enable();
    }
}

/// Setup device mappings (called after fixmap stage is ready)
#[allow(dead_code)]
pub fn setup_device_mappings() {
    unsafe {
        // SVPBMT IO memory type (PTE bits 62:61) requires the CPU to
        // implement the svpbmt extension — QEMU's default `-cpu rv64`
        // ISA (rv64imafdch...) does NOT include svpbmt, and setting the
        // bits there makes the PTE reserved-invalid: every MMIO access
        // faults (observed boot panic at PLIC set_priority). Until ISA
        // probing gates this, map devices cacheable (QEMU's memory model
        // tolerates it); a real-machine port must probe svpbmt first.
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::A | PageTableEntry::D;

        // Map full MMIO ranges (not just single pages)
        // UART: 0x10000000, 1 page (already mapped by mm::init)
        // VirtIO: 0x10001000, 8 slots each at 0x1000 boundary
        map_kernel_region(VIRTIO_MMIO_BASE as u64, VIRTIO_MMIO_BASE as u64, 0x8000, device_flags);

        // PLIC: priority space (0x2000) + enable/threshold/claim per hart context
        // With 4 harts: context space at 0x200000, each 0x1000, total ~0x204000
        // megapage: the low-half device tables are cloned per exec by
        // copy_kernel_mappings — L1 leaves copy in O(1), L0 tables do not.
        map_kernel_region_huge(PLIC_BASE as u64, 0x210000, device_flags);

        // CLINT: 0x10000 bytes
        map_kernel_region(CLINT_BASE as u64, CLINT_BASE as u64, 0x10000, device_flags);

        // Goldfish RTC: 0x101000, 1 page (QEMU virt's default RTC device,
        // read once at boot for the wall-clock epoch offset — drivers/rtc.rs)
        map_kernel_region(
            crate::drivers::rtc::GOLDFISH_RTC_BASE,
            crate::drivers::rtc::GOLDFISH_RTC_BASE,
            crate::drivers::rtc::GOLDFISH_RTC_SIZE,
            device_flags,
        );

        // PCIe ECAM: buses 0-7 (8 x 1MB, function-per-4K windows). The
        // QEMU virt machine exposes a 256MB ECAM region here; bus 0 held
        // every boot device so 1MB used to be enough, but devices
        // hot-added behind a pcie-root-port (U4 rescan path) sit on
        // bus 1+. Mapped as megapages (see PLIC note).
        map_kernel_region_huge(PCIE_ECAM_BASE as u64, 0x800000, device_flags);

        // PCI MMIO: 0x10000000 bytes. Megapage: this single window was 128
        // L0 tables cloned into EVERY address space by
        // copy_kernel_mappings (~560KB of table alloc+zero per exec).
        // BARs are assigned by writing config space only — nothing maps
        // inside this window at 4K granularity later.
        map_kernel_region_huge(PCI_MMIO_BASE as u64, 0x10000000, device_flags);
    }
}

/// Setup linear mapping for physical memory
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    unsafe {
        // Initialize KERNEL_MAP.va_pa_offset for phys_to_virt/virt_to_phys
        // va_pa_offset = PAGE_OFFSET - phys_ram_base
        KERNEL_MAP.va_pa_offset = VA_PA_OFFSET;

        // Include X (execute) permission for kernel code in linear mapping
        let linear_flags = PageTableEntry::V | PageTableEntry::R |
                          PageTableEntry::W | PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;

        for region in memory_regions {
            let phys_start = region.base;
            let size = region.size;
            let phys_end = phys_start + size;

            let virt_start = phys_start + VA_PA_OFFSET;

            let mut phys = phys_start;
            let mut virt = virt_start;

            while phys < phys_end {
                let remaining = phys_end - phys;
                let map_size = best_map_size(phys, virt, remaining);

                if map_size == PMD_SIZE as usize {
                    map_pmd_huge_page(virt, phys, linear_flags);
                } else {
                    // No per-page flush: the whole linear mapping is fenced
                    // once after this loop (review PERF aggregation).
                    map_kernel_page_noflush(virt as u64, phys as u64, linear_flags);
                }

                phys += map_size;
                virt += map_size;
            }
        }

        asm!("sfence.vma zero, zero", options(nomem, nostack));
    }
}

/// Enable MMU (secondary function)
pub fn enable() {
    unsafe {
        let root_ppn = root_page_table_ppn();
        let addr_space = MmStruct::new_kernel(root_ppn);
        addr_space.enable();
    }
}

/// Map identity mapping
pub fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let vpn2 = virt.vpn(2) as usize;
    let ppn = phys.ppn();

    unsafe {
        ROOT_PAGE_TABLE.set(vpn2, PageTableEntry::from_bits((ppn << 10) | flags));
    }
}

/// Get kernel page table PPN (physical)
pub fn get_kernel_page_table_ppn() -> u64 {
    unsafe { root_page_table_ppn() }
}
