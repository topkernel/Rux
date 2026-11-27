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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use super::memory_layout::*;
use super::unsafe_edges::PtTable;
use super::pagetable::*;
use crate::mm::{MmStruct, alloc_pages, free_pages, GfpFlags};
use crate::mm::page_desc::MAX_PAGES;
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

/// Get the physical page number of the root page table.
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-42): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn root_page_table_ppn() -> u64 {
    crate::arch::arch_floor::riscv64::mm::mmu_init::root_page_table_ppn()
}

pub(crate) static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

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
pub(crate) const NUM_EARLY_PMD: usize = 8;   // L1 page tables (covers 8GB virtual space)
pub(crate) const NUM_EARLY_PTE: usize = 128; // L0 page tables (covers 256MB mapped space)



/// Counter for early page table allocation
pub(crate) static EARLY_PMD_NEXT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static EARLY_PTE_NEXT: AtomicUsize = AtomicUsize::new(0);

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
pub(crate) static ALLOC_STAGE: AtomicU8 = AtomicU8::new(AllocStage::Early as u8);

/// Get current allocation stage
pub fn get_alloc_stage() -> AllocStage {
    AllocStage::from_u8(ALLOC_STAGE.load(Ordering::Acquire))
}

/// FORENSIC: true once the buddy allocator is the table-frame source —
/// cheap check for the raw-free watchpoint in Zone::free_pages.
pub fn alloc_stage_is_late() -> bool {
    matches!(get_alloc_stage(), AllocStage::Late)
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
pub(crate) fn is_frame_allocator_ready() -> bool {
    get_alloc_stage() == AllocStage::Late
}
/// Allocate a page table and return its physical address
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-39): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn alloc_page_table() -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::mmu_init::alloc_page_table()
}

/// The phys→table conversion edge lives in the framework module now.
pub use super::unsafe_edges::get_page_table_virt;

pub(crate) fn free_page_table_checked(phys_addr: u64, site: &str) {
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
    crate::debug::memwatch::FUT_TABLES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    free_page_table(phys_addr)
}

pub(crate) static BOOT_TREE_STAMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// One-time: mark every page-table frame wired into the kernel's ROOT page
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-38): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn stamp_kernel_tree_boot() {
    crate::arch::arch_floor::riscv64::mm::mmu_init::stamp_kernel_tree_boot()
}
/// Free a page table (only valid for late stage allocations)
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-37): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn free_page_table(phys_addr: u64) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::free_page_table(phys_addr)
}

// FORENSIC: page-table frame ledger — every Late-stage alloc_page_table
// stamps the PPN; free_page_table clears it. A stamp on an already-stamped
// PPN = double allocation; clearing an unstamped or already-cleared PPN =
// double free / foreign free. Both are silent address-space corruptors
// under concurrent fork/exec.
pub struct PtLedger {
    // EXACT membership: one bit per PFN of the managed physical range
    // (PHYS_MEMORY_BASE..+PHYS_MEMORY_SIZE — MAX_PAGES bits, 64 KiB at
    // 2 GiB). Set by stamp(), cleared by take(). Zero collisions by
    // construction.
    //
    // The old 16384-slot direct hash (slot = ppn & 16383) saturated once
    // live table frames reached a few thousand (the ppn span maps onto
    // only span/16384 distinct slot groups — 32 at 2 GiB): unrelated
    // tables constantly overwrote each other's stamps, and take() then
    // REFUSED perfectly legitimate frees — every refusal permanently
    // leaked one 4 KiB table frame, and the serial filled with false
    // "PTLEDGER: DOUBLE-FREE REFUSED" / "FUT-STALE-TREE" reports (the
    // fake-OOM investigation's first dead end). A bitmap cannot collide.
    pub(crate) live: [core::sync::atomic::AtomicU64; MAX_PAGES / 64],
    // QUARANTINE bitset (mmforensics lane, 2026-11-15): set once, never
    // cleared, on any double-tracking guard firing for the frame. See
    // the x86_64 twin for the full rationale — a frame that was ever
    // tracked by two owners keeps unreclaimable stale references, and
    // every free path refuses poisoned frames so the circulation engine
    // (RAW-FREE echo family) dies at the first guard.
    pub(crate) poisoned: [core::sync::atomic::AtomicU64; MAX_PAGES / 64],
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
    pub(crate) boot: [core::sync::atomic::AtomicU64; 512],
    pub(crate) reported: core::sync::atomic::AtomicUsize,
}
impl PtLedger {
    pub(crate) const fn new() -> Self {
        Self {
            live: [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PAGES / 64],
            poisoned: [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PAGES / 64],
            boot: [const { core::sync::atomic::AtomicU64::new(0) }; 512],
            reported: core::sync::atomic::AtomicUsize::new(0),
        }
    }
    /// Bit index for an absolute PPN; None when outside the managed range.
    #[inline]
    pub(crate) fn bit_index(ppn: u64) -> Option<usize> {
        let base = crate::mm::page_desc::MIN_PFN as u64;
        let i = ppn.checked_sub(base)? as usize;
        if i < MAX_PAGES {
            Some(i)
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn live_bit(&self, ppn: u64) -> bool {
        match Self::bit_index(ppn) {
            Some(i) => {
                let word = &self.live[i / 64];
                word.load(Ordering::Relaxed) & (1u64 << (i % 64)) != 0
            }
            None => false,
        }
    }
    #[inline]
    pub(crate) fn set_live_bit(&self, ppn: u64, value: bool) -> bool {
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
    pub(crate) fn stamp_boot(&self, ppn: u64) {
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
    pub(crate) fn take_returns(&self, ppn: u64) -> bool {
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
    pub(crate) fn stamp(&self, ppn: u64, pid: u32) {
        use core::sync::atomic::Ordering::Relaxed;
        if self.set_live_bit(ppn, true) {
            crate::pr_err!(
                "PTLEDGER: DOUBLE-ALLOC ppn={:#x} by pid={} (still live)",
                ppn, pid
            );
            // Double-tracked frame: quarantine.
            Self::poison_table(ppn);
        }
        // FORENSIC (fake-OOM family): alloc-side ring — pairs with
        // PT_FREE_RING to replay a frame's full table-incarnation history.
        {
            let idx = PT_STAMP_CURSOR.fetch_add(1, Relaxed) % PT_FREE_RING.len();
            PT_STAMP_RING[idx].ppn.store(ppn, Relaxed);
            PT_STAMP_RING[idx].pid.store(pid, Relaxed);
            PT_STAMP_RING[idx].site.store(b'S' as u64, Relaxed);
            // mmforensics (x86_64 parity): record the stamping tree's
            // root; fetch gated off the default hot path.
            let root = if crate::trace::EVENT_MMFORENSICS.armed() {
                crate::sched::current()
                    .and_then(|t| t.address_space().map(|mm| mm.root_ppn()))
                    .unwrap_or(0)
            } else {
                0
            };
            PT_STAMP_RING[idx].root.store(root, Relaxed);
        }
    }

}
pub static PT_LEDGER: PtLedger = PtLedger::new();

// FORENSIC (fake-OOM family): ring of successful page-table frees, recorded
// by PtLedger::take() — replayed when the stale-tree census fires.
pub struct PtFreeEntry {
    pub ppn: core::sync::atomic::AtomicU64,
    pub pid: core::sync::atomic::AtomicU32,
    pub site: core::sync::atomic::AtomicU64,
    /// Root of the tree that stamped/freed the frame (mmforensics
    /// parity with x86_64: at a RAW-FREE echo this names the frame's
    /// other owner).
    pub root: core::sync::atomic::AtomicU64,
}
impl PtFreeEntry {
    pub(crate) const fn new() -> Self {
        Self {
            ppn: core::sync::atomic::AtomicU64::new(0),
            pid: core::sync::atomic::AtomicU32::new(0),
            site: core::sync::atomic::AtomicU64::new(0),
            root: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
pub(crate) const PT_FREE_NEW: PtFreeEntry = PtFreeEntry::new();
pub static PT_FREE_RING: [PtFreeEntry; 4096] = [PT_FREE_NEW; 4096];
pub static PT_FREE_CURSOR: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
pub static PT_STAMP_RING: [PtFreeEntry; 4096] = [PT_FREE_NEW; 4096];
pub static PT_STAMP_CURSOR: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

// FORENSIC (fake-OOM family): fork-mm registry — every AddressSpace::fork
// records its fresh root. When a registered root's ledger stamp is taken,
// the taker is compared against the registered owner: a free by any other
// context while the owner task is still alive is the EARLY-TEARDOWN
// witness (the seed of the stale-tree corruption family).
pub struct ForkRootReg {
    pub root: core::sync::atomic::AtomicU64,
    pub owner: core::sync::atomic::AtomicU32,
}
impl ForkRootReg {
    pub(crate) const fn new() -> Self {
        Self {
            root: core::sync::atomic::AtomicU64::new(0),
            owner: core::sync::atomic::AtomicU32::new(0),
        }
    }
}
pub(crate) const FORK_ROOT_NEW: ForkRootReg = ForkRootReg::new();
pub static FORK_ROOTS: [ForkRootReg; 256] = [FORK_ROOT_NEW; 256];
pub static FORK_ROOT_CURSOR: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

pub fn register_fork_root(root_ppn: u64, owner_pid: u32) {
    let idx = FORK_ROOT_CURSOR.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
        % FORK_ROOTS.len();
    FORK_ROOTS[idx].root.store(root_ppn, core::sync::atomic::Ordering::Relaxed);
    FORK_ROOTS[idx].owner.store(owner_pid, core::sync::atomic::Ordering::Relaxed);
}
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-34): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn task_pid_alive(pid: u32) -> bool {
    crate::arch::arch_floor::riscv64::mm::mmu_init::task_pid_alive(pid)
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

    /// FORENSIC (fake-OOM family): live-stamp probe for the stale-tree
    /// census. Returns 1 when the frame carries a live table stamp, 0 when
    /// not (exact bitmap — no hash-collision ambiguity).
    pub fn peek(&self, ppn: u64) -> u64 {
        self.live_bit(ppn) as u64
    }

    /// FORENSIC: pid that last successfully freed this ppn's stamp
    /// (most recent PT_FREE_RING entry naming the ppn; 0 = unknown).
    pub fn freed_by_peek(&self, ppn: u64) -> u32 {
        pt_free_ring_prev_free_pid(ppn)
    }

    /// FORENSIC (fake-OOM family): does this frame carry a LIVE table stamp?
    /// Used by the raw-free watchpoint in Zone::free_pages — a raw
    /// (non-page-table-path) free of a live-stamped frame is the seed event
    /// that turns a live page-table tree stale.
    /// Quarantine a frame (see the `poisoned` field doc). One-way.
    pub fn poison_table(ppn: u64) {
        match Self::bit_index(ppn) {
            Some(i) => {
                PT_LEDGER.poisoned[i / 64]
                    .fetch_or(1u64 << (i % 64), core::sync::atomic::Ordering::Relaxed);
            }
            None => {}
        }
    }

    /// Is this frame quarantined (ever double-tracked)? Every free path
    /// refuses such frames; cache pins refuse to serve them.
    pub fn is_poisoned(ppn: u64) -> bool {
        match Self::bit_index(ppn) {
            Some(i) => {
                PT_LEDGER.poisoned[i / 64]
                    .load(core::sync::atomic::Ordering::Relaxed)
                    & (1u64 << (i % 64))
                    != 0
            }
            None => false,
        }
    }

    pub fn is_live_table(ppn: u64) -> bool {
        PT_LEDGER.live_bit(ppn)
    }

    /// Accounting ledger: number of frames currently carrying a live
    /// page-table stamp (exact bitmap popcount, bounded by the runtime
    /// descriptor count). This is the authoritative PageTables figure.
    pub fn live_count() -> usize {
        let bits = crate::mm::page_desc::nr_descs().min(MAX_PAGES);
        let mut n = 0usize;
        for i in 0..bits.div_ceil(64) {
            n += PT_LEDGER.live[i].load(core::sync::atomic::Ordering::Relaxed)
                .count_ones() as usize;
        }
        n
    }
}

/// FORENSIC: newest PT_FREE_RING entry naming `ppn` — the pid context of its
/// most recent legitimate free (0 when the frame never freed within the
/// ring window). Backwards scan from the cursor; the freshest match wins.
pub(crate) fn pt_free_ring_prev_free_pid(ppn: u64) -> u32 {
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
    pub root: core::sync::atomic::AtomicU64,
    pub pid: core::sync::atomic::AtomicU64,
}
impl FutEntry {
    pub(crate) const fn new() -> Self {
        Self {
            root: core::sync::atomic::AtomicU64::new(0),
            pid: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
pub(crate) const FUT_NEW: FutEntry = FutEntry::new();
pub static FUT_RING: [FutEntry; 1024] = [FUT_NEW; 1024];
pub static FUT_RING_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// Global cap on "REPEAT teardown" forensic reports. Even with a
/// 1024-entry ring, high fork-churn tests (epoll-ltp: 13824 fork/exit
/// protected regions) recycle root ppns within the ring window
/// constantly — every benign teardown can match a stale entry and print,
/// flooding the serial console (10k+ lines per test) and drowning every
/// other diagnostic. The first handful of reports carry the forensic
/// value; the rest are duplicates.
pub static FUT_REPEAT_REPORTS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Free all page tables and user data pages used by a user address space
///
/// Only frees USER space page tables (VPN2 0-255 with U=1).
/// Kernel mappings (U=0) are shared and should NOT be freed.
///
/// IMPORTANT: For non-leaf L2 entries, U bit is not meaningful (R/W/X=0).
/// We must walk all valid user-space L2 entries, not skip them based on U bit.

#[inline]
pub(crate) fn is_framebuffer_frame(phys: u64) -> bool {
    // Device frames (virtio-gpu framebuffer) are NOT RAM pages: tearing
    // them down through put_page/free_pages corrupts the page_desc
    // accounting (mapcount/refcount of an unrelated descriptor) — the
    // recycled-frame family's amplifier.
    if let Some(info) = crate::drivers::gpu::get_framebuffer_info() {
        if phys >= info.addr && phys < info.addr + info.size as u64 {
            return true;
        }
    }
    // Any other boot-reserved frame is equally foreign to the user mm:
    // drm GEM dumb-buffer backing is kernel-heap memory mmap(2)'d into
    // userspace (drm::create_dumb + gem_mmap_resolve). The owner frees
    // it through its own path (GemObject::drop -> heap dealloc), so the
    // teardown walk must skip it — no rmap, no put_page, no free.
    // Without this, fork's COW copy refcounted the foreign frames and
    // the last teardown's put_page(0) called free_pages on kernel-heap
    // phys (sc83: 7442-line REFUSED printk flood + lock wedge).
    crate::mm::page_alloc::is_foreign_user_frame(phys as usize)
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
    pub(crate) const fn new() -> Self {
        Self {
            root: core::sync::atomic::AtomicU64::new(0),
            va: core::sync::atomic::AtomicU64::new(0),
            ppn: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
pub(crate) const PTEI_NEW: PteInstall = PteInstall::new();
pub static PTEI_RING: [PteInstall; 65536] = [PTEI_NEW; 65536];
pub static PTEI_CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[inline]
pub fn pte_install_log(root_ppn: u64, va: u64, ppn: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    // Hot path (every user PTE install — a fork copies hundreds of them):
    // the ring writes are forensic-only (fake-OOM PTE-alias hunt) and cost
    // 4 emulated atomics per copied PTE under TCG. Behind the
    // rux:mmforensics (retired dfx=mmforensics) runtime switch — LTP fork_procs lost seconds per
    // 1000 fork+exit cycles to this logging.
    if !crate::trace::EVENT_MMFORENSICS.armed() {
        return;
    }
    let i = PTEI_CUR.fetch_add(1, Relaxed) % PTEI_RING.len();
    PTEI_RING[i].root.store(root_ppn, Relaxed);
    PTEI_RING[i].va.store(va, Relaxed);
    PTEI_RING[i].ppn.store(ppn, Relaxed);
}

// ==================== SIGILL foreign-free adjudication (gnome-sigill) ====================

/// Scan `root_ppn`'s user half for leaf PTEs mapping `pfn`; returns
/// (total_hits, up to 4 vas). Read-only, called only from the rare
/// foreign-free report path.
pub(crate) fn owner_maps_pfn(root_ppn: u64, pfn: usize) -> (usize, [usize; 4]) {
    let mut hits = 0usize;
    let mut vas = [0usize; 4];
    if root_ppn == 0 {
        return (hits, vas);
    }
    // SAFETY: diagnostic read-only walk of a live task's page-table tree
    // (serialized by the caller's PTE_MODIFY_LOCK); every table pointer is
    // derived from a valid PTE chain.
    // Typed read-only walk (the round-1 PtTable edge); a guard miss is
    // a corrupt tree — report no hits (the diagnostic caller prints the
    // stamps regardless).
    let root = match PtTable::from_phys(root_ppn << PAGE_SHIFT) {
        Some(t) => t,
        None => return (0, [0usize; 4]),
    };
    let mut hits = 0usize;
    let mut vas = [0usize; 4];
    for vpn2 in 0..crate::arch::mm::memory_layout::KERNEL_PGD_START {
            let pte2 = root.get(vpn2);
            if !pte2.is_valid() || pte2.is_leaf() {
                continue; // 1G user leaf: no file text lives there in practice
            }
            let Some(t1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else { continue };
            for vpn1 in 0..512 {
                let pte1 = t1.get(vpn1);
                if !pte1.is_valid() || pte1.is_leaf() {
                    continue;
                }
                let Some(t0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else { continue };
                for vpn0 in 0..512 {
                    let pte0 = t0.get(vpn0);
                    if !pte0.is_valid() || !pte0.is_leaf() || !pte0.is_user() {
                        continue;
                    }
                    if pte0.ppn() as usize == pfn {
                        if hits < vas.len() {
                            vas[hits] = (vpn2 << 30) | (vpn1 << 21) | (vpn0 << 12);
                        }
                        hits += 1;
                    }
                }
            }
    }
    (hits, vas)
}
/// Look up task `pid`; when alive, count its live PTEs mapping `pfn`
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-33): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn textfree_owner_maps(owner_pid: u32, pfn: usize) -> usize {
    crate::arch::arch_floor::riscv64::mm::mmu_init::textfree_owner_maps(owner_pid, pfn)
}
/// Teardown-walker leaf release with stamp adjudication (gnome-sigill).
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-32): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn release_user_leaf( page: &crate::mm::page_desc::Page, pfn: usize, va: usize, ) -> bool {
    crate::arch::arch_floor::riscv64::mm::mmu_init::release_user_leaf(page, pfn, va)
}

/// `pte` is the PTE section for the teardown chunks (INV-LOCK-7; the
/// dying mm's own PteLock, or FRESH_TREE_PTE_LOCK for unpublished trees).
/// riscv64 is GLOBAL mode, so every handle maps to the same global epoch.
// (fork-frag lane) x86_64-only deferred page-table-free protocol: munmap
// on x86_64 unlinks and defer-frees emptied leaf tables behind a
// TLB-flush epoch (see arch/x86_64/mm/mmu_init.rs). This arch's munmap
// never frees tables, so the drain is a no-op twin kept so arch-generic
// churn points (fork, exit) compile identically.
pub fn pt_pending_drain() {}

pub fn free_user_page_tables(
    pte: &crate::mm::mm_struct::PteLock,
    root_ppn: u64,
    // (sysmm-2x lane) VMA range snapshot bounding the leaf scans —
    // implemented on the x86_64 twin first (platform priority); the
    // riscv64 walk keeps its full scans and ignores the list.
    _vma_ranges: &[(u64, u64)],
) {
    use crate::mm::{page_desc, phys_to_pfn, phys_valid, page_desc::PageFlag, free_pages};
    crate::debug::memwatch::FUT_CALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Double-teardown detector: the same root walked twice means two
    // MmStructs ended up owning one page-table tree — the second walk
    // chases recycled frames full of foreign data (the corruption family
    // behind do_wait children-list panics). Ring is small; a hit here is
    // not proof for old trees, but a fresh repeat IS.
    // Every exit pays a 1024-entry linear scan for it — behind the
    // rux:mmforensics (retired dfx=mmforensics) runtime switch (see pte_install_log).
    if crate::trace::EVENT_MMFORENSICS.armed() {
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
                    // dfx-migrate: the double-teardown hit also records
                    // into the unified framework (rux:mmforensics
                    // kind=FUT_REPEAT; b packs prev<<32|now pid).
                    crate::trace::trace_mmforensics(
                        0,
                        root_ppn,
                        (FUT_RING[k].pid.load(Relaxed) << 32)
                            | crate::sched::get_current_pid() as u64,
                        crate::sched::get_current_pid() as u32,
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
    // FORENSIC (fake-OOM family): stale-tree census. Every intermediate
    // table frame this walk reaches must carry a LIVE ledger stamp (its
    // allocation incarnation). A missing stamp means the frame was freed
    // earlier and this tree has been pointing at recycled memory — the
    // walk is about to put_page/free whatever now lives there. Collected
    // here (with vpn indices), printed once per offending teardown.
    let mut stale_tables: [(u8, usize, usize, u64); 16] = [(0, 0, 0, 0); 16];
    let mut stale_count = 0usize;
    // Serialize against concurrent fork copies / COW faults on ANY mm: the
    // pages freed here can be immediately reallocated as page tables or
    // COW copies by another CPU (PTE_MODIFY_LOCK, NEW2 class).
    let _pte_guard = pte.lock_irqsave();

    let root_phys = root_ppn << PAGE_SHIFT;
    // Dying-mm root: a guard miss is corruption — free the root frame
    // itself and return (the containment the inline guard gave).
    let Some(root_table) = PtTable::from_phys(root_phys) else {
        free_page_table_checked(root_phys, "root");
        return;
    };

    // Walk and free all levels (only user space: VPN2 0-255)
    for vpn2 in 0..256 {
        let pte2 = root_table.get(vpn2);
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
            if let Some(page) = page_desc(pfn) {
                // SIGILL-stamp adjudicated release (gnome-sigill)
                release_user_leaf(page, pfn, vpn2 << 30);
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

        let Some(table1) = PtTable::from_phys(table1_phys) else { continue };

        for vpn1 in 0..512 {
            let pte1 = table1.get(vpn1);
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
                if let Some(page) = page_desc(pfn) {
                    // SIGILL-stamp adjudicated release (gnome-sigill)
                    release_user_leaf(page, pfn, (vpn2 << 30) | (vpn1 << 21));
                }
                continue;
            }

            let ppn0 = pte1.ppn();
            let table0_phys = ppn0 << PAGE_SHIFT;

            if !phys_valid(table0_phys as usize) {
                continue;
            }

            let Some(table0) = PtTable::from_phys(table0_phys) else { continue };

            for vpn0 in 0..512 {
                let pte0 = table0.get(vpn0);

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

                if !phys_valid(phys_addr as usize) || phys_addr < 0x80000000 {
                    continue;
                }
                let Some(page) = page_desc(pfn) else { continue };

                // SIGILL-stamp adjudicated release (gnome-sigill)
                release_user_leaf(
                    page,
                    pfn,
                    (vpn2 << 30) | (vpn1 << 21) | (vpn0 << 12),
                );
                // PCACHE leaf (filemap_fault, riscv64 port): drop the
                // mapping's page-cache pin after the descriptor put inside
                // release_user_leaf (same ordering as unmap_pages; the
                // pin's release may free an invalidated frame). PCACHE only
                // ever sits on 4K user leaves — the L1/L2 leaf arms above
                // never fault through the file path.
                if pte0.bits() & super::mm_ops::cow_flags::PCACHE != 0 {
                    super::mm_ops::release_cache_mapping_pin(pfn);
                }
            }
            // FORENSIC: census the L0 table before freeing it.
            if get_alloc_stage() == AllocStage::Late && !PT_LEDGER.is_boot(ppn0) {
                let live = PT_LEDGER.peek(ppn0) != 0;
                if !live {
                    if stale_count < stale_tables.len() {
                        stale_tables[stale_count] = (0, vpn2, vpn1, ppn0);
                    }
                    stale_count += 1;
                }
            }
            free_page_table_checked(table0_phys, "l0");
        }
        // FORENSIC: census the L1 table before freeing it.
        if get_alloc_stage() == AllocStage::Late && !PT_LEDGER.is_boot(ppn1) {
            let live = PT_LEDGER.peek(ppn1) != 0;
            if !live {
                if stale_count < stale_tables.len() {
                    stale_tables[stale_count] = (1, vpn2, 0xFFFF, ppn1);
                }
                stale_count += 1;
            }
        }
        free_page_table_checked(table1_phys, "l1");
    }

    if stale_count > 0 {
        static STALE_REPORTS: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if STALE_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 6 {
            crate::pr_err!(
                "FUT-STALE-TREE: root ppn={:#x} pid={} walks {} foreign/unstamped table frames:",
                root_ppn, crate::sched::get_current_pid(), stale_count
            );
            for i in 0..stale_count.min(stale_tables.len()) {
                let (lvl, v2, v1, ppn) = stale_tables[i];
                crate::pr_err!(
                    "  FUT-STALE: lvl={} vpn2={} vpn1={} ppn={:#x}",
                    lvl, v2, v1, ppn
                );
            }
            // Root-slot forensics: who freed THIS root's ledger stamp last?
            crate::pr_err!(
                "  FUT-STALE: root slot peek={:#x} last-freed-by pid={}",
                PT_LEDGER.peek(root_ppn),
                PT_LEDGER.freed_by_peek(root_ppn)
            );
            // Replay of the stamp/free rings: search them for the FIRST few
            // stale ppns (+ the root) and print every recorded incarnation.
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
            // Recent teardown history — with a 1024-entry ring this reaches
            // back past a full repro round; entries naming THIS root are the
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

    // mmforensics: deep capture naming the first stale table frame (the
    // stale-tree census witness — x86_64 parity).
    if stale_count > 0 {
        crate::debug::mmforensics::capture_frame(stale_tables[0].3 as usize, "fut-stale-table");
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
/// Rate-limited one-liner naming the walk level whose page-table
/// allocation failed (fake-OOM family). The loud allocator-state dump
/// lives in alloc_page_table (oom_forensic_dump, itself rate-limited);
/// this only marks the failure depth in the serial log.
pub(crate) fn pt_alloc_fail(level: &str, va: u64) {
    static N: core::sync::atomic::AtomicUsize =
        core::sync::atomic::AtomicUsize::new(0);
    if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
        crate::pr_err!(
            "map_page: PT alloc failed ({} level) at va {:#x}",
            level,
            va
        );
    }
}
/// Map one 4K page; returns false when the mapping could NOT be installed
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-31): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn map_page_noflush(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_page_noflush(root_ppn, virt, phys, flags)
}
/// Map a single page in page table
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-30): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_page(root_ppn, virt, phys, flags)
}
/// Map a region with identity mapping
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-29): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn map_region(root_ppn: u64, start: u64, size: u64, flags: u64) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_region(root_ppn, start, size, flags)
}
/// Map a 2MB huge page using PMD leaf entry
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-28): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn map_pmd_huge_page(virt: usize, phys: usize, flags: u64) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_pmd_huge_page(virt, phys, flags)
}
/// Map a kernel virtual page to a physical page WITHOUT flushing the TLB.
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-27): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn map_kernel_page_noflush(virt: u64, phys: u64, flags: u64) -> bool {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_kernel_page_noflush(virt, phys, flags)
}
/// Map a kernel virtual page to a physical page
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-26): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn map_kernel_page(virt: u64, phys: u64, flags: u64) -> bool {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_kernel_page(virt, phys, flags)
}
/// Map a region of kernel virtual pages to physical pages (identity mapped MMIO)
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-25): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn map_kernel_region(virt: u64, phys: u64, size: u64, flags: u64) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_kernel_region(virt, phys, size, flags)
}
/// Map a kernel device region using 2MB megapage leaf entries where the
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-24): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn map_kernel_region_huge(virt: u64, size: u64, flags: u64) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_kernel_region_huge(virt, size, flags)
}
/// Map device memory page to user space
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-23): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn map_device_page(virt: usize, phys: usize, flags: u64) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_device_page(virt, phys, flags)
}

/// Select best mapping size
#[inline]
pub(crate) fn best_map_size(pa: usize, va: usize, size: usize) -> usize {
    const PMD_MASK: usize = (PMD_SIZE as usize) - 1;

    // For 64-bit: use PMD_SIZE (2MB) if aligned
    if (pa & PMD_MASK) == 0 && (va & PMD_MASK) == 0 && size >= PMD_SIZE as usize {
        PMD_SIZE as usize
    } else {
        PAGE_SIZE as usize
    }
}

// ==================== MMU Initialization ====================

/// Initialize MMU
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-21): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init() {
    crate::arch::arch_floor::riscv64::mm::mmu_init::init()
}

/// E8-MM: the low-half (identity, va == phys) device windows the kernel
/// maps into EVERY address space (see setup_device_mappings and
/// copy_kernel_mappings — the kernel may access any of them from a trap
/// while a USER satp is active, e.g. the PLIC claim read in IRQ entry).
///
/// `clear_pte` consults this so that unmapping a user page that was
/// mapped OVER one of these windows (possible since map_page started
/// demoting megapages for user fixed-address mmaps — Linux MAP_FIXED
/// replace semantics) RESTORES the device translation instead of
/// punching an unmapped hole the kernel would page-fault into on this
/// mm's satp (claim at 0x0c201004: fault in S-mode = KERNPANIC).
///
/// Returns the raw PTE bits of the device translation for a page inside
/// a window (identity phys, V|R|W|A|D — the same flags
/// setup_device_mappings installs), or None for ordinary user VAs.
pub fn kernel_device_window_pte(va: u64) -> Option<u64> {
    const WINDOWS: &[(u64, u64)] = &[
        (CLINT_BASE, 0x1_0000),                                // CLINT
        (PLIC_BASE, 0x21_0000),                                // PLIC prio/pending/enable/contexts
        (VIRTIO_MMIO_BASE & !0xFFF, 0x9_000),                  // UART + virtio-mmio slots
        (crate::drivers::rtc::GOLDFISH_RTC_BASE, crate::drivers::rtc::GOLDFISH_RTC_SIZE),
        (PCIE_ECAM_BASE, 0x800_000),                           // PCIe ECAM buses 0-7
        (PCI_MMIO_BASE, 0x1000_0000),                          // PCI MMIO BAR window
    ];
    let page = va & !0xFFF;
    for &(base, size) in WINDOWS {
        if page >= base && page + 0x1000 <= base + size {
            let device_flags = PageTableEntry::V
                | PageTableEntry::R
                | PageTableEntry::W
                | PageTableEntry::A
                | PageTableEntry::D;
            return Some(((page >> PAGE_SHIFT) << 10) | device_flags);
        }
    }
    None
}

/// Setup device mappings (called after fixmap stage is ready)
#[allow(dead_code)]
pub fn setup_device_mappings() {
    {
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

        // Kernel-half MMIO alias of the whole low window (phys
        // [0, MMIO_ALIAS_SIZE) at MMIO_ALIAS_BASE): the permanent,
        // user-unreachable translation every runtime MMIO accessor uses
        // via mmio_alias(). A user MAP_FIXED mapping may legitimately
        // replace the low identity copies in its own root (gjs's heap
        // cage at 0x0c000000 does); the kernel keeps working through
        // the alias either way. 2MB leaves keep the clone cost per
        // address space at a handful of L1 entries.
        map_kernel_region(
            MMIO_ALIAS_BASE as u64,
            0,
            MMIO_ALIAS_SIZE as u64,
            device_flags,
        );
    }
}
/// Setup linear mapping for physical memory
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-20): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::setup_linear_mapping(memory_regions)
}
/// Enable MMU (secondary function)
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-19): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable() {
    crate::arch::arch_floor::riscv64::mm::mmu_init::enable()
}
/// Map identity mapping
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-18): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::map_identity(virt, phys, flags)
}
/// Get kernel page table PPN (physical)
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-17): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_kernel_page_table_ppn() -> u64 {
    crate::arch::arch_floor::riscv64::mm::mmu_init::get_kernel_page_table_ppn()
}

// ==================== mmforensics: live-tree reverse scan ====================
/// Bounded reverse scan: which LIVE Sv39 trees still reference
///
/// Collected into `arch::arch_floor` (INV-ARCH-MMU-INIT-16): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn forensics_scan_trees( target_ppn: u64, out: &mut [crate::debug::mmforensics::TreeHit], ) -> (usize, usize, usize, usize) {
    crate::arch::arch_floor::riscv64::mm::mmu_init::forensics_scan_trees(target_ppn, out)
}

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: setup_vm moved to arch::arch_floor::riscv64::mm::mmu_init (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::mm::mmu_init::setup_vm;
pub(crate) use crate::arch::arch_floor::riscv64::mm::mmu_init::EARLY_PTE;
pub(crate) use crate::arch::arch_floor::riscv64::mm::mmu_init::EARLY_PMD;
pub(crate) use crate::arch::arch_floor::riscv64::mm::mmu_init::ROOT_PAGE_TABLE;
