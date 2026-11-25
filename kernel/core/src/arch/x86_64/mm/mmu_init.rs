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

use super::r#unsafe::PtTable;
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
pub fn root_page_table_ppn() -> u64 {
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
/// Diagnostic: early-pool tables consumed so far.
pub fn early_tables_used() -> usize {
    EARLY_TABLE_NEXT.load(Ordering::Acquire)
}

/// INV-XE-ZP (zero-pages edge, owned by mm/unsafe.rs): `phys` is a
/// frame this caller just took from a page/table allocator and owns
/// exclusively; zeroing it cannot race any other accessor. The linear-map
/// (or early-pool VMA) address is derived by the same stage-aware
/// conversion [`get_page_table_virt`] uses.
#[inline]
pub(crate) fn zero_owned_pages(virt: u64, bytes: usize) {
    // SAFETY: INV-XE-ZP — freshly allocated, exclusively owned frames;
    // the address comes from the audited stage-aware conversion.
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, bytes) };
}

pub fn alloc_page_table() -> Option<u64> {
    let stage = get_alloc_stage();
    match stage {
        AllocStage::Early => {
            // Early boot: use the static BSS pool. phys = virt - va_kernel_pa_offset
            // (the image's constant VMA-LMA offset holds for the whole image).
            // SAFETY: single-CPU boot section; the pool slot is claimed
            // exclusively by the fetch_add below.
            let offset = unsafe { KERNEL_MAP.va_kernel_pa_offset } as u64;
            let idx = EARLY_TABLE_NEXT.fetch_add(1, Ordering::AcqRel);
            if idx < NUM_EARLY_TABLES {
                let table_virt = unsafe { &raw const EARLY_TABLES[idx] as u64 };
                let table_phys = table_virt.wrapping_sub(offset);
                // SAFETY: static BSS pool slot just claimed exclusively by
                // the fetch_add above (INV-XE-ZP).
                unsafe { core::ptr::write_bytes(table_virt as *mut u8, 0, PAGE_SIZE as usize) };
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
            zero_owned_pages(virt_addr.bits(), PAGE_SIZE as usize);
            Some(phys_addr as u64)
        }
        AllocStage::Late => {
            // Late stage: use zone allocator
            use crate::mm::zone::ZoneType;

            // SAFETY: node-table mutation is the zone allocator's own
            // discipline (single-threaded bring-up / zone-locked paths).
            let phys_addr = if let Some(node) = unsafe { crate::mm::pglist::first_online_node_mut() } {
                if let Some(zone) = node.zone_mut(ZoneType::ZoneNormal) {
                    if zone.is_initialized() {
                        // (fork-frag lane) Per-CPU pageset fast path: a
                        // fork's COW walk allocates one table per parent
                        // leaf table (36 on the fragmented pbench shape)
                        // and each used to pay a full zone-lock round
                        // trip. The pcp row serves order-0 frames with no
                        // zone lock (refill pays it once per PCP_BATCH) —
                        // the exact fast path page_alloc::alloc_pages
                        // gives data pages. Slowpath/reclaim semantics
                        // unchanged: a pcp miss falls through to the
                        // zone path below verbatim (the GFP slowpath
                        // stays OUT of here — this call may run under the
                        // mm PteLock, and swap-out reclaim of the same mm
                        // would self-deadlock; see the shrink comment).
                        let mut pfn_opt = crate::mm::pcp::alloc_order0(zone);
                        if pfn_opt.is_none() {
                            pfn_opt = zone.alloc_pages(0);
                        }
                        if pfn_opt.is_none() {
                            // PT-alloc slowpath (fake-OOM family): the
                            // demand-fault path allocates its DATA page
                            // through page_alloc::alloc_pages (watermark
                            // wakeup + direct reclaim) and then calls
                            // map_page for the TABLE page — the old raw
                            // zone.alloc_pages here bypassed reclaim, so
                            // a zone at its reclaim edge failed the TABLE
                            // alloc right after succeeding the data alloc,
                            // and map_page's .expect turned a recoverable
                            // failure into a kernel panic. Reclaim only
                            // the subset that is safe from this context:
                            // we may run under the caller's per-mm PteLock,
                            // and page-cache eviction of CLEAN pages takes
                            // no mm PTE locks and performs no I/O.
                            // Swap-out reclaim (rmap unmap) is deliberately
                            // NOT attempted — unmapping a page of the SAME
                            // mm would self-deadlock on the PteLock we hold.
                            for _ in 0..4 {
                                let freed =
                                    crate::fs::page_cache::get_page_cache().shrink(32);
                                if freed == 0 {
                                    break;
                                }
                                pfn_opt = zone.alloc_pages(0);
                                if pfn_opt.is_some() {
                                    break;
                                }
                            }
                            if pfn_opt.is_none() {
                                // One-shot ledger + zone + census dump
                                // (internally rate-limited): attributes
                                // the drain (genuine exhaustion vs broken
                                // free lists vs leak) on the serial log.
                                crate::mm::page_alloc::oom_forensic_dump(
                                    "alloc_page_table order-0",
                                );
                            }
                        }
                        match pfn_opt {
                            Some(pfn) => {
                                // SCRIBBLE HUNT: zone starts above the image; a
                                // low PFN here means the zone was mis-seeded.
                                if (pfn << 12) < 0x1E0_0000 {
                                    crate::pr_err!(
                                        "PTALLOC-LOW-HIT ppn={:#x} — below zone floor!",
                                        pfn
                                    );
                                }
                                if let Some(page) = crate::mm::page_desc::page_desc(pfn) {
                                    page.set_refcount(1);
                                    page.set_order(0);
                                    page.set_flag(crate::mm::page_desc::PageFlag::Referenced);
                                }
                                crate::mm::zone::pfn_to_phys(pfn) as u64
                            }
                            None => 0,
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

            // DEBUG memwatch: page-table frames bypass page_alloc::alloc_pages
            // (direct zone.alloc_pages above) — feed them to the page-level
            // site table from THIS frame so the chain names the mapping path
            // (map_page / create_user_address_space / ...).
            if crate::debug::memwatch::armed() {
                let mut mw_frames: [u64; crate::debug::memwatch::SITE_FRAMES] =
                    [0; crate::debug::memwatch::SITE_FRAMES];
                // SAFETY: reading rbp is register-only; the frame-pointer
                // chain walk is the debug edge (guarded against wild frames).
                let mw_bp: u64 = unsafe {
                    let v;
                    core::arch::asm!("mov {}, rbp", out(reg) v, options(nomem, nostack));
                    v
                };
                // SAFETY: the chain walk is the debug stack-forensics edge;
                // mw_bp is this frame's own frame pointer.
                unsafe { crate::debug::memwatch::walk_fp_chain(mw_bp, &mut mw_frames) };
                crate::debug::memwatch::note_page_alloc(phys_addr as usize, 0, &mw_frames);
            }

            // FORENSIC ledger: stamp every Late-stage table allocation.
            PT_LEDGER.stamp(phys_addr >> PAGE_SHIFT, crate::sched::get_current_pid());

            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            zero_owned_pages(virt_addr.bits(), PAGE_SIZE as usize);
            Some(phys_addr)
        }
    }
}

/// The phys→table conversion edge lives in the framework module now
/// (one audited site per arch; see unsafe.rs).
pub use super::r#unsafe::get_page_table_virt;

fn free_page_table_checked(phys_addr: u64, site: &str) {
    free_page_table_checked_rooted(phys_addr, site, 0)
}

/// `root_ppn` feeds the sharing-pair forensics ring (which tree freed the
/// frame). Callers inside free_user_page_tables pass the tree root.
fn free_page_table_checked_rooted(phys_addr: u64, site: &str, root_ppn: u64) {
    if get_alloc_stage() == AllocStage::Late {
        stamp_kernel_tree_boot();
        if !PT_LEDGER.take_returns(phys_addr >> PAGE_SHIFT) {
            return; // boot-shared table: never free, never reuse
        }
        if !PT_LEDGER.take(phys_addr >> PAGE_SHIFT, crate::sched::get_current_pid(), site, root_ppn) {
            return; // double/foreign free: refuse, keep the frame leaked
                    // rather than wired into two live trees
        }
    }
    crate::debug::memwatch::FUT_TABLES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // 0xb9 canary (oh-launch round 7): scan the table's content BEFORE
    // the free for flags-only entries — PRESENT with zero address bits
    // (phys page 0 is never a legal target; the x86 boot reserves
    // 0..image_end). The hilogd teardown fault walked exactly such an
    // entry (0xb9 = P|PWT|PCD|A|PS). A hit here means the entry was
    // corrupt while the tree was LIVE (this frame passed the ledger's
    // ownership takes) — the producer wrote it before death; the
    // install-side validator names the writer when it happens live.
    // Gated by rux:mmforensics; deferred printk (lock-safe post
    // e1a2a5b1a); zero cost when not armed.
    if crate::trace::EVENT_MMFORENSICS.armed() {
        let mut bad: u64 = 0;
        let mut first: u64 = 0;
        if let Some(t) = PtTable::from_phys(phys_addr) {
            for i in 0..512usize {
                let e = t.get_raw(i);
                if (e & 0x1) != 0 && (e & 0x000F_FFFF_FFFF_F000) == 0 {
                    bad += 1;
                    if first == 0 {
                        first = e;
                    }
                }
            }
        }
        if bad > 0 {
            crate::pr_err!(
                "PT-CANARY: table {:#x} (site {} root {:#x} pid {}) has {} flags-only entr(ies), first={:#x}",
                phys_addr,
                site,
                root_ppn,
                crate::sched::get_current_pid(),
                bad,
                first
            );
        }
    }
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
fn stamp_kernel_tree_boot() {
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
    let root = PtTable::static_root();
    for vpn4 in 0..512usize {
        let pte4 = root.get(vpn4);
        if !pte4.is_valid() || pte4.is_leaf() {
            continue;
        }
        let pud_phys = pte4.ppn() << PAGE_SHIFT;
        if !phys_valid(pud_phys as usize) || pud_phys == root_phys {
            continue;
        }
        PT_LEDGER.stamp_boot(pte4.ppn());
        let Some(pud) = PtTable::from_phys(pud_phys) else { continue };
        for vpn3 in 0..512usize {
            let pte3 = pud.get(vpn3);
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
fn free_page_table(phys_addr: u64) {
    if get_alloc_stage() != AllocStage::Late {
        return;
    }

    // Check if it's from the early static pool (never freed, only reused
    // during Early boot — by Late stage the pool is dormant).
    // SAFETY: KERNEL_MAP is set before any Late-stage free can run;
    // reading the pool's address is inert.
    let offset = unsafe { KERNEL_MAP.va_kernel_pa_offset } as u64;
    let pool_start = (&raw const EARLY_TABLES as u64).wrapping_sub(offset);
    let pool_end = pool_start + (NUM_EARLY_TABLES * PAGE_SIZE as usize) as u64;

    if phys_addr >= pool_start && phys_addr < pool_end {
        // Don't free early page tables
        return;
    }

    // Use zone allocator to free
    crate::mm::page_alloc::free_pages(phys_addr as usize, 0);
}

// ============================================================================
// Deferred page-table free (fork-frag lane, 2026-11-17) — the
// tlb_remove_table shape, Rux edition
// ============================================================================

/// Why this exists: `unmap_pages`/`munmap` historically cleared leaf PTEs
/// but NEVER freed emptied leaf tables — the fragmentation source behind
/// the pbench p4 shape (36 linked leaf tables vs 61 present PTEs per
/// fork; every fork allocated 36 child tables and every teardown walked
/// all 36). Freeing a leaf table of a LIVE mm is the one table operation
/// that needs more than the PTE lock:
///
/// 1. HARDWARE: a remote CPU that ever loaded this mm may hold a stale
///    paging-structure-cache entry pointing at the table. The
///    fire-and-forget `flush_tlb_others_mm` IPI does not prove the remote
///    flush completed, so the frame must not reach the allocator until
///    every target CPU's TLB_FLUSH_EPOCH advanced past a snapshot taken
///    AFTER the PMD entry was cleared (any epoch advance implies a full
///    non-global flush on that CPU — the remote handler flushes
///    everything, not just one range).
/// 2. KERNEL WALKERS: a lock-free PMD read (fork's COW walk, teardown)
///    could have read the still-valid PMD entry and then dereference the
///    freed table. Fixed by re-reading the PMD entry INSIDE the per-table
///    PTE-lock chunk in both walkers (see copy_page_table_cow /
///    free_user_page_tables) — a table that gets unlinked mid-walk turns
///    into "entry invalid, skip", never a dangling dereference.
///
/// Protocol (munmap side): under the mm PTE lock — clear the table's
/// PTEs, verify ALL 512 entries are zero, then
/// `pt_free_emptied_table` (capacity check → PMD-entry clear → epoch
/// snapshot → queue). After the lock: munmap's existing shootdown pair
/// sends the IPIs that advance the target epochs. A later drain (munmap
/// entry/exit, fork, exit_mm) frees every entry whose targets all
/// advanced.
///
/// Overflow policy: the list is bounded. A munmap that empties more
/// tables than there are free slots simply leaves the excess tables
/// linked (yesterday's behavior — fragmentation, never corruption), and
/// the next drain frees capacity for the following munmap.
pub struct PtPendingFree {
    /// Physical address of the unlinked leaf-table frame.
    pub phys: u64,
    /// Remote CPUs that must complete a flush before the free (bitmask).
    pub targets: u64,
    /// TLB_FLUSH_EPOCH snapshot per CPU, taken at commit (AFTER the PMD
    /// entry was cleared).
    pub snaps: [u64; crate::config::MAX_CPUS],
}

const PT_PENDING_CAP: usize = 96;

static PT_PENDING: crate::sync::spinlock::Spinlock<[Option<PtPendingFree>; PT_PENDING_CAP]> =
    crate::sync::spinlock::Spinlock::new([const { None }; PT_PENDING_CAP]);

/// Unlink and defer-free ONE emptied leaf table. Runs under the caller's
/// mm PTE lock (INV-LOCK-7), so the PMD entry cannot race a same-mm
/// writer; lock order PTE → PT_PENDING is one-way (PT_PENDING is never
/// held across a PTE lock). Steps, in the order the protocol needs:
/// capacity check → PMD-entry clear → epoch snapshot → queue. Returns
/// false when the list is full (the table stays linked — fragmentation,
/// never corruption).
///
/// `tlb_mask` is the mm's tlb_cpu_mask (grow-only set of CPUs that ever
/// loaded this root): exactly the CPUs whose hardware could hold stale
/// paging-structure-cache entries for this tree, minus self (self
/// flushes locally before the syscall returns).
///
/// SAFETY: `pmd_table` is the live PMD table of the caller's mm;
/// `table_phys` is the leaf-table frame its `vpn2` entry names.
pub fn pt_free_emptied_table(
    pmd_table: PtTable,
    vpn2: usize,
    table_phys: u64,
    tlb_mask: u64,
) -> bool {
    // Early/Fixmap stages have no per-mm table ownership semantics (the
    // early pool is reused, boot tables are shared) — only Late-stage
    // frames ever enter the deferred-free protocol.
    if !alloc_stage_is_late() {
        return false;
    }
    // Boot-shared tables (kernel-root subtree frames cloned into user
    // mms) are never this mm's to free — keep them linked.
    if PT_LEDGER.is_boot(table_phys >> PAGE_SHIFT) {
        return false;
    }
    let targets = crate::arch::ipi::tlb_remote_targets(tlb_mask);
    let mut list = PT_PENDING.lock();
    let mut free_slot = None;
    for (i, slot) in list.iter().enumerate() {
        if slot.is_none() {
            free_slot = Some(i);
            break;
        }
    }
    let Some(idx) = free_slot else { return false };
    // 1. Unlink the table: hardware walks re-read the PMD entry from
    //    memory and fault-allocate a fresh table; kernel walkers
    //    re-read the entry under the PTE lock (fork/teardown re-read
    //    fix) and skip.
    pmd_table.set(vpn2, PageTableEntry::from_bits(0));
    // 2. Snapshot epochs AFTER the clear (protocol above).
    let snaps = crate::arch::ipi::tlb_epoch_snapshot();
    list[idx] = Some(PtPendingFree {
        phys: table_phys,
        targets,
        snaps,
    });
    true
}

/// Free every pending entry whose target CPUs have all completed a
/// post-snapshot TLB flush. Bounded by PT_PENDING_CAP; called from
/// munmap entry/exit, fork's mm phase, and exit_mm — the natural
/// alloc/free churn points of page tables.
pub fn pt_pending_drain() {
    let mut list = PT_PENDING.lock();
    for slot in list.iter_mut() {
        let Some(e) = slot.as_ref() else { continue };
        if !crate::arch::ipi::tlb_epochs_advanced(e.targets, &e.snaps) {
            continue;
        }
        let phys = e.phys;
        *slot = None; // take the slot before the free (the free path never
        // re-enters this list; keeping the slot clear even if the checked
        // free prints keeps the list walk-consistent).
        // phys names a Late-stage alloc_page_table frame that was unlinked
        // from its (live) mm under the mm PTE lock and waited out its
        // target-CPU flush epochs; no tree references it.
        free_page_table_checked(phys, "munmap");
    }
}

/// Number of entries currently pending (DEBUG/observability).
pub fn pt_pending_count() -> usize {
    let list = PT_PENDING.lock();
    list.iter().filter(|s| s.is_some()).count()
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
    // QUARANTINE bitset (mmforensics lane, 2026-11-15): set once, never
    // cleared, on any double-tracking guard firing for the frame (raw
    // free of a live-stamped frame, cache-side live-PT release refusal,
    // phantom zero-put leak, ledger double-free/double-alloc). A frame
    // that was ever tracked by two owners may keep unreclaimable stale
    // references (an immortal page-cache entry, surviving PCACHE PTEs,
    // foreign tree links); letting it re-enter the allocator re-wires
    // those references into every new owner — the circulation engine
    // behind the RAW-FREE echo family (capture evidence: pfn 0x6bd1
    // re-stamped by five different trees while the cache kept releasing
    // it). Every free path refuses poisoned frames: the poisoned frame
    // becomes an inert, bounded (<= a few/boot) orphan instead.
    poisoned: [AtomicU64; MAX_PAGES / 64],
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
            poisoned: [const { AtomicU64::new(0) }; MAX_PAGES / 64],
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
            // Double-tracked frame: quarantine so no free path can ever
            // re-expose it to the allocator.
            Self::poison_table(ppn);
        }
        // FORENSIC (fake-OOM family): alloc-side ring — pairs with
        // PT_FREE_RING to replay a frame's full table-incarnation history.
        // mmforensics (2026-11-15, pcachewedge §6 residual): the stamp
        // side now also records WHICH tree stamped the frame (the
        // stamping context's root) — at a RAW-FREE echo this names the
        // frame's live owner, the half of the double-track the free-side
        // rings cannot see. Root fetch only when the switch is armed:
        // stamp() runs on every table alloc, the fetch stays off the
        // default hot path.
        {
            let idx = PT_STAMP_CURSOR.fetch_add(1, Relaxed) % PT_STAMP_RING.len();
            PT_STAMP_RING[idx].ppn.store(ppn, Relaxed);
            PT_STAMP_RING[idx].pid.store(pid, Relaxed);
            PT_STAMP_RING[idx].site.store(b'S' as u64, Relaxed);
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
    /// Returns true if the frame had a live stamp (a legitimate first
    /// free) and was cleared; false for double/foreign frees — the caller
    /// must then REFUSE to return the frame to the zone. A table frame
    /// entering the free list twice is what wires one physical page into
    /// two live page-table trees (the shared-pgd corruption family).
    fn take(&self, ppn: u64, pid: u32, site: &str, root_ppn: u64) -> bool {
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
                PT_FREE_RING[idx].root.store(root_ppn, Relaxed);
            }
            // EARLY-TEARDOWN WITNESS: a registered fork-mm root is being
            // freed. If the freeing context is NOT the registered owner and
            // the owner task is still alive, another mm shared this root.
            // The registry entry is CONSUMED on the first root free of this
            // frame — the frame returns to the allocator and its next
            // incarnation as a fork root gets a fresh registration;
            // leaving the stale entry made every recycled root a
            // false-positive witness (exitwalk probe, seconds in).
            if site == "root" {
                for k in 0..FORK_ROOTS.len() {
                    let r = FORK_ROOTS[k].root_ppn.load(Relaxed);
                    if r == ppn && r != 0 {
                        let owner = FORK_ROOTS[k].owner_pid.load(Relaxed);
                        if owner != pid && task_pid_alive(owner) {
                            static EARLY_REPORTS: AtomicUsize = AtomicUsize::new(0);
                            if EARLY_REPORTS.fetch_add(1, Relaxed) < 8 {
                                crate::pr_debug!(
                                    "EARLY-ROOT-FREE root={:#x} owner pid={} ALIVE, freed in pid={} context",
                                    ppn, owner, pid
                                );
                            }
                        }
                        FORK_ROOTS[k].root_ppn.store(0, Relaxed);
                        break;
                    }
                }
            }
            return true;
        }
        if self.reported.fetch_add(1, Relaxed) < 8 {
            crate::pr_err!(
                "PTLEDGER: DOUBLE-FREE ppn={:#x} at {} by pid={} REFUSED (prev-free pid={} root={:#x}, boot={})",
                ppn, site, pid, pt_free_ring_prev_free_pid(ppn),
                pt_free_ring_prev_free_root(ppn), self.is_boot(ppn)
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
        // mmforensics: deep capture at the double/foreign free refusal
        // (own report cap + switch gate; no locks needed here — the
        // refusal path runs before any frame-state transition).
        crate::debug::mmforensics::capture_frame(ppn as usize, "ptledger-double-free");
        // Double-tracked frame: quarantine.
        Self::poison_table(ppn);
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
    /// Root of the tree the frame was freed from (sharing-pair forensics)
    pub root: AtomicU64,
}
impl PtFreeEntry {
    const fn new() -> Self {
        Self {
            ppn: AtomicU64::new(0),
            pid: AtomicU32::new(0),
            site: AtomicU64::new(0),
            root: AtomicU64::new(0),
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
            // ZOMBIE/DEAD tasks have already dropped their mm at do_exit —
            // they stay in the pid hash until reaped but are NOT live
            // owners. Counting them made the EARLY-ROOT-FREE witness fire
            // on recycled root frames (stale registry entry + unreaped
            // zombie owner) — the exitwalk probe tripped it within seconds.
            let st = (*t).state();
            if !st.is_dead() && !st.contains(crate::process::task::TaskState::ZOMBIE) {
                alive.store(true, Ordering::Relaxed);
            }
        }
    });
    alive.load(Ordering::Relaxed)
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

    /// Quarantine a frame (see the `poisoned` field doc). One-way.
    pub fn poison_table(ppn: u64) {
        match Self::bit_index(ppn) {
            Some(i) => {
                PT_LEDGER.poisoned[i / 64].fetch_or(1u64 << (i % 64), Ordering::Relaxed);
            }
            None => {}
        }
    }

    /// Is this frame quarantined (ever double-tracked)? Every free path
    /// refuses such frames; cache pins refuse to serve them.
    pub fn is_poisoned(ppn: u64) -> bool {
        match Self::bit_index(ppn) {
            Some(i) => {
                PT_LEDGER.poisoned[i / 64].load(Ordering::Relaxed) & (1u64 << (i % 64)) != 0
            }
            None => false,
        }
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

/// Root of the tree that last successfully freed this frame (0 = unknown).
fn pt_free_ring_prev_free_root(ppn: u64) -> u64 {
    use core::sync::atomic::Ordering::Relaxed;
    let cur = PT_FREE_CURSOR.load(Relaxed);
    for k in 0..PT_FREE_RING.len() {
        let idx = (cur + PT_FREE_RING.len() - 1 - k) % PT_FREE_RING.len();
        if PT_FREE_RING[idx].ppn.load(Relaxed) == ppn {
            return PT_FREE_RING[idx].root.load(Relaxed);
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
    // copied PTE under TCG. Behind the rux:mmforensics (retired debug=mmforensics) runtime switch.
    if !crate::trace::EVENT_MMFORENSICS.armed() {
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
///
/// `pte` is the PTE section the teardown chunks take (INV-LOCK-7): the
/// DYING mm's own PteLock from MmStruct::drop (uncontended by
/// construction — last-Arc + synchronize_rcu drain), or
/// FRESH_TREE_PTE_LOCK for unpublished trees (exec error path, tests).
pub fn free_user_page_tables(
    pte: &crate::mm::mm_struct::PteLock,
    root_ppn: u64,
    vma_ranges: &[(u64, u64)],
) {
    use crate::mm::{page_desc, phys_to_pfn, phys_valid, free_pages};
    crate::debug::memwatch::FUT_CALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // hp89 hunt: dying-mm teardown is a whole-domain PTE-clearing pass
    // (the fork-child-exit face) — record it for post-hoc #GP attribution.
    crate::mm::unwatch::note(
        crate::mm::unwatch::KIND_MM_TEARDOWN,
        root_ppn,
        0,
        0,
    );
    // Double-teardown detector: the same root walked twice means two
    // MmStructs ended up owning one page-table tree — the second walk
    // chases recycled frames full of foreign data. Behind the
    // rux:mmforensics (retired debug=mmforensics) runtime switch (see pte_install_log).
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
                    // debug-migrate: the double-teardown hit also records
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
    // FORENSIC: stale-tree census. Every intermediate table frame this walk
    // reaches must carry a LIVE ledger stamp (its allocation incarnation).
    // A missing stamp means the frame was freed earlier and this tree has
    // been pointing at recycled memory — the walk is about to put_page/
    // free whatever now lives there. Collected here (with indices),
    // printed once per offending teardown.
    let mut stale_tables: [(u8, usize, usize, u64); 16] = [(0, 0, 0, 0); 16];
    let mut stale_count = 0usize;
    // CHUNKED teardown (launcher wall fix, session-3; per-mm sections,
    // INV-LOCK-7): the walk no longer holds one global section for the
    // whole tree — that monopolized a CPU (preempt disabled) for the
    // multi-second free walk of a large mm. Each leaf table (and each
    // huge-leaf entry, and each upper-table free) is processed in its own
    // short section chunk (`pte`, the dying mm's own PteLock) with a
    // cond_resched() boundary between chunks. Exclusion:
    // - this tree has no other users: Drop runs at the LAST Arc, every
    //   fork/rmap/compact walker pins the mm by reference, and raw
    //   borrowers were drained by the synchronize_rcu in MmStruct::drop;
    // - the NEW2 reuse race (a freed data/table frame immediately
    //   reallocated as a page table or COW copy by another CPU) is a
    //   per-frame race: each frame's put_page/free happens entirely
    //   inside the chunk that processes its PTE — after the free the
    //   teardown never touches the frame again, and the buddy heap lock
    //   serializes free vs alloc. (Pre-split, the closure also leaned on
    //   every allocator-side taker taking the same GLOBAL lock; with
    //   per-mm sections a fault of ANOTHER mm may allocate the freed
    //   frame concurrently — safe because the frame's last teardown
    //   access is the free itself.)

    let root_phys = root_ppn << PAGE_SHIFT;
    // A dying-mm root that fails the stale-frame guard is corruption (the
    // tree outlived its frames) — free the root frame itself and return,
    // exactly the containment the inline guard gave the raw walk.
    let Some(root_table) = PtTable::from_phys(root_phys) else {
        free_page_table_checked_rooted(root_phys, "root", root_ppn);
        return;
    };

    // Walk and free all levels of the user half (PML4[0..256])
    for vpn4 in 0..USER_PTRS_PER_PGD {
        let pte4 = root_table.get(vpn4);
        if !pte4.is_valid() {
            continue;
        }

        // PML4 entries are never leaves on x86_64 (PS at PML4 is reserved).

        let ppn3 = pte4.ppn();
        let table3_phys = ppn3 << PAGE_SHIFT;

        if table3_phys == 0 || !phys_valid(table3_phys as usize) {
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

        let Some(table3) = PtTable::from_phys(table3_phys) else { continue };

        for vpn3 in 0..512 {
            let pte3 = table3.get(vpn3);
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
                if phys_addr == 0 || !phys_valid(phys_addr as usize) {
                    continue;
                }
                // Foreign frame (kernel-heap GEM/fb backing mapped into
                // userspace): not this mm's to put/free — see
                // page_alloc::is_foreign_user_frame. Skip rmap/put/free.
                if crate::mm::page_alloc::is_foreign_user_frame(phys_addr as usize) {
                    continue;
                }
                // Chunk: 1GB-leaf frame return under the PTE lock (NEW2
                // reuse race — see the chunked-teardown comment above).
                {
                    let _pte_guard = pte.lock();
                    let pfn = phys_to_pfn(phys_addr as usize);
                    if let Some(page) = page_desc(pfn) {
                        if page.is_mapped() {
                            crate::mm::rmap::page_remove_rmap(page);
                        }
                        let new_ref = page.put_page_site(crate::debug::driftwatch::S_TEARDOWN_X86_L3);
                        if new_ref == 0 {
                            // Cache-backed leaf: leak, never hand the live
                            // cache frame to the zone (pcachewedge lane).
                            if pte3.bits() & super::mm_ops::cow_flags::PCACHE != 0 {
                                crate::mm::page_alloc::pcache_leaf_phantom_zero_put(
                                    phys_addr as usize,
                                    "teardown-l3",
                                );
                            } else {
                                crate::debug::memwatch::FUT_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                                free_pages(phys_addr as usize, 0);
                            }
                        }
                    }
                }
                continue;
            }

            let ppn2 = pte3.ppn();
            let table2_phys = ppn2 << PAGE_SHIFT;

            if table2_phys == 0 || !phys_valid(table2_phys as usize) {
                continue;
            }

            let Some(table2) = PtTable::from_phys(table2_phys) else { continue };

            for vpn2 in 0..512 {
                let pte2 = table2.get(vpn2);
                if !pte2.is_valid() {
                    continue;
                }

                // ONE-LEAF-TABLE CHUNK (see the chunked-teardown comment
                // at the top of the walk): the PTE lock covers this table's
                // zaps and its frame return; the cond_resched() at the
                // bottom yields the CPU between chunks when need_resched is
                // set — the whole-teardown CPU monopoly is gone.
                {
                let _pte_guard = pte.lock();

                // (fork-frag lane) RE-READ the PMD entry under the chunk's
                // lock — pte2 above was read without it, and munmap now
                // unlinks emptied leaf tables on live mms (teardown and a
                // racing same-mm munmap can overlap only through foreign
                // last-Arc drop contexts; the re-read makes the walk
                // structurally safe regardless — an unlinked table reads
                // as "entry invalid", never a dangling frame).
                let pte2 = table2.get(vpn2);
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
                    if phys_addr == 0 || !phys_valid(phys_addr as usize) {
                        continue;
                    }
                    // Foreign frame (kernel-heap GEM/fb backing mapped into
                    // userspace): not this mm's to put/free — see
                    // page_alloc::is_foreign_user_frame. Skip rmap/put/free.
                    if crate::mm::page_alloc::is_foreign_user_frame(phys_addr as usize) {
                        continue;
                    }
                    let pfn = phys_to_pfn(phys_addr as usize);
                    if let Some(page) = page_desc(pfn) {
                        if page.is_mapped() {
                            crate::mm::rmap::page_remove_rmap(page);
                        }
                        let new_ref = page.put_page_site(crate::debug::driftwatch::S_TEARDOWN_X86_L2);
                        if new_ref == 0 {
                            // Cache-backed leaf: leak, never hand the live
                            // cache frame to the zone (pcachewedge lane).
                            if pte2.bits() & super::mm_ops::cow_flags::PCACHE != 0 {
                                crate::mm::page_alloc::pcache_leaf_phantom_zero_put(
                                    phys_addr as usize,
                                    "teardown-l2",
                                );
                            } else {
                                crate::debug::memwatch::FUT_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                                free_pages(phys_addr as usize, 0);
                            }
                        }
                    }
                    continue;
                }

                let ppn1 = pte2.ppn();
                let table1_phys = ppn1 << PAGE_SHIFT;

                if table1_phys == 0 || !phys_valid(table1_phys as usize) {
                    continue;
                }

                let Some(table1) = PtTable::from_phys(table1_phys) else { continue };

                // (sysmm-2x lane) VMA-RANGE-BOUNDED leaf scan: by the
                // mapping invariant every mapped PTE lives inside some
                // VMA, so only the PTE windows the caller's VMA snapshot
                // covers can hold present entries. A fragmented mm (36
                // leaf tables, 61 present PTEs after munmap-heavy
                // phases) paid 512 streamed loads per table here — the
                // whole 18 µs drop_ptfree phase of the SMP-8 fork-exit
                // decomposition (forkprof=2, 2026-11-17). Windows are
                // computed per table from the sorted, page-aligned VMA
                // ranges; a table with NO covering VMA (or an empty
                // range list, or >8 intersecting ranges) keeps the full
                // defensive scan so orphan PTEs (invariant break) are
                // still found and freed.
                let mut scan_windows: [(usize, usize); 8] = [(0, 0); 8];
                let mut n_windows = 0usize;
                {
                    let leaf_base = (vpn4 as u64) << 39 | (vpn3 as u64) << 30
                        | (vpn2 as u64) << 21;
                    let leaf_end = leaf_base + (1u64 << 21);
                    for &(rs, re) in vma_ranges {
                        let s = rs.max(leaf_base);
                        let e = re.min(leaf_end);
                        if e <= s {
                            continue;
                        }
                        if n_windows == scan_windows.len() {
                            n_windows = 0; // too fragmented: full scan
                            break;
                        }
                        scan_windows[n_windows] =
                            (((s - leaf_base) >> 12) as usize, ((e - leaf_base) >> 12) as usize);
                        n_windows += 1;
                    }
                    if n_windows == 0 {
                        scan_windows[0] = (0, 512);
                        n_windows = 1;
                    }
                }
                for &(win_lo, win_hi) in &scan_windows[..n_windows] {
                for vpn1 in win_lo..win_hi {
                    let pte1 = table1.get(vpn1);

                    // Swap entry (P=0 leaf with the swap signature): the
                    // page lives on the swap device — free the slot.
                    if !pte1.is_valid() && crate::mm::mm_hooks::is_swap_entry(pte1.bits()) {
                        crate::mm::mm_hooks::swap_free_slot(
                            crate::mm::mm_hooks::swap_entry_type(pte1.bits()),
                            crate::mm::mm_hooks::swap_entry_offset(pte1.bits()),
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

                    if !phys_valid(phys_addr as usize) {
                        continue;
                    }
                    let Some(page) = page_desc(pfn) else { continue };

                    // Foreign frame (kernel-heap GEM/fb backing mapped into
                    // userspace): not this mm's to put/free — see
                    // page_alloc::is_foreign_user_frame. Skip rmap/put/free
                    // and let the owning subsystem (GemObject::drop ->
                    // heap dealloc) release the memory.
                    if crate::mm::page_alloc::is_foreign_user_frame(phys_addr as usize) {
                        continue;
                    }

                    if page.is_mapped() {
                        crate::mm::rmap::page_remove_rmap(page);
                    }

                    // RO alias (cow_flags::PCACHE_RO): mapcount-only leaf —
                    // no descriptor reference and no cache pin to drop; the
                    // page_remove_rmap above (mapcount dec) is its whole
                    // teardown.
                    if pte1.bits() & super::mm_ops::cow_flags::PCACHE_RO == 0 {
                        let new_ref = page.put_page_site(crate::debug::driftwatch::S_TEARDOWN_X86_L1);
                        if new_ref == 0 {
                            // Cache-backed leaf: leak, never hand the live
                            // cache frame to the zone (pcachewedge lane).
                            if pte1.bits() & super::mm_ops::cow_flags::PCACHE != 0 {
                                crate::mm::page_alloc::pcache_leaf_phantom_zero_put(
                                    phys_addr as usize,
                                    "teardown-l1",
                                );
                            } else {
                                crate::debug::memwatch::FUT_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                                free_pages(phys_addr as usize, 0);
                            }
                        }
                        // PCACHE leaf (filemap_fault private mapping): drop the
                        // mapping's page-cache pin after the descriptor put
                        // (same ordering as unmap_pages; the pin's release may
                        // free an invalidated frame).
                        if pte1.bits() & super::mm_ops::cow_flags::PCACHE != 0 {
                            super::mm_ops::release_cache_mapping_pin(pfn);
                        }
                    }
                }
                } // (sysmm-2x) end VMA-range-bounded leaf scan
                // FORENSIC: census the PT table before freeing it.
                if get_alloc_stage() == AllocStage::Late && !PT_LEDGER.is_boot(ppn1) {
                    let live = PT_LEDGER.peek(ppn1) != 0;
                    if !live {
                        if stale_count < stale_tables.len() {
                            stale_tables[stale_count] = (0, vpn4, vpn3 * 512 + vpn2, ppn1);
                        }
                        stale_count += 1;
                        // SCRIBBLE HUNT (x86 text-scribbler): dump the first
                        // leaves of the offending table plus the parent PMD
                        // slot bits — identifies whether the frame is a live
                        // kernel-image page (scribble engine) or a benign
                        // boot-era table, and which VA range it maps.
                        static STALE_DUMPS: AtomicUsize = AtomicUsize::new(0);
                        if STALE_DUMPS.fetch_add(1, Ordering::Relaxed) < 4 {
                            let va_base = (vpn4 as u64) << 39 | (vpn3 as u64) << 30
                                | (vpn2 as u64) << 21;
                            crate::pr_err!(
                                "  STALE-DUMP pmd_slot raw={:#x} valid={} leaf={} user={} ppn={:#x} pt_phys={:#x} va_base={:#x}",
                                pte2.bits(), pte2.is_valid(), pte2.is_leaf(),
                                pte2.is_user(), pte2.ppn(), table1_phys, va_base
                            );
                            for nk in [vpn2.wrapping_sub(1), vpn2 + 1] {
                                if nk < 512 {
                                    let ne = table2.get(nk);
                                    if ne.is_valid() {
                                        crate::pr_err!(
                                            "  STALE-DUMP pmd[{}] raw={:#x} leaf={} ppn={:#x}",
                                            nk, ne.bits(), ne.is_leaf(), ne.ppn()
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                free_page_table_checked_rooted(table1_phys, "l0", root_ppn);
                } // end one-leaf-table chunk (PTE lock released)

                // Chunk boundary: no lock held; cond_resched() no-ops unless
                // fully preemptible (task context, no bh/rcu section).
                crate::sched::cond_resched();
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
            // Chunk: PMD (l1) frame return under the PTE lock.
            {
                let _pte_guard = pte.lock();
                free_page_table_checked_rooted(table2_phys, "l1", root_ppn);
            }
            crate::sched::cond_resched();
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
        // Chunk: PUD (l2) frame return under the PTE lock.
        {
            let _pte_guard = pte.lock();
            free_page_table_checked_rooted(table3_phys, "l2", root_ppn);
        }
        crate::sched::cond_resched();
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

    // mmforensics: deep capture naming the first stale table frame (the
    // stale-tree census witness — bounded reverse scan answers who else
    // still references it).
    if stale_count > 0 {
        crate::debug::mmforensics::capture_frame(stale_tables[0].3 as usize, "fut-stale-table");
    }

    // Free root table (PML4)
    free_page_table_checked_rooted(root_phys, "root", root_ppn);
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
/// SCRIBBLE GUARD: a table-link PTE parsed out of a scribbled slot can
/// carry an arbitrary ppn (we saw .text bytes parsed as links — the x86
/// text-scribbler self-propagation). Every walk that FOLLOWS a link must
/// validate the target lies inside physical RAM before touching it; a
/// bad link is reported and the walk refuses to continue (the mapping
/// fails loudly instead of writing a "page table" over kernel memory).
#[inline]
fn valid_table_link_ppn(ppn: u64, root_ppn: u64) -> bool {
    // Runtime RAM top (discovered memory map — fake-OOM fix: the old
    // compile-time MAX_PFN rejected every table link above the Kernel.toml
    // nominal 2 GiB, breaking user page tables on larger machines).
    let ram_top = (crate::mm::page_desc::max_pfn() as u64) << 12;
    let phys = ppn << 12;
    if phys >= ram_top {
        return false;
    }
    // USER-tree links must also stay above the kernel-image reserve: their
    // tables come from the zone (floor ~1GB), never from the Early BSS
    // pool or the image itself. A user-tree link below 0x1E00000 is a
    // scribbled slot parsing as a pointer — following it would read/write
    // the kernel image (the x86 scribble engine: PMD[6]=0x300065 steering
    // writes into .text). The KERNEL root's own tables legitimately live
    // in the image (BSS pool) and are exempt.
    root_ppn == unsafe { root_page_table_ppn() } || phys >= 0x1E0_0000
}

/// Rate-limited one-liner naming the walk level whose page-table
/// allocation failed (fake-OOM family). The loud allocator-state dump
/// lives in alloc_page_table (oom_forensic_dump, itself rate-limited);
/// this only marks the failure depth in the serial log.
fn pt_alloc_fail(level: &str, va: u64) {
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
/// (page-table allocation failure — the caller decides the failure
/// policy: user demand faults convert this to MmFaultResult::OutOfMemory,
/// i.e. SIGKILL/OOM-kill, instead of the old kernel panic; see the
/// fake-OOM family notes in alloc_page_table).
///
/// No-TLB-flush contract (fault-around batches): callers installing
/// leaves into slots that were NOT PRESENT (verified by their own walk
/// immediately before, under the PTE lock) need no invlpg — x86 never
/// caches non-present translations, and every teardown path
/// (munmap/mprotect) flushes what it zaps. Callers REPLACING a present
/// leaf must flush that VA themselves.
pub fn map_page_noflush(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    // 0xb9 install validator (oh-launch round 7) — see write_pte_raw's
    // twin: a PRESENT entry with zero address bits is corrupt by
    // construction; name the installer the moment it is written. Gated
    // by rux:mmforensics; deferred printk; one load when off.
    if crate::trace::EVENT_MMFORENSICS.armed()
        && (flags & 0x1) != 0
        && ((flags | phys.bits()) & 0x000F_FFFF_FFFF_F000) == 0
    {
        static BADMN_N: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if BADMN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16 {
            crate::pr_err!(
                "PT-BAD-INSTALL: map_page_noflush flags={:#x} phys={:#x} va={:#x} root={:#x} pid {}",
                flags,
                phys.bits(),
                virt.bits(),
                root_ppn,
                crate::sched::get_current_pid()
            );
        }
    }
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    let a = VirtAddr::new(virt_addr);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    let Some(root) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        pt_alloc_fail("ROOT", virt_addr);
        return false;
    };

    // x86 checks U/S at EVERY walk level: a user mapping (flags carry US)
    // must reach the leaf through links that all carry US, or a CPL3
    // access faults with P=1 (protection) even though the leaf itself is
    // user-accessible. `link_user` is OR'd into every link this walk
    // creates and upgrades pre-existing user-half links that lack it
    // (e.g. the supervisor-only links create_user_address_space /
    // earlier kernel-side installs left behind). Kernel mappings pass
    // link_user=0 and never widen anything.
    let link_user = flags & PageTableEntry::US;

    // PML4 -> PUD (PML4 leaves do not exist on x86_64)
    let pte4 = root.get(vpn4);
    let ppn3 = if pte4.is_valid() {
        if link_user != 0 && pte4.bits() & PageTableEntry::US == 0 {
            root.set(
                vpn4,
                PageTableEntry::from_bits(pte4.bits() | PageTableEntry::US),
            );
        }
        pte4.ppn()
    } else {
        let Some(table_phys) = alloc_page_table() else {
            pt_alloc_fail("PUD", virt_addr);
            return false;
        };
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(
            vpn4,
            PageTableEntry::from_bits(PageTableEntry::new_table(ppn).bits() | link_user),
        );
        ppn
    };

    // PUD -> PMD
    if !valid_table_link_ppn(ppn3, root_ppn) {
        crate::pr_err!(
            "SCRIBBLE-GUARD map_page: PUD link ppn={:#x} bad at va {:#x} — refusing",
            ppn3, virt_addr
        );
        return false;
    }
    let Some(table3_ref) = PtTable::from_phys(ppn3 << PAGE_SHIFT) else {
        pt_alloc_fail("PUD", virt_addr);
        return false;
    };
    let mut pte3 = table3_ref.get(vpn3);
    // 1GB leaf demotion: install a PMD table whose 512 entries reproduce
    // the 1GB mapping as 2MB leaves (same phys base, same flags + PS).
    if pte3.is_valid() && pte3.is_leaf() {
        if let Some(pmd_phys) = alloc_page_table() {
            let Some(pmd) = PtTable::from_phys(pmd_phys) else {
                return false;
            };
            let base = pte3.phys_addr(); // 1GB-aligned
            let flag_bits = pte3.bits() & !PageTableEntry::PHYS_MASK_PUBLIC;
            for k in 0..512u64 {
                pmd.set(
                    k as usize,
                    PageTableEntry::from_bits((base + (k << PMD_SHIFT)) | flag_bits),
                );
            }
            // Single-word swap: a concurrent walker sees either the leaf or
            // a fully populated table — both translate the whole 1GB
            // identically. The caller's page is overwritten afterwards; the
            // flush in map_page / region batch publishes both writes.
            let pmd_ppn = pmd_phys >> PAGE_SHIFT;
            table3_ref.set(
                vpn3,
                PageTableEntry::from_bits(PageTableEntry::new_table(pmd_ppn).bits() | link_user),
            );
            pte3 = table3_ref.get(vpn3);
        } else {
            crate::pr_err!(
                "map_page: PMD table alloc failed demoting 1GB leaf at {:#x}",
                virt_addr
            );
            return false;
        }
    }
    let ppn2 = if pte3.is_valid() {
        if link_user != 0 && pte3.bits() & PageTableEntry::US == 0 {
            table3_ref.set(
                vpn3,
                PageTableEntry::from_bits(pte3.bits() | PageTableEntry::US),
            );
        }
        pte3.ppn()
    } else {
        let Some(table_phys) = alloc_page_table() else {
            pt_alloc_fail("PMD", virt_addr);
            return false;
        };
        let ppn = table_phys >> PAGE_SHIFT;
        table3_ref.set(
            vpn3,
            PageTableEntry::from_bits(PageTableEntry::new_table(ppn).bits() | link_user),
        );
        ppn
    };

    // PMD -> PT
    if !valid_table_link_ppn(ppn2, root_ppn) {
        crate::pr_err!(
            "SCRIBBLE-GUARD map_page: PMD link ppn={:#x} out of RAM at va {:#x} — refusing",
            ppn2, virt_addr
        );
        return false;
    }
    let Some(table2_ref) = PtTable::from_phys(ppn2 << PAGE_SHIFT) else {
        pt_alloc_fail("PMD", virt_addr);
        return false;
    };
    let mut pte2 = table2_ref.get(vpn2);
    // 2MB leaf demotion (E8-MM): install a PT whose 512 entries reproduce
    // the 2MB mapping verbatim (same phys base, same flag bits minus PS).
    if pte2.is_valid() && pte2.is_leaf() {
        if let Some(pt_phys) = alloc_page_table() {
            let Some(pt) = PtTable::from_phys(pt_phys) else {
                return false;
            };
            let base = pte2.phys_addr(); // 2MB-aligned
            let flag_bits = (pte2.bits() & !PageTableEntry::PHYS_MASK_PUBLIC) & !PageTableEntry::PS;
            for k in 0..512u64 {
                pt.set(
                    k as usize,
                    PageTableEntry::from_bits((base + (k << PAGE_SHIFT)) | flag_bits),
                );
            }
            let pt_ppn = pt_phys >> PAGE_SHIFT;
            table2_ref.set(
                vpn2,
                PageTableEntry::from_bits(PageTableEntry::new_table(pt_ppn).bits() | link_user),
            );
            pte2 = table2_ref.get(vpn2);
        } else {
            crate::pr_err!(
                "map_page: PT table alloc failed demoting 2MB leaf at {:#x}",
                virt_addr
            );
            return false;
        }
    }
    let ppn1 = if pte2.is_valid() {
        if link_user != 0 && pte2.bits() & PageTableEntry::US == 0 {
            table2_ref.set(
                vpn2,
                PageTableEntry::from_bits(pte2.bits() | PageTableEntry::US),
            );
        }
        pte2.ppn()
    } else {
        let Some(table_phys) = alloc_page_table() else {
            pt_alloc_fail("PT", virt_addr);
            return false;
        };
        let ppn = table_phys >> PAGE_SHIFT;
        table2_ref.set(
            vpn2,
            PageTableEntry::from_bits(PageTableEntry::new_table(ppn).bits() | link_user),
        );
        ppn
    };

    // PT -> physical page
    if !valid_table_link_ppn(ppn1, root_ppn) {
        crate::pr_err!(
            "SCRIBBLE-GUARD map_page: PT link ppn={:#x} out of RAM at va {:#x} — refusing",
            ppn1, virt_addr
        );
        return false;
    }
    let Some(table1_ref) = PtTable::from_phys(ppn1 << PAGE_SHIFT) else {
        pt_alloc_fail("PT", virt_addr);
        return false;
    };
    let ppn: u64 = phys_addr >> PAGE_SHIFT;
    let pte_bits: u64 = (ppn << PAGE_SHIFT) | flags;

    // SCRIBBLE HUNT: a leaf whose physical frame lies inside the kernel
    // image (below the 30MB reserve) is corruption — the zone starts at
    // ~1GB and post-boot memblock never dips below the reserve. The ONE
    // legitimate low-image alias is the vDSO pair (106th): the two
    // memblock-reserved statics every exec maps into the user tree.
    let is_vdso = crate::mm::vdso::is_vdso_phys(phys_addr);
    if phys_addr < 0x1E0_0000 && root_ppn != root_page_table_ppn() && !is_vdso {
        crate::pr_err!(
            "LEAF-LOW-HIT map_page va={:#x} -> phys={:#x} flags={:#x} root={:#x}",
            virt_addr, phys_addr, flags, root_ppn
        );
    }
    // CORRUPTION TRIPWIRE: overwriting a PRESENT USER leaf with a
    // different frame is the aliasing instant (one mapping's block reads
    // another's data — heapstress blk canary, gnome-shell malloc assert).
    // map_page legitimately installs each user page exactly once (fault
    // path re-checks under PTE_MODIFY_LOCK), so a live-leaf overwrite is
    // always a bug upstream.
    {
        let old = table1_ref.get(vpn1);
        if old.is_valid()
            && old.bits() & PageTableEntry::US != 0
            && old.ppn() != ppn
            && root_ppn != root_page_table_ppn()
        {
            static OW_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if OW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 24 {
                // Name the caller: context marker set at candidate
                // map_page call sites (mm_ops::MM_CTX).
                let ctx = super::mm_ops::MM_CTX.load(core::sync::atomic::Ordering::Relaxed);
                // Data verdict: does the new frame carry the old frame's
                // contents (copy semantics) or is the mapping's data
                // destroyed (fresh zero/garbage)?
                let old_kv = phys_to_virt(PhysAddr::new(old.ppn() << PAGE_SHIFT)).bits()
                    as *const u8;
                let new_kv =
                    phys_to_virt(PhysAddr::new(ppn << PAGE_SHIFT)).bits() as *const u8;
                // SAFETY: both are kernel linear-map addresses of the two
                // frames; read-only compare of 16 bytes each.
                let equal = unsafe {
                    core::slice::from_raw_parts(old_kv, 16)
                        == core::slice::from_raw_parts(new_kv, 16)
                };
                let comm = crate::process::current_task()
                    .map(|t| {
                        core::str::from_utf8(t.comm())
                            .unwrap_or("?")
                            .trim_end_matches('\0')
                    })
                    .unwrap_or("?");
                crate::pr_err!(
                    "PTE-OVERWRITE-LIVE va={:#x} old={:#x} new={:#x} pid={} comm={} ctx={} data_{}",
                    virt_addr,
                    old.bits(),
                    pte_bits,
                    crate::process::current_pid(),
                    comm,
                    ctx,
                    if equal { "COPIED" } else { "DESTROYED" }
                );
                // Attribution: MM_CTX can be stale (it is never reset),
                // so the printed ctx only says what ran RECENTLY, not
                // which path is mapping now. The frame walk names the
                // actual caller — b86 lost render_service to a 2-page
                // heap overwrite at 0x1f000 whose ctx label was useless.
                crate::debug::backtrace::dump_stack();
            }
        }
    }
    table1_ref.set(vpn1, PageTableEntry::from_bits(pte_bits));
    pte_install_log(root_ppn, virt_addr, ppn);
    true
}

/// Map a single 4K page in a page table (with TLB flush of the page).
///
/// Returns false when the mapping could not be installed (page-table
/// allocation failure). User demand-fault callers convert this to
/// MmFaultResult::OutOfMemory (SIGKILL of the faulting task — Linux
/// pagefault-OOM semantics) instead of the former kernel panic.
pub fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    let ok = map_page_noflush(root_ppn, virt, phys, flags);

    // Flush TLB (single-page callers — page faults, mremap, io_uring —
    // need the new translation visible before the retrying access).
    // Unconditional, failure paths included (matching the pre-failible
    // behavior where the flush always ran).
    crate::arch::cpu::invlpg(virt.bits());
    ok
}

/// Install a 2MB PMD leaf mapping into the static kernel root.
///
/// Overwrites whatever PMD entry covers `virt` (a fresh link, an existing
/// leaf — e.g. replacing a cacheable bootstrap leaf with an IO-flagged
/// one). Must only be used for windows mapped once at boot and never
/// re-mapped at 4K granularity later... except map_page CAN demote these
/// leaves per-address-space after the clone (E8-MM).
///
/// Boot-time kernel windows only: a table-allocation failure here leaves
/// the window UNMAPPED with no recovery path, so the .expect panics below
/// are deliberate (unlike map_page, which is failible for the fake-OOM
/// family — user demand faults must OOM-kill, not panic).
fn map_pmd_huge_page(virt: usize, phys: usize, flags: u64) {
    let a = VirtAddr::new(virt as u64);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;

    let root = PtTable::static_root();

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
    let Some(table3_ref) = PtTable::from_phys(table3_phys) else {
        panic!("map_pmd_huge_page: bad PMD table link");
    };
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
    let Some(table2) = PtTable::from_phys(table2_phys) else {
        panic!("map_pmd_huge_page: bad PT table link");
    };
    table2.set(vpn2, PageTableEntry::from_bits(entry_bits));
}

/// Install a 1GB PUD leaf mapping into the static kernel root (x86
/// PDPTE-with-PS). Same contract as map_pmd_huge_page: boot-time windows
/// only. Used for the above-4G device alias (DEVICE_ALIAS64), where 2MB
/// leaves would cost hundreds of PD tables for one PML4 slot.
fn map_pud_huge_page(virt: usize, phys: usize, flags: u64) {
    let a = VirtAddr::new(virt as u64);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;

    let root = PtTable::static_root();

    // PML4 -> PUD
    let pte4 = root.get(vpn4);
    let ppn3 = if pte4.is_valid() {
        pte4.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_pud_huge_page: failed to allocate PUD table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn4, PageTableEntry::new_table(ppn));
        ppn
    };

    // PUD leaf (1GB huge page): bits [51:30] carry the 1GB-aligned
    // physical address; PS (bit 7) marks the leaf. The entry must not
    // already be a table link (that would orphan it).
    assert!(phys % (PUD_SIZE as usize) == 0, "phys must be 1GB aligned for PUD leaf");
    assert!(virt % (PUD_SIZE as usize) == 0, "virt must be 1GB aligned for PUD leaf");

    let entry_bits = ((phys as u64) & PageTableEntry::PHYS_MASK_PUBLIC) | PageTableEntry::PS | flags;
    let table3_phys = ppn3 << PAGE_SHIFT;
    let Some(table3) = PtTable::from_phys(table3_phys) else {
        panic!("map_pud_huge_page: bad PUD table link");
    };
    table3.set(vpn3, PageTableEntry::from_bits(entry_bits));
}

/// Map a kernel virtual page to a physical page WITHOUT flushing the TLB.
///
/// Internal batch primitive — region callers flush once via
/// map_kernel_region. Walks from the CURRENTLY ACTIVE root (CR3): kernel
/// VAs resolve through the shared kernel-half links in any root.
fn map_kernel_page_noflush(virt: u64, phys: u64, flags: u64) -> bool {
    let root_ppn = root_page_table_ppn();
    map_page_noflush(root_ppn, VirtAddr::new(virt), PhysAddr::new(phys), flags)
}

/// Map a kernel virtual page to a physical page.
///
/// Used for vmemmap and other kernel mappings that need 4KB granularity.
/// Returns false when the mapping could not be installed; boot-time
/// callers treat that as fatal (a kernel window that failed to map is
/// unusable), runtime ioremap-class callers should fail their request.
pub fn map_kernel_page(virt: u64, phys: u64, flags: u64) -> bool {
    let ok = map_kernel_page_noflush(virt, phys, flags);
    crate::arch::cpu::invlpg(virt);
    ok
}

/// Map a region of kernel virtual pages to physical pages (VA and PA in
/// lockstep), using 2MB PMD leaves where alignment permits and 4K pages
/// for the head/tail — then flushes the TLB ONCE for the whole region.
pub fn map_kernel_region(virt: u64, phys: u64, size: u64, flags: u64) {
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
pub fn map_kernel_region_huge(virt: u64, size: u64, flags: u64) {
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
    {
        println!("mm: x86_64 4-level page tables (building real PML4)");

        let static_root = PtTable::static_root();
        static_root.zero_entries();

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
        static_root.set(
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
            let pte4 = static_root.get(vpn4);
            if pte4.is_valid() {
                if let Some(pud) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) {
                    if !pud.get(vpn3).is_valid() {
                        if let Some(pmd_phys) = alloc_page_table() {
                            pud.set(
                                vpn3,
                                PageTableEntry::new_table(pmd_phys >> PAGE_SHIFT),
                            );
                        }
                    }
                }
            }
        }

        // Switch to the real tables. The switching code path (this text,
        // the low boot stack) is identically mapped in the bootstrap and
        // the new tables, so the very next instruction after `mov cr3`
        // fetches fine; loading CR3 flushes the non-global TLB.
        let root_phys = root_ppn << PAGE_SHIFT;
        // SAFETY: root_phys is the static root PML4 just built above
        // (image BSS); the switching path is identically mapped.
        unsafe { crate::arch::cpu::write_cr3(root_phys) };

        MMU_INITIALIZED.store(true, Ordering::Release);

        // Record the real image size for /proc-style consumers.
        let image_size = (kernel_image_phys_end() - KERNEL_ENTRY) as usize;
        // SAFETY: single-CPU boot section; no concurrent reader exists.
        unsafe { KERNEL_MAP.size = image_size };

        println!(
            "mm: switched CR3 to real PML4 (phys {:#x}), image {:#x}-{:#x}",
            root_phys,
            kernel_virt,
            kernel_virt + image_size as u64
        );

        // Arm PCID now: CR3 holds the clean (low bits 0) static root.
        // Context switches from here on carry a per-mm PCID + NOFLUSH;
        // every TLB invalidation goes through the INVPCID paths in
        // mm/asid.rs. No-op (stays flush-on-CR3) without CPUID
        // PCID+INVPCID.
        super::asid::enable_pcid();
    }
}

/// Drop the bootstrap identity map (PML4[0] -> boot_pdpt_lo, identity
/// 0..2GB supervisor-only) from the kernel CR3. See boot_stack_continuation
/// for the call site and the run/perf-review-ledger.md for the contract.
pub fn drop_bootstrap_identity() {
    // Exclusive owner of the kernel root table (init() built it); runs
    // once on the BSP after the stack switch, task context.
    PtTable::static_root().set(0, PageTableEntry::new());
    // SAFETY: reloading the static root's own physical address.
    unsafe { crate::arch::cpu::write_cr3(root_static_phys()) };
}

/// Setup linear mapping for physical memory (PAGE_OFFSET region).
///
/// Called from main.rs after memblock is initialized and the memory
/// regions are parsed; CR3 already points at the real PML4. All usable RAM
/// is mapped at PAGE_OFFSET with 2MB leaves where region boundaries align,
/// 4K pages otherwise.
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    {
        // Initialize KERNEL_MAP.va_pa_offset for phys_to_virt/virt_to_phys
        // SAFETY: single-CPU boot section; no concurrent reader exists.
        unsafe { KERNEL_MAP.va_pa_offset = VA_PA_OFFSET };

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
            // SAFETY: boot section, single CPU (see above).
            let offset = unsafe { KERNEL_MAP.va_kernel_pa_offset } as u64;
            let pool_virt = &raw const EARLY_TABLES as u64;
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
/// Runtime ioremap'd device windows (above-4G PCI BARs), recorded so
/// `kernel_device_window_pte` restores them when a user mapping is torn
/// down over one. Fixed capacity; overflow refuses the ioremap.
const EXTRA_WINDOW_SLOTS: usize = 16;

struct ExtraWindows {
    base: [core::sync::atomic::AtomicU64; EXTRA_WINDOW_SLOTS],
    size: [core::sync::atomic::AtomicU64; EXTRA_WINDOW_SLOTS],
}

static EXTRA_WINDOWS: ExtraWindows = ExtraWindows {
    base: [const { core::sync::atomic::AtomicU64::new(0) }; EXTRA_WINDOW_SLOTS],
    size: [const { core::sync::atomic::AtomicU64::new(0) }; EXTRA_WINDOW_SLOTS],
};

/// Map an extra device window (identity, uncached, 2MB-granular) and
/// record it. Safe to call repeatedly for the same range.
pub fn ioremap_device_window(base: u64, size: u64) -> bool {
    use core::sync::atomic::Ordering;
    if size == 0 {
        return true;
    }
    // already recorded?
    for i in 0..EXTRA_WINDOW_SLOTS {
        let (rb, rs) = (EXTRA_WINDOWS.base[i].load(Ordering::Acquire), EXTRA_WINDOWS.size[i].load(Ordering::Acquire));
        if rb != 0 && base >= rb && base + size <= rb + rs {
            return true;
        }
    }
    for i in 0..EXTRA_WINDOW_SLOTS {
        if EXTRA_WINDOWS.base[i].load(Ordering::Acquire) == 0 {
            let device_flags = PageTableEntry::P | PageTableEntry::RW
                | PageTableEntry::ACCESSED | PageTableEntry::DIRTY
                | PageTableEntry::IO;
            map_kernel_region_huge(base, size, device_flags);
            EXTRA_WINDOWS.base[i].store(base, Ordering::Release);
            EXTRA_WINDOWS.size[i].store(size, Ordering::Release);
            return true;
        }
    }
    false
}

pub fn kernel_device_window_pte(va: u64) -> Option<u64> {
    use core::sync::atomic::Ordering;
    const WINDOWS: &[(u64, u64)] = &[
        (PCIE_ECAM_BASE, PCIE_ECAM_SIZE),  // q35 MMCONFIG, 256MB
        (0xc000_0000, 0x4000_0000),           // q35 32-bit PCI MMIO hole,
                                             // 0xc0000000..4GB (firmware
                                             // BARs incl. 0xfe000000 zone)
        (IOAPIC_BASE, 0x1000),
        (LAPIC_BASE, 0x1000),
    ];
    let page = va & !0xFFF;
    let mut hit = WINDOWS.iter().any(|&(base, size)| {
        page >= base && page + 0x1000 <= base + size
    });
    if !hit {
        for i in 0..EXTRA_WINDOW_SLOTS {
            let (rb, rs) = (EXTRA_WINDOWS.base[i].load(Ordering::Acquire), EXTRA_WINDOWS.size[i].load(Ordering::Acquire));
            if rb != 0 && page >= rb && page + 0x1000 <= rb + rs {
                hit = true;
                break;
            }
        }
    }
    if hit {
        let device_flags = PageTableEntry::P
            | PageTableEntry::RW
            | PageTableEntry::ACCESSED
            | PageTableEntry::DIRTY
            | PageTableEntry::IO;
        return Some((page & PageTableEntry::PHYS_MASK_PUBLIC) | device_flags);
    }
    None
}

// ==================== Per-BAR kernel-half ioremap (K1-BAR) ====================
//
// Under KVM the q35 host bridge assigns virtio-modern 64-bit BARs far
// above the fixed DEVICE_ALIAS64 window (observed 0xe0000008000 = 14TiB
// with the full OH device set; TCG on the same machine stays under 1TB —
// the placement is firmware/accelerator policy, not a contract we can
// pre-map for). Linux's answer is ioremap-per-BAR into vmalloc space; our
// equivalent is a bump arena in the kernel half plus this registry. Each
// discovered out-of-window BAR is mapped into the STATIC root's kernel
// half at probe time; because copy_kernel_mappings shares PML4[256..512]
// by reference and PCI probe runs before the first user mm exists, every
// CR3 in the system translates these VAs — trap-context MMIO (virtio
// notify writes, ISR reads from interrupt handlers) works under any user
// CR3, which the old identity ioremap (low half, boot root only) never
// provided.
//
// Boot-time contract: entries are never torn down. No PCI hot-unplug path
// exists in this kernel, and the old EXTRA_WINDOWS identity windows had
// the same boot-permanent lifetime. If one is ever added, it must first
// deal with existing user roots that carry the arena PML4 links.

/// Registry capacity: one slot per distinct above-window BAR. The full OH
/// set (6 blk + net + keyboard + tablet + gpu ≈ 10 functions, 1-2 modern
/// BARs each) stays under 20; overflow refuses the ioremap loudly.
const DYN_BAR_SLOTS: usize = 32;

struct DynBarMap {
    pa_base: [AtomicU64; DYN_BAR_SLOTS],
    pa_end: [AtomicU64; DYN_BAR_SLOTS],
    va_base: [AtomicU64; DYN_BAR_SLOTS],
}

impl DynBarMap {
    const fn new() -> Self {
        Self {
            pa_base: [const { AtomicU64::new(0) }; DYN_BAR_SLOTS],
            pa_end: [const { AtomicU64::new(0) }; DYN_BAR_SLOTS],
            va_base: [const { AtomicU64::new(0) }; DYN_BAR_SLOTS],
        }
    }
}

static DYN_BARS: DynBarMap = DynBarMap::new();

/// Bump cursor into IOREMAP_ARENA. Chunks are 2MB-aligned so most BAR
/// mappings get 2MB PMD leaves for their interior.
static DYN_BAR_CURSOR: AtomicU64 = AtomicU64::new(
    crate::arch::mm::memory_layout::IOREMAP_ARENA_BASE as u64,
);

/// Set by create_user_address_space: once any user mm exists, a NEW PML4
/// entry installed here is invisible to the mms forked before it (they
/// copied the kernel-half links at creation). PCI probe must therefore
/// finish before the first exec — a late ioremap is still mapped for the
/// boot root but is reported loudly instead of failing silently later.
static USER_MMS_EXIST: AtomicBool = AtomicBool::new(false);

pub fn mark_user_mms_exist() {
    USER_MMS_EXIST.store(true, Ordering::Release);
}

/// Resolve a PA through the per-BAR registry: Some(kernel-half VA) for a
/// PA inside any registered BAR window, None otherwise. Range-aware so
/// interior offsets (the virtio notify capability sits at BAR+0x7000 in
/// the KVM layout) resolve against the BAR's own mapping.
pub fn device_alias_dyn(pa: u64) -> Option<u64> {
    for i in 0..DYN_BAR_SLOTS {
        let base = DYN_BARS.pa_base[i].load(Ordering::Acquire);
        if base == 0 {
            // Slots fill in order and are never cleared.
            break;
        }
        let end = DYN_BARS.pa_end[i].load(Ordering::Acquire);
        if pa >= base && pa < end {
            let va = DYN_BARS.va_base[i].load(Ordering::Acquire);
            return Some(va + (pa - base));
        }
    }
    None
}

/// Map one device BAR (or any device PA range) into the kernel-half
/// ioremap arena and register it for `device_alias`.
///
/// Returns the kernel-half VA base on success — either an existing
/// covering mapping (fixed window or earlier registration) or a fresh
/// one. Returns None when the registry or the arena is exhausted, or the
/// mapping could not be installed.
///
/// Safe to call repeatedly for the same range (idempotent). Call from
/// single-threaded boot context (PCI probe); installs PML4 entries in the
/// static kernel root.
pub fn ioremap_bar_kernel(pa: u64, size: u64) -> Option<u64> {
    use crate::arch::mm::memory_layout::{
        device_alias_fixed, IOREMAP_ARENA_BASE, IOREMAP_ARENA_END,
    };
    if size == 0 {
        return None;
    }
    // Already inside a fixed alias window: nothing to do (and never burn
    // arena space for it).
    if let Some(va) = device_alias_fixed(pa) {
        return Some(va);
    }
    if USER_MMS_EXIST.load(Ordering::Acquire) {
        // Not fatal for the boot root, but every existing user CR3 will
        // page-fault on this BAR — the callers' warning covers the
        // consequence; this names the cause.
        crate::pr_err!(
            "ioremap_bar_kernel: pa={:#x} mapped AFTER user mms exist — \
             existing user CR3s will not see this arena link",
            pa
        );
    }
    // Already registered (re-probe of the same function, two capabilities
    // sharing one BAR)?
    if let Some(va) = device_alias_dyn(pa) {
        return Some(va);
    }
    // Fresh chunk: 2MB-aligned size, bumped atomically.
    let chunk = (size + 0x1f_ffff) & !0x1f_ffff;
    let va = DYN_BAR_CURSOR.fetch_add(chunk, Ordering::AcqRel);
    if va < IOREMAP_ARENA_BASE as u64 || va + chunk > IOREMAP_ARENA_END as u64 {
        crate::pr_err!(
            "ioremap_bar_kernel: arena exhausted (pa={:#x} size={:#x}) — \
             enlarge IOREMAP_ARENA_SIZE",
            pa, size
        );
        return None;
    }
    unsafe {
        let device_flags = PageTableEntry::P | PageTableEntry::RW
            | PageTableEntry::ACCESSED | PageTableEntry::DIRTY
            | PageTableEntry::IO;
        map_kernel_region(va, pa, size, device_flags);
    }
    // Record after the mapping is installed (Release/Acquire pairing).
    for i in 0..DYN_BAR_SLOTS {
        if DYN_BARS.pa_base[i].load(Ordering::Acquire) == 0 {
            DYN_BARS.va_base[i].store(va, Ordering::Release);
            DYN_BARS.pa_end[i].store(pa + size, Ordering::Release);
            DYN_BARS.pa_base[i].store(pa, Ordering::Release);
            return Some(va);
        }
    }
    crate::pr_err!(
        "ioremap_bar_kernel: registry full ({} slots) — pa={:#x} left on the \
         identity fallback; user-CR3 MMIO on it will KERNPANIC",
        DYN_BAR_SLOTS, pa
    );
    None
}

/// Device/ECAM mappings (q35: MMCONFIG @ 0xb0000000, IOAPIC, LAPIC).
///
/// These VAs sit ABOVE the bootstrap identity map's 2GB coverage, so the
/// walk allocates fresh PMD tables — no demotion of bootstrap leaves is
/// needed; the ECAM window goes in as 2MB IO leaves (128 entries).
pub fn setup_device_mappings() {
    use crate::arch::mm::memory_layout::{
        DEVICE_ALIAS64_BASE, DEVICE_ALIAS64_PHYS_BASE, DEVICE_ALIAS64_SIZE,
        DEVICE_ALIAS_BASE, DEVICE_ALIAS_PHYS_BASE, DEVICE_ALIAS_SIZE,
    };
    // All map helpers below are the round-1 safe walkers.
    {

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

    // KERNEL-HALF alias of the whole device region (Linux fixmap-style):
    // one contiguous window [DEVICE_ALIAS_BASE, +1.25GB) mirroring phys
    // [0xb0000000, 0x100000000). The kernel half is shared into every
    // mm's PML4, so trap-context MMIO works under any user CR3 — the
    // replacement for the E8-MM low-identity clone (that clone let
    // fork/teardown walks demote and free device-window page tables:
    // the x86 scribbler family).
    map_kernel_region(
        DEVICE_ALIAS_BASE as u64,
        DEVICE_ALIAS_PHYS_BASE as u64,
        DEVICE_ALIAS_SIZE as u64,
        device_flags,
    );

    // Second alias window: the above-4G PCI hole, eight PML4 slots of
    // 1GB leaves. With the OH device set QEMU places 64-bit BARs at
    // 0xc000000000 (768GB — it moves with device count/RAM; an earlier
    // boot had them under 512GB) — bar_va()/device_alias() must resolve
    // those under ANY CR3, or the first virtio MMIO access from
    // interrupt context under a user CR3 page-faults (seen as the OH
    // boot KERNPANIC at raw ISR address 0xc000005000 inside
    // intx_dispatch, right after PID 1 started).
    let mut v = DEVICE_ALIAS64_BASE as u64;
    let mut p = DEVICE_ALIAS64_PHYS_BASE as u64;
    let end = v + DEVICE_ALIAS64_SIZE as u64;
    while v < end {
        map_pud_huge_page(v as usize, p as usize, device_flags);
        v += PUD_SIZE;
        p += PUD_SIZE;
    }
    crate::arch::mm::asid::flush_tlb_all();

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

// ==================== mmforensics: live-tree reverse scan ====================

/// Bounded reverse scan: which LIVE page-table trees still reference
/// `target_ppn` (as root, as an upper-table link, or as a leaf/data
/// mapping)? This is the reverse map the kernel otherwise lacks — the
/// decisive capture at a RAW-FREE-OF-LIVE-TABLE echo (pcachewedge §6
/// residual): the frame is double-tracked, and the referencing tree that
/// is NOT the stamping root holds the stale pointer.
///
/// Safety shape (forensic, gated by rux:mmforensics (retired debug=mmforensics), <= a few/boot):
/// - task set snapshotted with `for_each_task_try` (non-blocking; held
///   pid-hash buckets are skipped and reported);
/// - each task's root is read with a single-field unsafe peek
///   (`mm_ptr_peek` + `(*mm).pgd`) — no Arc is cloned inside the
///   bucket-locked callback (its drop could run a teardown under the
///   spinlock). A stale read at worst yields a garbage root, which the
///   ledger-live/boot validation below rejects;
/// - every walk link is bounds-checked (`phys_valid`, above the kernel
///   image reserve, below the discovered RAM top) before descent, and
///   the walk carries a hard entry budget;
/// - shared boot subtrees (MMIO/ECAM tables cloned into every mm) are
///   link-checked but not descended into — they are boot-permanent and
///   never the freed frame.
///
/// Returns (hits, tasks_scanned, skipped_buckets, entries_read).
pub fn forensics_scan_trees(
    target_ppn: u64,
    out: &mut [crate::debug::mmforensics::TreeHit],
) -> (usize, usize, usize, usize) {
    const MAX_TASKS: usize = 128;
    let mut roots = [(0u32, 0u64); MAX_TASKS];
    let mut tasks = 0usize;
    let skipped = crate::sched::for_each_task_try(|t| {
        if tasks >= MAX_TASKS {
            return;
        }
        // SAFETY: task is pinned by the pid-hash bucket lock; the mm
        // pointer peek takes mm_slot_lock (never held in reverse around
        // pid-hash writers); the single-field read cannot fault (kernel
        // heap is linearly mapped) and a stale value is filtered by the
        // ledger validation below.
        // SAFETY: see the comment above the closure.
        let (pid, root) = unsafe {
            let root = (*t)
                .mm_ptr_peek()
                .map(|mm| (*mm).pgd)
                .unwrap_or(0);
            ((*t).pid() as u32, root)
        };
        roots[tasks] = (pid, root);
        tasks += 1;
    });

    let ram_top = (crate::mm::page_desc::max_pfn() as u64) << 12;
    let mut reads = 0usize;
    const BUDGET: usize = 400_000;
    let mut nh = 0usize;

    let mut push = |pid: u32, root: u64, level: u8, i0: u32, i1: u32| {
        if nh < out.len() {
            out[nh] = crate::debug::mmforensics::TreeHit {
                pid,
                root_ppn: root,
                level,
                idx0: i0,
                idx1: i1,
            };
        }
        nh += 1;
    };

    let link_ok = |ppn: u64| -> bool {
        let phys = ppn << 12;
        ppn != 0 && phys >= 0x1E0_0000 && phys < ram_top && crate::mm::phys_valid(phys as usize)
    };

    for &(pid, root) in roots.iter().take(tasks) {
        if root == 0 || reads >= BUDGET {
            continue;
        }
        // Root validation: a live mm's root carries a ledger stamp (or is
        // boot-shared); a garbage peek fails here and the task is skipped.
        if root != target_ppn
            && !PtLedger::is_live_table(root)
            && !PT_LEDGER.is_boot(root)
        {
            continue;
        }
        if root == target_ppn {
            push(pid, root, 0, 0xFFFF, 0xFFFF);
        }
        // Link_ok-validated frames are inside managed RAM and linearly
        // mapped; the read-only walk goes through the typed accessor.
        let Some(root_table) = PtTable::from_phys(root << 12) else { continue };
        for vpn4 in 0..USER_PTRS_PER_PGD {
            if reads >= BUDGET {
                break;
            }
            reads += 1;
            let pte4 = root_table.get(vpn4);
            if !pte4.is_valid() || pte4.is_leaf() {
                continue;
            }
            let ppn3 = pte4.ppn();
            if ppn3 == target_ppn {
                push(pid, root, 1, vpn4 as u32, 0xFFFF);
            }
            if !link_ok(ppn3) || PT_LEDGER.is_boot(ppn3) {
                continue;
            }
            let Some(table3) = PtTable::from_phys(ppn3 << 12) else { continue };
            for vpn3 in 0..512 {
                if reads >= BUDGET {
                    break;
                }
                reads += 1;
                let pte3 = table3.get(vpn3);
                if !pte3.is_valid() {
                    continue;
                }
                if pte3.is_leaf() {
                    if pte3.ppn() == target_ppn {
                        push(pid, root, 4, vpn4 as u32, vpn3 as u32);
                    }
                    continue;
                }
                let ppn2 = pte3.ppn();
                if ppn2 == target_ppn {
                    push(pid, root, 2, vpn4 as u32, vpn3 as u32);
                }
                if !link_ok(ppn2) || PT_LEDGER.is_boot(ppn2) {
                    continue;
                }
                let Some(table2) = PtTable::from_phys(ppn2 << 12) else { continue };
                for vpn2 in 0..512 {
                    if reads >= BUDGET {
                        break;
                    }
                    reads += 1;
                    let pte2 = table2.get(vpn2);
                    if !pte2.is_valid() {
                        continue;
                    }
                    if pte2.is_leaf() {
                        if pte2.ppn() == target_ppn {
                            push(pid, root, 4, vpn3 as u32, vpn2 as u32);
                        }
                        continue;
                    }
                    let ppn1 = pte2.ppn();
                    if ppn1 == target_ppn {
                        push(pid, root, 3, vpn3 as u32, vpn2 as u32);
                    }
                    if !link_ok(ppn1) || PT_LEDGER.is_boot(ppn1) {
                        continue;
                    }
                    let Some(table1) = PtTable::from_phys(ppn1 << 12) else { continue };
                    for vpn1 in 0..512 {
                        if reads >= BUDGET {
                            break;
                        }
                        reads += 1;
                        let pte1 = table1.get(vpn1);
                        if pte1.is_valid() && pte1.is_leaf() && pte1.ppn() == target_ppn {
                            push(pid, root, 4, vpn2 as u32, vpn1 as u32);
                        }
                    }
                }
            }
        }
    }
    (nh.min(out.len()), tasks, skipped, reads)
}
