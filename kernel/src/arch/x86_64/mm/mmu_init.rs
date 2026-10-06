//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 MMU initialization and page mapping.
//!
//! Ported from the riscv64 backend (arch/riscv64/mm/mmu_init.rs) — the
//! logic is portable; only PTE encoding and walk depth differ (4 levels,
//! PML4→PUD→PMD→PT, 2MB/1GB leaves via the PS bit).
//!
//! Boot contract (boot.S):
//! - The bootstrap tables (identity 0..2GB via PML4[0], kernel image at
//!   KERNEL_LINK_BASE via PML4[511]) stay active while `init()` runs.
//! - `init()` builds the real ROOT_PAGE_TABLE (kernel image higher-half +
//!   identity low link + fixmap PUD pre-link) and switches CR3. The low
//!   boot stack stays valid through the switch because the new tables
//!   KEEP PML4[0] → bootstrap identity PUD (dropped later, with KPTI).
//!
//! Stage design (same as riscv64):
//! - Early: static BSS pool (works under both bootstrap and real tables)
//! - Fixmap: memblock allocation (linear map live)
//! - Late: buddy allocator + PtLedger accounting

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicU32, AtomicUsize, Ordering};

use super::memory_layout::*;
use super::pagetable::*;
use super::fixmap::FIXADDR_START;
use crate::mm::page_desc::MAX_PAGES;
use crate::println;

// ==================== Linker / boot.S symbols ====================

// VMA end of the kernel image (linker script). LMA = VMA - KERNEL_LINK_BASE.
extern "C" {
    static __kernel_end: u8;
    /// Bootstrap identity PUD (boot_pdpt_lo, .boot.pt section — VMA == LMA).
    static boot_pdpt_lo: [u8; 4096];
}

/// Physical end of the mapped kernel image window.
///
/// `__kernel_end` (end of .bss) is NOT the true image end: the supplementary
/// linker fragment (linker-x86-extra.ld) places `.got` AFTER .bss. Round the
/// window up so everything past `__kernel_end` (GOT, any late fragments)
/// stays mapped after the CR3 switch.
pub fn kernel_image_phys_end() -> u64 {
    let end = raw_image_phys_end();
    (end + 0x1fffff) & !0x1fffff // round up to 2MB
}

fn raw_image_phys_end() -> u64 {
    // Linker symbol; address-of and arithmetic only.
    let vma_end = &raw const __kernel_end as usize as u64;
    vma_end - KERNEL_LINK_BASE as u64
}

// ==================== Root Page Table ====================

/// Kernel root page table (PML4) — the Rust-built one
pub static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

/// Physical address of the static ROOT_PAGE_TABLE (image BSS).
#[inline]
pub fn root_static_phys() -> u64 {
    // SAFETY: address arithmetic on a static; no dereference.
    unsafe {
        let vma = &raw mut ROOT_PAGE_TABLE as *mut PageTable as u64;
        vma - KERNEL_MAP.va_kernel_pa_offset as u64
    }
}

/// Physical page number of the static root.
#[inline]
pub fn root_static_ppn() -> u64 {
    root_static_phys() >> PAGE_SHIFT
}

/// Physical page number of the kernel root page table.
///
/// Semantics (pinned-function behavior, adjusted — documented in the port
/// report): returns the STATIC kernel root, like the riscv64 twin. The
/// stub read CR3, which is a USER root whenever another task is current —
/// generic callers (process/exit.rs, init.rs) use this as "the kernel
/// root" identity, and kernel-VA walks resolve identically through the
/// shared kernel-half links either way. Before init() installs the real
/// tables this names the root being built.
pub unsafe fn root_page_table_ppn() -> u64 {
    root_static_ppn()
}

static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ==================== Page Table Allocation ====================
//
// Three-stage page table allocation:
// 1. Early: static BSS pool (bootstrap or fresh tables both active — the
//    pool lives inside the kernel image, reachable at its higher-half VMA
//    in either CR3)
// 2. Fixmap: memblock allocation (MMU on real tables, linear map live,
//    buddy not ready)
// 3. Late: buddy allocator (full memory management available)
//
// NOTE: the bring-up plan sketched a memblock-backed bump region above
// __kernel_end for the Early stage. memblock is not even initialized when
// arch::mm::init() runs, and a free-floating bump region could collide
// with whatever heap/slab placement main.rs settles on for x86; a static
// BSS pool is part of the image (reserved with it) and needs no
// coordination. That is why the riscv64 static-pool design is kept.

/// Early page-table pool: 96 tables (384KB of BSS). The linear map of 2GB
/// at 2MB granularity needs ~6 PMD tables + PUDs; the kernel image and
/// fixmap a handful more; 96 leaves generous headroom.
const NUM_EARLY_TABLES: usize = 96;

#[link_section = ".bss"]
static mut EARLY_TABLES: [PageTable; NUM_EARLY_TABLES] =
    [const { PageTable::new() }; NUM_EARLY_TABLES];

/// Counter for early page table allocation
static EARLY_TABLE_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Allocation stage tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocStage {
    /// Early boot: static BSS pool (identity/higher-half reachable)
    Early,
    /// Fixmap stage: real tables active, use memblock allocation
    Fixmap,
    /// Late stage: buddy allocator ready, use normal page allocation
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

/// FORENSIC: true once the buddy allocator is the table-frame source —
/// cheap check for the raw-free watchpoint in Zone::free_pages.
pub fn alloc_stage_is_late() -> bool {
    matches!(get_alloc_stage(), AllocStage::Late)
}

/// Transition to fixmap stage (real tables + linear map active)
pub fn pt_ops_set_fixmap() {
    ALLOC_STAGE.store(AllocStage::Fixmap as u8, Ordering::Release);
}

/// Transition to late stage (buddy allocator ready)
pub fn pt_ops_set_late() {
    ALLOC_STAGE.store(AllocStage::Late as u8, Ordering::Release);
}

/// Allocate a page table and return its physical address
///
/// Three-stage allocation:
/// - Early: static BSS pool inside the kernel image
/// - Fixmap: memblock allocation (linear mapped)
/// - Late: buddy allocator (linear mapped)
pub unsafe fn alloc_page_table() -> Option<u64> {
    let stage = get_alloc_stage();
    match stage {
        AllocStage::Early => {
            // Early boot: use the static BSS pool. phys = virt - va_kernel_pa_offset
            // (the image's constant VMA-LMA offset holds for the whole image).
            let offset = KERNEL_MAP.va_kernel_pa_offset as u64;
            let idx = EARLY_TABLE_NEXT.fetch_add(1, Ordering::AcqRel);
            if idx < NUM_EARLY_TABLES {
                let table_virt = &EARLY_TABLES[idx] as *const PageTable as u64;
                let table_phys = table_virt.wrapping_sub(offset);
                core::ptr::write_bytes(table_virt as *mut u8, 0, PAGE_SIZE as usize);
                return Some(table_phys);
            }
            panic!(
                "mm: out of early page tables (used {}, pool {})",
                idx, NUM_EARLY_TABLES
            );
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
            // site table from THIS frame so the chain names the mapping path
            // (map_page / create_user_address_space / ...).
            if crate::dfx::memwatch::armed() {
                let mut mw_frames: [u64; crate::dfx::memwatch::SITE_FRAMES] =
                    [0; crate::dfx::memwatch::SITE_FRAMES];
                let mw_bp: u64;
                unsafe {
                    core::arch::asm!("mov {}, rbp", out(reg) mw_bp, options(nomem, nostack));
                    crate::dfx::memwatch::walk_fp_chain(mw_bp, &mut mw_frames);
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
            // Early pool lives inside the kernel image: reach it at its
            // higher-half VMA (VMA - va_kernel_pa_offset == LMA). Works
            // under both the bootstrap tables and the real ROOT tables.
            let offset = KERNEL_MAP.va_kernel_pa_offset as u64;
            let virt_addr = phys_addr.wrapping_add(offset);
            virt_addr as *mut PageTable
        }
        AllocStage::Fixmap | AllocStage::Late => {
            // Linear mapping is available at these stages
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

static BOOT_TREE_STAMPED: AtomicBool = AtomicBool::new(false);

/// One-time: mark every page-table frame wired into the kernel's ROOT page
/// table as boot-permanent. Early boot builds tables by hand (not through
/// alloc_page_table), and user mms forked from the initial template can
/// inherit references to them; their teardown then freed those SHARED
/// frames. Called from the teardown path (PTE_MODIFY_LOCK held) before any
/// table bookkeeping. (INV note ported from riscv64: the stamp set is the
/// kernel ROOT tree's own tables ONLY — stamping every memblock-reserved
/// frame saturates the boot set and refuses every legitimate free.)
unsafe fn stamp_kernel_tree_boot() {
    use crate::mm::phys_valid;
    if BOOT_TREE_STAMPED.load(Ordering::Acquire) {
        return;
    }
    BOOT_TREE_STAMPED.store(true, Ordering::Release);
    // Boot-permanent tables are exactly the frames reachable from the
    // kernel ROOT tree below (plus the Fixmap-stage tables stamped at
    // their allocation, and the EARLY_TABLES static pool range-checked
    // in free_page_table).
    let root_phys = root_static_phys();
    for vpn4 in 0..512usize {
        let pte4 = ROOT_PAGE_TABLE.get(vpn4);
        if !pte4.is_valid() || pte4.is_leaf() {
            continue;
        }
        let pud_phys = pte4.ppn() << PAGE_SHIFT;
        if !phys_valid(pud_phys as usize) || pud_phys == root_phys {
            continue;
        }
        PT_LEDGER.stamp_boot(pte4.ppn());
        let pud = get_page_table_virt(pud_phys);
        for vpn3 in 0..512usize {
            let pte3 = (*pud).get(vpn3);
            if !pte3.is_valid() || pte3.is_leaf() {
                continue;
            }
            let pmd_phys = pte3.ppn() << PAGE_SHIFT;
            if !phys_valid(pmd_phys as usize) || pmd_phys == root_phys {
                continue;
            }
            PT_LEDGER.stamp_boot(pte3.ppn());
        }
    }
}

/// Free a page table (only valid for late stage allocations)
unsafe fn free_page_table(phys_addr: u64) {
    if get_alloc_stage() != AllocStage::Late {
        return;
    }

    // Check if it's from the early static pool (never freed, only reused
    // during Early boot — by Late stage the pool is dormant).
    let offset = KERNEL_MAP.va_kernel_pa_offset as u64;
    let pool_start = (&EARLY_TABLES as *const _ as u64).wrapping_sub(offset);
    let pool_end = pool_start + (NUM_EARLY_TABLES * PAGE_SIZE as usize) as u64;

    if phys_addr >= pool_start && phys_addr < pool_end {
        // Don't free early page tables
        return;
    }

    // Use zone allocator to free
    crate::mm::page_alloc::free_pages(phys_addr as usize, 0);
}

// FORENSIC: page-table frame ledger — every Late-stage alloc_page_table
// stamps the PPN; free_page_table clears it. A stamp on an already-stamped
// PPN = double allocation; clearing an unstamped or already-cleared PPN =
// double free / foreign free. Both are silent address-space corruptors
// under concurrent fork/exec.
pub struct PtLedger {
    // EXACT membership: one bit per PFN of the managed physical range
    // (MIN_PFN..+MAX_PAGES). Set by stamp(), cleared by take(). Zero
    // collisions by construction. (A hash bitmap saturates and then
    // refuses legitimate frees — every refusal permanently leaks one
    // table frame; see the riscv64 fake-OOM investigation.)
    live: [AtomicU64; MAX_PAGES / 64],
    // Boot-permanent tables (kernel root tree + Early static + Fixmap):
    // referenced by every early mm — freeing them per-mm tears down shared
    // state and feeds the frames back for reuse as live page tables (the
    // concurrent fork/exec corruption family). EXACT membership (array of
    // ppn+1, linear scan), NOT a hash: a false "boot" on a USER table made
    // fork share that subtree between parent and child (cross-process PTE
    // aliasing). The boot set is tiny (the kernel tree's own tables, ~dozens).
    boot: [AtomicU64; 512],
    reported: AtomicUsize,
}
impl PtLedger {
    const fn new() -> Self {
        Self {
            live: [const { AtomicU64::new(0) }; MAX_PAGES / 64],
            boot: [const { AtomicU64::new(0) }; 512],
            reported: AtomicUsize::new(0),
        }
    }
    /// Bit index for an absolute PPN; None when outside the managed range.
    #[inline]
    fn bit_index(ppn: u64) -> Option<usize> {
        let base = crate::mm::page_desc::MIN_PFN as u64;
        let i = ppn.checked_sub(base)? as usize;
        if i < MAX_PAGES {
            Some(i)
        } else {
            None
        }
    }
    #[inline]
    fn live_bit(&self, ppn: u64) -> bool {
        match Self::bit_index(ppn) {
            Some(i) => {
                let word = &self.live[i / 64];
                word.load(Ordering::Relaxed) & (1u64 << (i % 64)) != 0
            }
            None => false,
        }
    }
    #[inline]
    fn set_live_bit(&self, ppn: u64, value: bool) -> bool {
        // Returns the PREVIOUS bit (false for out-of-range PFNs — they are
        // never tracked, so a take() on one is refused as before).
        match Self::bit_index(ppn) {
            Some(i) => {
                let word = &self.live[i / 64];
                let mask = 1u64 << (i % 64);
                if value {
                    word.fetch_or(mask, Ordering::Relaxed) & mask != 0
                } else {
                    word.fetch_and(!mask, Ordering::Relaxed) & mask != 0
                }
            }
            None => false,
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
        if self.set_live_bit(ppn, true) {
            crate::pr_err!(
                "PTLEDGER: DOUBLE-ALLOC ppn={:#x} by pid={} (still live)",
                ppn, pid
            );
        }
        // FORENSIC (fake-OOM family): alloc-side ring — pairs with
        // PT_FREE_RING to replay a frame's full table-incarnation history.
        {
            let idx = PT_STAMP_CURSOR.fetch_add(1, Relaxed) % PT_STAMP_RING.len();
            PT_STAMP_RING[idx].ppn.store(ppn, Relaxed);
            PT_STAMP_RING[idx].pid.store(pid, Relaxed);
            PT_STAMP_RING[idx].site.store(b'S' as u64, Relaxed);
        }
    }
    /// Returns true if the frame had a live stamp (a legitimate first
    /// free) and was cleared; false for double/foreign frees — the caller
    /// must then REFUSE to return the frame to the zone. A table frame
    /// entering the free list twice is what wires one physical page into
    /// two live page-table trees (the shared-pgd corruption family).
    fn take(&self, ppn: u64, pid: u32, site: &str) -> bool {
        use core::sync::atomic::Ordering::Relaxed;
        if self.set_live_bit(ppn, false) {
            // FORENSIC: ring of successful table frees — at a stale-tree
            // event this names who freed each frame and from which
            // teardown site ('r'=root, '0'=PT, '1'=PMD, '2'=PUD).
            {
                let idx = PT_FREE_CURSOR.fetch_add(1, Relaxed) % PT_FREE_RING.len();
                PT_FREE_RING[idx].ppn.store(ppn, Relaxed);
                PT_FREE_RING[idx].pid.store(pid, Relaxed);
                PT_FREE_RING[idx].site.store(site.as_bytes()[0] as u64, Relaxed);
            }
            // EARLY-TEARDOWN WITNESS: a registered fork-mm root is being
            // freed. If the freeing context is NOT the registered owner and
            // the owner task is still alive, another mm shared this root.
            if site == "root" {
                for k in 0..FORK_ROOTS.len() {
                    let r = FORK_ROOTS[k].root_ppn.load(Relaxed);
                    if r == ppn && r != 0 {
                        let owner = FORK_ROOTS[k].owner_pid.load(Relaxed);
                        if owner != pid && task_pid_alive(owner) {
                            static EARLY_REPORTS: AtomicUsize = AtomicUsize::new(0);
                            if EARLY_REPORTS.fetch_add(1, Relaxed) < 8 {
                                crate::pr_err!(
                                    "EARLY-ROOT-FREE root={:#x} owner pid={} ALIVE, freed in pid={} context",
                                    ppn, owner, pid
                                );
                            }
                        }
                        break;
                    }
                }
            }
            return true;
        }
        if self.reported.fetch_add(1, Relaxed) < 8 {
            crate::pr_err!(
                "PTLEDGER: DOUBLE-FREE ppn={:#x} at {} by pid={} REFUSED (prev-free pid={}, boot={})",
                ppn, site, pid, pt_free_ring_prev_free_pid(ppn), self.is_boot(ppn)
            );
            // FORENSIC: the frame's live descriptor state distinguishes
            // "freed earlier, still on freelist" (OnFreelist, refcount 0)
            // from "reallocated as live data page" (refcount>0, Anonymous/
            // Lru) — both prove this tree references a frame it no longer
            // owns.
            {
                let page = crate::mm::page_desc::pfn_to_page(ppn as usize);
                if !page.is_null() {
                    // SAFETY: read-only descriptor access on the diagnostic path.
                    let (rc, linked, lru, anon) = unsafe {
                        use crate::mm::page_desc::PageFlag;
                        (
                            (*page).refcount(),
                            (*page).test_flag(PageFlag::OnFreelist),
                            (*page).test_flag(PageFlag::Lru),
                            (*page).test_flag(PageFlag::Anonymous),
                        )
                    };
                    crate::pr_err!(
                        "  PTLEDGER: frame state refcount={} on_freelist={} lru={} anon={}",
                        rc, linked, lru, anon
                    );
                }
            }
            let cur = FUT_RING_CURSOR.load(core::sync::atomic::Ordering::Relaxed);
            for k in 0..32 {
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

// FORENSIC: ring of successful page-table frees, recorded by
// PtLedger::take() — replayed when the stale-tree census fires.
pub struct PtFreeEntry {
    pub ppn: AtomicU64,
    pub pid: AtomicU32,
    pub site: AtomicU64,
}
impl PtFreeEntry {
    const fn new() -> Self {
        Self {
            ppn: AtomicU64::new(0),
            pid: AtomicU32::new(0),
            site: AtomicU64::new(0),
        }
    }
}
const PT_FREE_NEW: PtFreeEntry = PtFreeEntry::new();
pub static PT_FREE_RING: [PtFreeEntry; 1024] = [PT_FREE_NEW; 1024];
pub static PT_FREE_CURSOR: AtomicUsize = AtomicUsize::new(0);
pub static PT_STAMP_RING: [PtFreeEntry; 1024] = [PT_FREE_NEW; 1024];
pub static PT_STAMP_CURSOR: AtomicUsize = AtomicUsize::new(0);

// FORENSIC (fake-OOM family): fork-mm registry — every AddressSpace::fork
// records its fresh root. When a registered root's ledger stamp is taken,
// the taker is compared against the registered owner: a free by any other
// context while the owner task is still alive is the EARLY-TEARDOWN
// witness (the seed of the stale-tree corruption family).
pub struct ForkRootReg {
    pub root_ppn: AtomicU64,
    pub owner_pid: AtomicU32,
}
impl ForkRootReg {
    const fn new() -> Self {
        Self {
            root_ppn: AtomicU64::new(0),
            owner_pid: AtomicU32::new(0),
        }
    }
}
const FORK_ROOT_NEW: ForkRootReg = ForkRootReg::new();
pub static FORK_ROOTS: [ForkRootReg; 256] = [FORK_ROOT_NEW; 256];
pub static FORK_ROOT_CURSOR: AtomicUsize = AtomicUsize::new(0);

pub fn register_fork_root(root_ppn: u64, owner_pid: u32) {
    let idx = FORK_ROOT_CURSOR.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
        % FORK_ROOTS.len();
    FORK_ROOTS[idx].root_ppn.store(root_ppn, core::sync::atomic::Ordering::Relaxed);
    FORK_ROOTS[idx].owner_pid.store(owner_pid, core::sync::atomic::Ordering::Relaxed);
}

fn task_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let alive = AtomicBool::new(false);
    crate::sched::for_each_task(|t| unsafe {
        if (*t).pid() as u32 == pid {
            alive.store(true, core::sync::atomic::Ordering::Relaxed);
        }
    });
    alive.load(core::sync::atomic::Ordering::Relaxed)
}

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

    /// FORENSIC: live-stamp probe for the stale-tree census. Returns 1 when
    /// the frame carries a live table stamp, 0 when not (exact bitmap — no
    /// hash-collision ambiguity).
    pub fn peek(&self, ppn: u64) -> u64 {
        self.live_bit(ppn) as u64
    }

    /// FORENSIC: pid that last successfully freed this ppn's stamp
    /// (most recent PT_FREE_RING entry naming the ppn; 0 = unknown).
    pub fn freed_by_peek(&self, ppn: u64) -> u32 {
        pt_free_ring_prev_free_pid(ppn)
    }

    /// FORENSIC: does this frame carry a LIVE table stamp? Used by the
    /// raw-free watchpoint in Zone::free_pages — a raw (non-page-table-
    /// path) free of a live-stamped frame is the seed event that turns a
    /// live page-table tree stale.
    pub fn is_live_table(ppn: u64) -> bool {
        PT_LEDGER.live_bit(ppn)
    }
}

/// FORENSIC: newest PT_FREE_RING entry naming `ppn` — the pid context of its
/// most recent legitimate free (0 when the frame never freed within the
/// ring window). Backwards scan from the cursor; the freshest match wins.
fn pt_free_ring_prev_free_pid(ppn: u64) -> u32 {
    use core::sync::atomic::Ordering::Relaxed;
    let cur = PT_FREE_CURSOR.load(Relaxed);
    for k in 0..PT_FREE_RING.len() {
        let idx = (cur + PT_FREE_RING.len() - 1 - k) % PT_FREE_RING.len();
        if PT_FREE_RING[idx].ppn.load(Relaxed) == ppn {
            return PT_FREE_RING[idx].pid.load(Relaxed) as u32;
        }
    }
    0
}

pub struct FutEntry {
    pub root: AtomicU64,
    pub pid: AtomicU64,
}
impl FutEntry {
    const fn new() -> Self {
        Self {
            root: AtomicU64::new(0),
            pid: AtomicU64::new(0),
        }
    }
}
const FUT_NEW: FutEntry = FutEntry::new();
pub static FUT_RING: [FutEntry; 1024] = [FUT_NEW; 1024];
pub static FUT_RING_CURSOR: AtomicUsize = AtomicUsize::new(0);
/// Global cap on "REPEAT teardown" forensic reports — the first handful
/// carry the forensic value; the rest are duplicates flooding the serial
/// console under fork-churn tests.
pub static FUT_REPEAT_REPORTS: AtomicUsize = AtomicUsize::new(0);

// FORENSIC: PTE-install ledger — every user PTE installation records
// (root, va, ppn). At a crash we replay which roots EVER mapped the
// victim's frame: an alias installed without a matching allocation.
pub struct PteInstall {
    pub root: AtomicU64,
    pub va: AtomicU64,
    pub ppn: AtomicU64,
}
impl PteInstall {
    const fn new() -> Self {
        Self {
            root: AtomicU64::new(0),
            va: AtomicU64::new(0),
            ppn: AtomicU64::new(0),
        }
    }
}
const PTEI_NEW: PteInstall = PteInstall::new();
pub static PTEI_RING: [PteInstall; 65536] = [PTEI_NEW; 65536];
pub static PTEI_CUR: AtomicUsize = AtomicUsize::new(0);
#[inline]
pub fn pte_install_log(root_ppn: u64, va: u64, ppn: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    // Hot path (every user PTE install — a fork copies hundreds of them):
    // the ring writes are forensic-only and cost 4 emulated atomics per
    // copied PTE under TCG. Behind the dfx=mmforensics runtime switch.
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::MmForensics) {
        return;
    }
    let i = PTEI_CUR.fetch_add(1, Relaxed) % PTEI_RING.len();
    PTEI_RING[i].root.store(root_ppn, Relaxed);
    PTEI_RING[i].va.store(va, Relaxed);
    PTEI_RING[i].ppn.store(ppn, Relaxed);
}

/// Free all page tables and user data pages used by a user address space.
///
/// Only frees USER space page tables (PML4 entries 0..256). Kernel
/// mappings (PML4[256..512] links) are shared and must NOT be freed.
///
/// For non-leaf entries the U bit is not meaningful — we must walk all
/// valid user-space upper entries, not skip them based on U bit.
pub unsafe fn free_user_page_tables(root_ppn: u64) {
    use crate::mm::{pfn_to_page, phys_to_pfn, phys_valid, free_pages};
    crate::dfx::memwatch::FUT_CALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Double-teardown detector: the same root walked twice means two
    // MmStructs ended up owning one page-table tree — the second walk
    // chases recycled frames full of foreign data. Behind the
    // dfx=mmforensics runtime switch (see pte_install_log).
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::MmForensics) {
        use core::sync::atomic::Ordering::Relaxed;
        for k in 0..FUT_RING.len() {
            if FUT_RING[k].root.load(Relaxed) == root_ppn && root_ppn != 0 {
                if FUT_REPEAT_REPORTS.fetch_add(1, Relaxed) < 8 {
                    crate::pr_err!(
                        "FUT: REPEAT teardown of root ppn={:#x} (ring[{}], prev pid={}, now pid={})",
                        root_ppn, k,
                        FUT_RING[k].pid.load(Relaxed),
                        crate::sched::get_current_pid()
                    );
                }
                // Consume the matched entry even past the report cap —
                // leaving it makes every later teardown of a recycled
                // root ppn re-match the same stale slot.
                FUT_RING[k].root.store(0, Relaxed);
                break;
            }
        }
        let idx = FUT_RING_CURSOR.fetch_add(1, Relaxed) % FUT_RING.len();
        FUT_RING[idx].root.store(root_ppn, Relaxed);
        FUT_RING[idx].pid.store(crate::sched::get_current_pid() as u64, Relaxed);
    }
    // FORENSIC: stale-tree census. Every intermediate table frame this walk
    // reaches must carry a LIVE ledger stamp (its allocation incarnation).
    // A missing stamp means the frame was freed earlier and this tree has
    // been pointing at recycled memory — the walk is about to put_page/
    // free whatever now lives there. Collected here (with indices),
    // printed once per offending teardown.
    let mut stale_tables: [(u8, usize, usize, u64); 16] = [(0, 0, 0, 0); 16];
    let mut stale_count = 0usize;
    // Serialize against concurrent fork copies / COW faults on ANY mm: the
    // pages freed here can be immediately reallocated as page tables or
    // COW copies by another CPU (PTE_MODIFY_LOCK, NEW2 class).
    let _pte_guard = super::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();

    let root_phys = root_ppn << PAGE_SHIFT;
    let root_table = get_page_table_virt(root_phys);

    // Walk and free all levels of the user half (PML4[0..256])
    for vpn4 in 0..USER_PTRS_PER_PGD {
        let pte4 = (*root_table).get(vpn4);
        if !pte4.is_valid() {
            continue;
        }

        // PML4 entries are never leaves on x86_64 (PS at PML4 is reserved).

        let ppn3 = pte4.ppn();
        let table3_phys = ppn3 << PAGE_SHIFT;

        if !phys_valid(table3_phys as usize) {
            continue;
        }

        if table3_phys == root_phys {
            continue;
        }

        // Shared kernel subtree (identity/device windows cloned into every
        // mm at creation can share boot tables): nothing under it belongs
        // to THIS mm — skip the descent entirely.
        if get_alloc_stage() == AllocStage::Late && PT_LEDGER.is_boot(ppn3) {
            continue;
        }

        let table3 = get_page_table_virt(table3_phys);

        for vpn3 in 0..512 {
            let pte3 = (*table3).get(vpn3);
            if !pte3.is_valid() {
                continue;
            }

            // 1GB leaf (PUD leaf with PS): user 1GB leaves are never
            // created by this port; kernel ones (U=0) are skipped.
            if pte3.is_leaf() {
                if !pte3.is_user() {
                    continue;
                }
                let phys_addr = pte3.ppn() << PAGE_SHIFT;
                if !phys_valid(phys_addr as usize) {
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

            let ppn2 = pte3.ppn();
            let table2_phys = ppn2 << PAGE_SHIFT;

            if !phys_valid(table2_phys as usize) {
                continue;
            }

            let table2 = get_page_table_virt(table2_phys);

            for vpn2 in 0..512 {
                let pte2 = (*table2).get(vpn2);
                if !pte2.is_valid() {
                    continue;
                }

                // 2MB leaf (PMD leaf with PS)
                if pte2.is_leaf() {
                    // Skip kernel pages (device windows cloned per-mm,
                    // U=0 — their frames are not this mm's to free)
                    if !pte2.is_user() {
                        continue;
                    }
                    let phys_addr = pte2.ppn() << PAGE_SHIFT;
                    if !phys_valid(phys_addr as usize) {
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

                let table1 = get_page_table_virt(table1_phys);

                for vpn1 in 0..512 {
                    let pte1 = (*table1).get(vpn1);

                    // Swap entry (P=0 leaf with the swap signature): the
                    // page lives on the swap device — free the slot.
                    if !pte1.is_valid() && crate::mm::swap::is_swap_entry(pte1.bits()) {
                        crate::mm::swap::swap_free_slot(
                            crate::mm::swap::swap_entry_type(pte1.bits()),
                            crate::mm::swap::swap_entry_offset(pte1.bits()),
                        );
                        continue;
                    }

                    if !pte1.is_valid() {
                        continue;
                    }

                    // Skip kernel pages
                    if !pte1.is_user() {
                        continue;
                    }
                    let phys_addr = pte1.ppn() << PAGE_SHIFT;
                    let pfn = phys_to_pfn(phys_addr as usize);
                    let page = pfn_to_page(pfn);

                    if page.is_null() || !phys_valid(phys_addr as usize) {
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
                // FORENSIC: census the PT table before freeing it.
                if get_alloc_stage() == AllocStage::Late && !PT_LEDGER.is_boot(ppn1) {
                    let live = PT_LEDGER.peek(ppn1) != 0;
                    if !live {
                        if stale_count < stale_tables.len() {
                            stale_tables[stale_count] = (0, vpn4, vpn3 * 512 + vpn2, ppn1);
                        }
                        stale_count += 1;
                    }
                }
                free_page_table_checked(table1_phys, "l0");
            }
            // FORENSIC: census the PMD table before freeing it.
            if get_alloc_stage() == AllocStage::Late && !PT_LEDGER.is_boot(ppn2) {
                let live = PT_LEDGER.peek(ppn2) != 0;
                if !live {
                    if stale_count < stale_tables.len() {
                        stale_tables[stale_count] = (1, vpn4, vpn3, ppn2);
                    }
                    stale_count += 1;
                }
            }
            free_page_table_checked(table2_phys, "l1");
        }
        // FORENSIC: census the PUD table before freeing it.
        if get_alloc_stage() == AllocStage::Late && !PT_LEDGER.is_boot(ppn3) {
            let live = PT_LEDGER.peek(ppn3) != 0;
            if !live {
                if stale_count < stale_tables.len() {
                    stale_tables[stale_count] = (2, vpn4, 0xFFFF, ppn3);
                }
                stale_count += 1;
            }
        }
        free_page_table_checked(table3_phys, "l2");
    }

    if stale_count > 0 {
        static STALE_REPORTS: AtomicUsize = AtomicUsize::new(0);
        if STALE_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 6 {
            crate::pr_err!(
                "FUT-STALE-TREE: root ppn={:#x} pid={} walks {} foreign/unstamped table frames:",
                root_ppn, crate::sched::get_current_pid(), stale_count
            );
            for i in 0..stale_count.min(stale_tables.len()) {
                let (lvl, v4, v3, ppn) = stale_tables[i];
                crate::pr_err!(
                    "  FUT-STALE: lvl={} pml4={} lower={} ppn={:#x}",
                    lvl, v4, v3, ppn
                );
            }
            // Root-slot forensics: who freed THIS root's ledger stamp last?
            crate::pr_err!(
                "  FUT-STALE: root slot peek={:#x} last-freed-by pid={}",
                PT_LEDGER.peek(root_ppn),
                PT_LEDGER.freed_by_peek(root_ppn)
            );
            // Replay of the stamp/free rings for the first few stale ppns.
            use core::sync::atomic::Ordering::Relaxed;
            let mut probes = [0u64; 4];
            let mut nprobes = 0usize;
            for i in 0..stale_count.min(stale_tables.len()) {
                if nprobes == probes.len() { break; }
                probes[nprobes] = stale_tables[i].3;
                nprobes += 1;
            }
            let scur = PT_STAMP_CURSOR.load(Relaxed);
            for k in 0..PT_STAMP_RING.len() {
                let idx = (scur + PT_STAMP_RING.len() - 1 - k) % PT_STAMP_RING.len();
                let ppn = PT_STAMP_RING[idx].ppn.load(Relaxed);
                if ppn == 0 {
                    continue;
                }
                if probes.iter().any(|&p| p == ppn) || ppn == root_ppn {
                    crate::pr_err!(
                        "  PT-HIST stamp[{}]: ppn={:#x} by pid={} (age {})",
                        idx, ppn, PT_STAMP_RING[idx].pid.load(Relaxed), k
                    );
                }
            }
            let fcur = PT_FREE_CURSOR.load(Relaxed);
            for k in 0..PT_FREE_RING.len() {
                let idx = (fcur + PT_FREE_RING.len() - 1 - k) % PT_FREE_RING.len();
                let ppn = PT_FREE_RING[idx].ppn.load(Relaxed);
                if ppn == 0 {
                    continue;
                }
                if probes.iter().any(|&p| p == ppn) || ppn == root_ppn {
                    crate::pr_err!(
                        "  PT-HIST free[{}]: ppn={:#x} by pid={} site={} (age {})",
                        idx, ppn,
                        PT_FREE_RING[idx].pid.load(Relaxed),
                        PT_FREE_RING[idx].site.load(Relaxed) as u8 as char,
                        k
                    );
                }
            }
            // Recent teardown history — entries naming THIS root are the
            // early teardown(s).
            let cur = FUT_RING_CURSOR.load(core::sync::atomic::Ordering::Relaxed);
            for k in 0..FUT_RING.len() {
                let idx = (cur + FUT_RING.len() - 1 - k) % FUT_RING.len();
                let r = FUT_RING[idx].root.load(core::sync::atomic::Ordering::Relaxed);
                let p = FUT_RING[idx].pid.load(core::sync::atomic::Ordering::Relaxed);
                if r == root_ppn {
                    crate::pr_err!(
                        "  FUT-HIST: root={:#x} torn down in pid={} context (age {})",
                        r, p, k
                    );
                }
            }
        }
    }

    // Free root table (PML4)
    free_page_table_checked(root_phys, "root");
}

// ==================== Page Mapping Functions ====================

/// Map a single 4K page in a page table WITHOUT flushing the TLB.
///
/// Internal batch primitive — region/batch callers flush ONCE after their
/// loop; single-page callers use the public `map_page`, which keeps its
/// flush so a freshly faulted-in page is immediately usable.
///
/// 4-level walk with intermediate-table allocation. A 2MB PMD leaf or 1GB
/// PUD leaf cannot host a 4K entry as-is — its PPN names a frame, not a
/// table. Linux semantics for a mapping over an existing translation: the
/// new mapping REPLACES it, so the leaf is DEMOTED — an intermediate table
/// is installed whose entries reproduce the leaf's translation verbatim
/// (same phys base, same flag bits) — then the caller's 4K map overwrites
/// exactly its own slot below (E8-MM, ported from riscv64; user roots
/// clone the kernel's low-half device windows, and a user fixed-address
/// mmap landing on one must replace, not silently drop, the window).
///
/// # Arguments
/// - root_ppn: Root page table physical page number
/// - virt: Virtual address
/// - phys: Physical address
/// - flags: Page table entry flag bits
unsafe fn map_page_noflush(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    let a = VirtAddr::new(virt_addr);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
    let root = &mut *root_table;

    // PML4 -> PUD (PML4 leaves do not exist on x86_64)
    let pte4 = root.get(vpn4);
    let ppn3 = if pte4.is_valid() {
        pte4.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_page: failed to allocate PUD table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn4, PageTableEntry::new_table(ppn));
        ppn
    };

    // PUD -> PMD
    let table3_phys = ppn3 << PAGE_SHIFT;
    let table3 = get_page_table_virt(table3_phys);
    let table3_ref = &mut *table3;
    let mut pte3 = table3_ref.get(vpn3);
    // 1GB leaf demotion: install a PMD table whose 512 entries reproduce
    // the 1GB mapping as 2MB leaves (same phys base, same flags + PS).
    if pte3.is_valid() && pte3.is_leaf() {
        if let Some(pmd_phys) = alloc_page_table() {
            let pmd = get_page_table_virt(pmd_phys);
            let base = pte3.phys_addr(); // 1GB-aligned
            let flag_bits = pte3.bits() & !PageTableEntry::PHYS_MASK_PUBLIC;
            for k in 0..512u64 {
                (*pmd).set(
                    k as usize,
                    PageTableEntry::from_bits((base + (k << PMD_SHIFT)) | flag_bits),
                );
            }
            // Single-word swap: a concurrent walker sees either the leaf or
            // a fully populated table — both translate the whole 1GB
            // identically. The caller's page is overwritten afterwards; the
            // flush in map_page / region batch publishes both writes.
            let pmd_ppn = pmd_phys >> PAGE_SHIFT;
            table3_ref.set(vpn3, PageTableEntry::new_table(pmd_ppn));
            pte3 = table3_ref.get(vpn3);
        } else {
            crate::pr_err!(
                "map_page: PMD table alloc failed demoting 1GB leaf at {:#x}",
                virt_addr
            );
            return;
        }
    }
    let ppn2 = if pte3.is_valid() {
        pte3.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_page: failed to allocate PMD table");
        let ppn = table_phys >> PAGE_SHIFT;
        table3_ref.set(vpn3, PageTableEntry::new_table(ppn));
        ppn
    };

    // PMD -> PT
    let table2_phys = ppn2 << PAGE_SHIFT;
    let table2 = get_page_table_virt(table2_phys);
    let table2_ref = &mut *table2;
    let mut pte2 = table2_ref.get(vpn2);
    // 2MB leaf demotion (E8-MM): install a PT whose 512 entries reproduce
    // the 2MB mapping verbatim (same phys base, same flag bits minus PS).
    if pte2.is_valid() && pte2.is_leaf() {
        if let Some(pt_phys) = alloc_page_table() {
            let pt = get_page_table_virt(pt_phys);
            let base = pte2.phys_addr(); // 2MB-aligned
            let flag_bits = (pte2.bits() & !PageTableEntry::PHYS_MASK_PUBLIC) & !PageTableEntry::PS;
            for k in 0..512u64 {
                (*pt).set(
                    k as usize,
                    PageTableEntry::from_bits((base + (k << PAGE_SHIFT)) | flag_bits),
                );
            }
            let pt_ppn = pt_phys >> PAGE_SHIFT;
            table2_ref.set(vpn2, PageTableEntry::new_table(pt_ppn));
            pte2 = table2_ref.get(vpn2);
        } else {
            crate::pr_err!(
                "map_page: PT table alloc failed demoting 2MB leaf at {:#x}",
                virt_addr
            );
            return;
        }
    }
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_page: failed to allocate PT table");
        let ppn = table_phys >> PAGE_SHIFT;
        table2_ref.set(vpn2, PageTableEntry::new_table(ppn));
        ppn
    };

    // PT -> physical page
    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    let table1_ref = &mut *table1;
    let ppn: u64 = phys_addr >> PAGE_SHIFT;
    let pte_bits: u64 = (ppn << PAGE_SHIFT) | flags;

    table1_ref.set(vpn1, PageTableEntry::from_bits(pte_bits));
    pte_install_log(root_ppn, virt_addr, ppn);
}

/// Map a single 4K page in a page table (with TLB flush of the page).
pub unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    map_page_noflush(root_ppn, virt, phys, flags);

    // Flush TLB (single-page callers — page faults, mremap, io_uring —
    // need the new translation visible before the retrying access).
    crate::arch::cpu::invlpg(virt.bits());
}

/// Install a 2MB PMD leaf mapping into the static kernel root.
///
/// Overwrites whatever PMD entry covers `virt` (a fresh link, an existing
/// leaf — e.g. replacing a cacheable bootstrap leaf with an IO-flagged
/// one). Must only be used for windows mapped once at boot and never
/// re-mapped at 4K granularity later... except map_page CAN demote these
/// leaves per-address-space after the clone (E8-MM).
unsafe fn map_pmd_huge_page(virt: usize, phys: usize, flags: u64) {
    let a = VirtAddr::new(virt as u64);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;

    let root = &mut ROOT_PAGE_TABLE;

    // PML4 -> PUD
    let pte4 = root.get(vpn4);
    let ppn3 = if pte4.is_valid() {
        pte4.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_pmd_huge_page: failed to allocate PUD table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn4, PageTableEntry::new_table(ppn));
        ppn
    };

    // PUD -> PMD
    let table3_phys = ppn3 << PAGE_SHIFT;
    let table3 = get_page_table_virt(table3_phys);
    let table3_ref = &mut *table3;
    let pte3 = table3_ref.get(vpn3);
    let ppn2 = if pte3.is_valid() && !pte3.is_leaf() {
        pte3.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_pmd_huge_page: failed to allocate PMD table");
        let ppn = table_phys >> PAGE_SHIFT;
        table3_ref.set(vpn3, PageTableEntry::new_table(ppn));
        ppn
    };

    // PMD leaf (2MB huge page): PTE bits [51:21] carry the 2MB-aligned
    // physical address; PS marks the leaf.
    assert!(phys % (PMD_SIZE as usize) == 0, "phys must be 2MB aligned for huge page");

    let entry_bits = ((phys as u64) & PageTableEntry::PHYS_MASK_PUBLIC) | PageTableEntry::PS | flags;
    let table2_phys = ppn2 << PAGE_SHIFT;
    let table2 = get_page_table_virt(table2_phys);
    (*table2).set(vpn2, PageTableEntry::from_bits(entry_bits));
}

/// Map a kernel virtual page to a physical page WITHOUT flushing the TLB.
///
/// Internal batch primitive — region callers flush once via
/// map_kernel_region. Walks from the CURRENTLY ACTIVE root (CR3): kernel
/// VAs resolve through the shared kernel-half links in any root.
unsafe fn map_kernel_page_noflush(virt: u64, phys: u64, flags: u64) {
    let root_ppn = root_page_table_ppn();
    map_page_noflush(root_ppn, VirtAddr::new(virt), PhysAddr::new(phys), flags);
}

/// Map a kernel virtual page to a physical page.
///
/// Used for vmemmap and other kernel mappings that need 4KB granularity.
pub unsafe fn map_kernel_page(virt: u64, phys: u64, flags: u64) {
    map_kernel_page_noflush(virt, phys, flags);
    crate::arch::cpu::invlpg(virt);
}

/// Map a region of kernel virtual pages to physical pages (VA and PA in
/// lockstep), using 2MB PMD leaves where alignment permits and 4K pages
/// for the head/tail — then flushes the TLB ONCE for the whole region.
pub unsafe fn map_kernel_region(virt: u64, phys: u64, size: u64, flags: u64) {
    let mut v = virt;
    let mut p = phys;
    let end = virt.saturating_add(size);
    while v < end {
        let remain = end - v;
        if v % PMD_SIZE == 0 && p % PMD_SIZE == 0 && remain >= PMD_SIZE {
            // 2MB superpage into the static kernel root
            map_pmd_huge_page(v as usize, p as usize, flags);
            v += PMD_SIZE;
            p += PMD_SIZE;
        } else {
            map_kernel_page_noflush(v, p, flags);
            v += PAGE_SIZE;
            p += PAGE_SIZE;
        }
    }
    // One full flush for the whole region (batch aggregation).
    crate::arch::mm::asid::flush_tlb_all();
}

/// Map a kernel device region using 2MB PMD leaf entries where the range
/// permits (interior 2MB-aligned span), falling back to 4K pages for the
/// head/tail. Must only be used for device memory mapped once at boot.
pub unsafe fn map_kernel_region_huge(virt: u64, size: u64, flags: u64) {
    const PMD: u64 = 0x20_0000;
    let mut v = virt;
    let end = virt.saturating_add(size);
    // Head: 4K pages up to the next 2MB boundary
    while v < end && v % PMD != 0 {
        map_kernel_page_noflush(v, v, flags);
        v += PAGE_SIZE;
    }
    // Interior: 2MB leaf entries (identity v==p)
    while v + PMD <= end {
        map_pmd_huge_page(v as usize, v as usize, flags);
        v += PMD;
    }
    // Tail: 4K pages
    while v < end {
        map_kernel_page_noflush(v, v, flags);
        v += PAGE_SIZE;
    }
    crate::arch::mm::asid::flush_tlb_all();
}

// ==================== MMU Initialization ====================

/// arch::mm::init() — build the real kernel PML4 and switch CR3.
///
/// Called from main.rs early, while the bootstrap tables are still active.
/// Contents:
/// - Kernel image at KERNEL_LINK_ADDR..__kernel_end (higher-half)
/// - PML4[0] linked to the bootstrap identity PUD (0..2GB) so the low
///   boot stack and early low-memory access stay valid across the switch
///   (bring-up simplification; drop with KPTI)
/// - Fixmap PUD pre-link (the 16MB window at FIXADDR_START)
///
/// The linear map at PAGE_OFFSET is added later by setup_linear_mapping().
pub fn init() {
    unsafe {
        println!("mm: x86_64 4-level page tables (building real PML4)");

        ROOT_PAGE_TABLE.zero();

        let root_ppn = root_static_ppn();

        // Kernel image mapping: RW supervisor pages. 2MB leaves where the
        // image boundary aligns, 4K for the tail (the image's constant
        // VMA-LMA offset keeps va and pa alignment-locked).
        let kernel_flags = PageTableEntry::P | PageTableEntry::RW
            | PageTableEntry::ACCESSED | PageTableEntry::DIRTY;

        let kernel_virt = KERNEL_LINK_ADDR as u64;
        let kernel_phys = KERNEL_ENTRY;
        let kernel_end_phys = kernel_image_phys_end();

        let mut phys = kernel_phys;
        let mut virt = kernel_virt;
        while phys < kernel_end_phys {
            let remaining = kernel_end_phys - phys;
            if remaining >= PMD_SIZE
                && (phys & (PMD_SIZE - 1)) == 0
                && (virt & (PMD_SIZE - 1)) == 0
            {
                map_pmd_huge_page(virt as usize, phys as usize, kernel_flags);
                phys += PMD_SIZE;
                virt += PMD_SIZE;
            } else {
                // No per-page flush — the CR3 switch below publishes the
                // whole table once.
                map_page_noflush(
                    root_ppn,
                    VirtAddr::new(virt),
                    PhysAddr::new(phys),
                    kernel_flags,
                );
                phys += PAGE_SIZE;
                virt += PAGE_SIZE;
            }
        }

        // Keep the bootstrap identity map alive: PML4[0] → boot_pdpt_lo
        // (identity 0..2GB, supervisor-only). The boot stack is low, and
        // early low-memory access patterns (multiboot blob copies) rely on
        // it until the linear map is up.
        let boot_pdpt_lo_phys = &raw const boot_pdpt_lo as usize as u64;
        ROOT_PAGE_TABLE.set(
            0,
            PageTableEntry::from_bits(boot_pdpt_lo_phys | PageTableEntry::P | PageTableEntry::RW),
        );

        // Fixmap PUD pre-link: the kernel image mapping already created
        // PML4[511]'s PUD table; install one PMD table for the fixmap
        // window so set_fixmap never needs a table allocation.
        {
            let fixmap_virt = VirtAddr::new(FIXADDR_START as u64);
            let vpn4 = fixmap_virt.pgd_index() as usize;
            let vpn3 = fixmap_virt.pud_index() as usize;
            let pte4 = ROOT_PAGE_TABLE.get(vpn4);
            if pte4.is_valid() {
                let pud = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
                if !(*pud).get(vpn3).is_valid() {
                    if let Some(pmd_phys) = alloc_page_table() {
                        (*pud).set(
                            vpn3,
                            PageTableEntry::new_table(pmd_phys >> PAGE_SHIFT),
                        );
                    }
                }
            }
        }

        // Switch to the real tables. The switching code path (this text,
        // the low boot stack) is identically mapped in the bootstrap and
        // the new tables, so the very next instruction after `mov cr3`
        // fetches fine; loading CR3 flushes the non-global TLB.
        let root_phys = root_ppn << PAGE_SHIFT;
        crate::arch::cpu::write_cr3(root_phys);

        MMU_INITIALIZED.store(true, Ordering::Release);

        // Record the real image size for /proc-style consumers.
        KERNEL_MAP.size = (kernel_image_phys_end() - KERNEL_ENTRY) as usize;

        println!(
            "mm: switched CR3 to real PML4 (phys {:#x}), image {:#x}-{:#x}",
            root_phys,
            kernel_virt,
            kernel_virt + KERNEL_MAP.size as u64
        );
    }
}

/// Setup linear mapping for physical memory (PAGE_OFFSET region).
///
/// Called from main.rs after memblock is initialized and the memory
/// regions are parsed; CR3 already points at the real PML4. All usable RAM
/// is mapped at PAGE_OFFSET with 2MB leaves where region boundaries align,
/// 4K pages otherwise.
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    unsafe {
        // Initialize KERNEL_MAP.va_pa_offset for phys_to_virt/virt_to_phys
        KERNEL_MAP.va_pa_offset = VA_PA_OFFSET;

        let linear_flags = PageTableEntry::P | PageTableEntry::RW
            | PageTableEntry::ACCESSED | PageTableEntry::DIRTY;

        let root_ppn = root_static_ppn();

        for region in memory_regions {
            let phys_start = region.base as u64;
            let size = region.size as u64;
            let phys_end = phys_start.saturating_add(size);

            let virt_start = phys_start + VA_PA_OFFSET as u64;

            let mut phys = phys_start;
            let mut virt = virt_start;

            while phys < phys_end {
                let remaining = phys_end - phys;
                // PAGE_OFFSET is 2MB-aligned and va-pa offset constant, so
                // va alignment tracks pa alignment (best_map_size logic).
                let map_size = if phys % PMD_SIZE == 0 && virt % PMD_SIZE == 0 && remaining >= PMD_SIZE
                {
                    PMD_SIZE
                } else {
                    PAGE_SIZE
                };

                if map_size == PMD_SIZE {
                    map_pmd_huge_page(virt as usize, phys as usize, linear_flags);
                } else {
                    // No per-page flush: the whole linear mapping is
                    // flushed once after this loop.
                    map_page_noflush(
                        root_ppn,
                        VirtAddr::new(virt),
                        PhysAddr::new(phys),
                        linear_flags,
                    );
                }

                phys += map_size;
                virt += map_size;
            }
        }

        // Reserve the early table pool in memblock so no Fixmap-stage
        // memblock_alloc can hand its frames out from under live tables.
        // (The pool is part of the kernel image BSS; this is belt and
        // braces for flows whose image reservation is size-based.)
        {
            let offset = KERNEL_MAP.va_kernel_pa_offset as u64;
            let pool_virt = &EARLY_TABLES as *const _ as u64;
            let pool_phys = pool_virt.wrapping_sub(offset);
            let used = (EARLY_TABLE_NEXT.load(Ordering::Acquire).min(NUM_EARLY_TABLES))
                * PAGE_SIZE as usize;
            let _ = crate::mm::memblock_reserve(pool_phys as usize, used);
        }

        // One full flush for the whole linear map (new PML4 entries).
        crate::arch::mm::asid::flush_tlb_all();
    }
}

/// E8-MM port: the low-half identity windows the kernel maps into EVERY
/// address space (ECAM / IOAPIC / LAPIC — the kernel touches these from
/// trap context while a USER CR3 is active, e.g. the LAPIC EOI write in
/// IRQ entry). `clear_pte` consults this so that unmapping a user page
/// mapped OVER one of these windows RESTORES the device translation
/// instead of punching an unmapped hole the kernel would page-fault into
/// on this mm's CR3.
///
/// Returns the raw PTE bits of the device translation for a page inside a
/// window (identity phys, P|RW|A|D|PWT|PCD — the same flags
/// setup_device_mappings installs), or None for ordinary user VAs.
pub fn kernel_device_window_pte(va: u64) -> Option<u64> {
    const WINDOWS: &[(u64, u64)] = &[
        (PCIE_ECAM_BASE, PCIE_ECAM_SIZE),  // q35 MMCONFIG, 256MB
        (0xc000_0000, 0x4000_0000),           // q35 32-bit PCI MMIO hole,
                                             // 0xc0000000..4GB (firmware
                                             // BARs incl. 0xfe000000 zone)
        (IOAPIC_BASE, 0x1000),
        (LAPIC_BASE, 0x1000),
    ];
    let page = va & !0xFFF;
    for &(base, size) in WINDOWS {
        if page >= base && page + 0x1000 <= base + size {
            let device_flags = PageTableEntry::P
                | PageTableEntry::RW
                | PageTableEntry::ACCESSED
                | PageTableEntry::DIRTY
                | PageTableEntry::IO;
            return Some((page & PageTableEntry::PHYS_MASK_PUBLIC) | device_flags);
        }
    }
    None
}

/// Device/ECAM mappings (q35: MMCONFIG @ 0xb0000000, IOAPIC, LAPIC).
///
/// These VAs sit ABOVE the bootstrap identity map's 2GB coverage, so the
/// walk allocates fresh PMD tables — no demotion of bootstrap leaves is
/// needed; the ECAM window goes in as 2MB IO leaves (128 entries).
pub fn setup_device_mappings() {
    unsafe {
        // Uncached device memory: PWT|PCD (the IO alias in pagetable.rs).
        let device_flags = PageTableEntry::P | PageTableEntry::RW
            | PageTableEntry::ACCESSED | PageTableEntry::DIRTY | PageTableEntry::IO;

        // q35 MMCONFIG ECAM window: 256MB, 2MB-aligned — megapages.
        map_kernel_region_huge(PCIE_ECAM_BASE, PCIE_ECAM_SIZE, device_flags);

        // 32-bit PCI MMIO hole 0xc0000000..4GB: the firmware assigns virtio
        // BARs here (observed at 0xfe000000). One full PD of 2MB leaves.
        map_kernel_region_huge(0xc000_0000, 0x4000_0000, device_flags);

        // IOAPIC + LAPIC: single 4K pages each (already covered by the hole
        // megapages above; re-mapping the 4K pages keeps their exactness).
        map_kernel_region(IOAPIC_BASE, IOAPIC_BASE, 0x1000, device_flags);
        map_kernel_region(LAPIC_BASE, LAPIC_BASE, 0x1000, device_flags);
    }
}

/// Re-point CR3 at the static kernel root (parity with riscv64 enable()).
pub fn enable() {
    unsafe {
        let root_phys = root_static_phys();
        crate::arch::cpu::write_cr3(root_phys);
    }
}

/// Map identity mapping into the static root (interface parity).
pub fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let vpn4 = virt.pgd_index() as usize;
    let ppn = phys.ppn();

    unsafe {
        ROOT_PAGE_TABLE.set(vpn4, PageTableEntry::from_bits((ppn << PAGE_SHIFT) | flags));
    }
}

/// Get kernel page table PPN (physical)
pub fn get_kernel_page_table_ppn() -> u64 {
    root_static_ppn()
}
