//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 MMU initialization and kernel page mapping.
//!
//! Model mirrors the riscv64 backend: the kernel's static root
//! (ROOT_PAGE_TABLE — here the TTBR1 L0 table) is built at boot with a
//! staged page-table allocator (Early static pool → Fixmap/memblock →
//! Late/buddy); `get_page_table_virt` translates table frames to kernel
//! VAs per stage. Walks are 4-level (L0 512GB / L1 1GB / L2 2MB / L3 4KB)
//! against the TTBR1 (kernel) half; kernel mappings use L1/L2 block
//! descriptors where possible (linear map), L3 pages elsewhere.
//!
//! The boot CPU arrives here with the MMU ALREADY ON (boot.S installs
//! the early TTBR1 tables and drops to the kernel high-half before
//! rust_main); init() completes the full linear map + device windows on
//! top of the same root.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use super::fixmap;
use super::memory_layout::*;
use super::pagetable::{PageTable, PageTableEntry, Satp};

// ==================== Kernel root table ====================

/// The kernel's TTBR1 L0 table (static, in the image)
#[link_section = ".bss"]
pub static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

/// boot.S's early TTBR1 root (kern_l0) — its L0[511] subtree carries the
/// kernel image window that brought us into the high half. `init()`
/// adopts those entries into ROOT_PAGE_TABLE and switches TTBR1_EL1, so
/// the Rust-side builder extends the SAME live root instead of filling a
/// table the MMU never consults.
extern "C" {
    static kern_l0: PageTable;
}

/// Adopt boot.S's early TTBR1 root into ROOT_PAGE_TABLE and switch
/// TTBR1_EL1 to it.
///
/// # Safety
/// Early boot, single hart, MMU on with the boot tables live; called
/// once from init() before any Rust-side kernel mapping is built.
unsafe fn adopt_boot_ttbr1() {
    unsafe {
        // Copy all 512 L0 entries (all-but-511 are zero on the boot
        // root; copying verbatim keeps the two roots identical).
        let src = (&raw const kern_l0) as *const u8;
        let dst = (&raw mut ROOT_PAGE_TABLE) as *mut u8;
        core::ptr::copy_nonoverlapping(src, dst, core::mem::size_of::<PageTable>());
        let root_pa = root_page_table_ppn() << PAGE_SHIFT;
        asm!(
            "msr ttbr1_el1, {t}",
            "isb",
            t = in(reg) root_pa,
            options(nostack)
        );
        crate::arch::cpu::local_flush_tlb_all();
    }
}

/// Get the physical page number of the kernel root table
/// (ROOT_PAGE_TABLE lives at KERNEL_LINK_ADDR; convert via the image offset)
#[inline]
pub unsafe fn root_page_table_ppn() -> u64 {
    let root_virt = &raw mut ROOT_PAGE_TABLE as u64;
    let root_phys = root_virt.wrapping_sub(KERNEL_MAP.va_kernel_pa_offset as u64);
    root_phys / (PAGE_SIZE as u64)
}

/// Kernel root PPN (alias; interface parity)
#[inline]
pub unsafe fn get_kernel_page_table_ppn() -> u64 {
    root_page_table_ppn()
}

/// Physical address of the static kernel root (before any runtime table)
///
/// # Safety
/// Valid once boot.S has placed the kernel image (always, past _start).
#[inline]
pub unsafe fn root_static_phys() -> u64 {
    root_page_table_ppn() << PAGE_SHIFT
}

static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ==================== Staged page-table allocation ====================
//
// 1. Early: static .bss pools (buddy not up)
// 2. Fixmap: memblock allocation (stage 2 wiring)
// 3. Late: buddy allocator (stage 2 wiring)

/// Allocation stage (riscv64 parity)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AllocStage {
    Early = 0,
    Fixmap = 1,
    Late = 2,
}

impl AllocStage {
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => AllocStage::Fixmap,
            2 => AllocStage::Late,
            _ => AllocStage::Early,
        }
    }
}

static ALLOC_STAGE: AtomicU8 = AtomicU8::new(0);

/// Current page-table allocation stage
pub fn get_alloc_stage() -> AllocStage {
    AllocStage::from_u8(ALLOC_STAGE.load(Ordering::Acquire))
}

/// Whether the allocator reached the Late (buddy) stage
pub fn alloc_stage_is_late() -> bool {
    get_alloc_stage() == AllocStage::Late
}

/// Switch table allocation to the fixmap/memblock stage
pub fn pt_ops_set_fixmap() {
    ALLOC_STAGE.store(1, Ordering::Release);
}

/// Switch table allocation to the late/buddy stage
pub fn pt_ops_set_late() {
    ALLOC_STAGE.store(2, Ordering::Release);
}

/// Early L1/L2/L3 table pools (static .bss).
/// 1GB RAM linear map needs: 1 L1 (512GB→1GB entries) + 1 L2; the pools
/// carry headroom for vmemmap/fixmap/device windows.
const NUM_EARLY_L1: usize = 8;
const NUM_EARLY_L2: usize = 16;
const NUM_EARLY_L3: usize = 128;

#[link_section = ".bss"]
static mut EARLY_L1: [PageTable; NUM_EARLY_L1] = [PageTable::new(); NUM_EARLY_L1];
#[link_section = ".bss"]
static mut EARLY_L2: [PageTable; NUM_EARLY_L2] = [PageTable::new(); NUM_EARLY_L2];
#[link_section = ".bss"]
static mut EARLY_L3: [PageTable; NUM_EARLY_L3] = [PageTable::new(); NUM_EARLY_L3];

static EARLY_L1_NEXT: AtomicUsize = AtomicUsize::new(0);
static EARLY_L2_NEXT: AtomicUsize = AtomicUsize::new(0);
static EARLY_L3_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Allocate a page-table frame; returns its PHYSICAL address.
///
/// Early stage: from the static pools (converted via the image offset).
/// Later stages: stage-2 wiring (memblock / buddy) — until then the
/// early pools serve (they cover the stage-1 mapping budget).
///
/// # Safety
/// Early-boot single-threaded context, or PTE_MODIFY_LOCK held later.
pub unsafe fn alloc_page_table() -> Option<u64> {
    match get_alloc_stage() {
        AllocStage::Early | AllocStage::Fixmap => {
            // SAFETY: static pools with bump counters (see early_alloc).
            unsafe {
                let (ptr, i) = {
                    // Prefer L3 pools for leaf tables; intermediate levels
                    // have their own pools — the level distinction only
                    // sizes the accounting, the frames are identical.
                    let i = EARLY_L3_NEXT.fetch_add(1, Ordering::Relaxed);
                    if i < NUM_EARLY_L3 {
                        (&raw mut EARLY_L3[i] as u64, i)
                    } else {
                        let i2 = EARLY_L2_NEXT.fetch_add(1, Ordering::Relaxed);
                        if i2 < NUM_EARLY_L2 {
                            (&raw mut EARLY_L2[i2] as u64, i2)
                        } else {
                            let i1 = EARLY_L1_NEXT.fetch_add(1, Ordering::Relaxed);
                            if i1 < NUM_EARLY_L1 {
                                (&raw mut EARLY_L1[i1] as u64, i1)
                            } else {
                                return None;
                            }
                        }
                    }
                };
                let offset = KERNEL_MAP.va_kernel_pa_offset as u64;
                Some(ptr.wrapping_sub(offset))
            }
        }
        AllocStage::Late => {
            // Buddy-backed allocation (riscv64 twin semantics): every
            // stage-2 user tree frame comes from the zone allocator, is
            // ZEROED here (tables must never leak a previous owner's
            // entries), and is stamped into the PT ledger so the teardown
            // walk can tell Late frames (freeable) from boot-pool frames
            // (image .bss, never freed). The old static-pool fallback
            // exhausted after a handful of execs (128 frames, bump-only).
            let phys = crate::mm::alloc_pages(crate::mm::GfpFlags::GFP_KERNEL, 0);
            if phys == 0 {
                return None;
            }
            // SAFETY: phys is a freshly allocated, exclusively owned
            // order-0 frame inside the linear map.
            unsafe {
                let va = phys_to_virt(PhysAddr::new(phys as u64));
                core::ptr::write_bytes(va.bits() as *mut u8, 0, PAGE_SIZE as usize);
            }
            PT_LEDGER.stamp((phys >> PAGE_SHIFT) as u64);
            PT_LEDGER.live.fetch_add(1, Ordering::Relaxed);
            Some(phys as u64)
        }
    }
}

/// Free a page-table frame. Late-stage frames (ledger-stamped, buddy
/// owned) return to the allocator through the ledger-checked path; every
/// other frame (early static pools in the image .bss, fixmap/memblock
/// boot frames) is boot-permanent and leaks by design — refusing the
/// free keeps a recycled image frame from being handed out twice.
///
/// # Safety
/// The frame must not be reachable from any live table.
pub unsafe fn free_page_table(phys_addr: u64) {
    if !free_page_table_checked(phys_addr, "pt") {
        return;
    }
    crate::mm::page_alloc::free_pages(phys_addr as usize, 0);
}

/// Ledger-guarded half of the free: true when the caller may return the
/// frame to the buddy (exactly-one-live-stamp discipline; a double or
/// foreign free is refused with the frame left leaked rather than wired
/// into two live trees).
unsafe fn free_page_table_checked(phys_addr: u64, site: &str) -> bool {
    if get_alloc_stage() != AllocStage::Late {
        return false; // boot-stage frames are permanent
    }
    if !PT_LEDGER.take(phys_addr >> PAGE_SHIFT) {
        static REFUSED: AtomicUsize = AtomicUsize::new(0);
        if REFUSED.fetch_add(1, Ordering::Relaxed) < 8 {
            crate::pr_err!(
                "PT-FREE-REFUSED: ppn={:#x} site={} pid={} (no live Late stamp — leaked)",
                phys_addr >> PAGE_SHIFT,
                site,
                crate::sched::get_current_pid()
            );
        }
        return false;
    }
    PT_LEDGER.freed.fetch_add(1, Ordering::Relaxed);
    true
}

/// Translate a page-table frame's physical address to a kernel VA
///
/// # Safety
/// The frame must be a page-table allocated by this module.
pub unsafe fn get_page_table_virt(phys_addr: u64) -> *mut PageTable {
    match get_alloc_stage() {
        AllocStage::Early => {
            // Static pools live in the image; convert via the image offset
            let offset = unsafe { KERNEL_MAP.va_kernel_pa_offset } as u64;
            (phys_addr.wrapping_add(offset)) as *mut PageTable
        }
        AllocStage::Fixmap | AllocStage::Late => {
            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            virt_addr.bits() as *mut PageTable
        }
    }
}

// ==================== PT ledger (dfx instrumentation) ====================
//
/// Register a fork root (ledger bookkeeping; stage-2 wiring)
pub fn register_fork_root(_root_ppn: u64, _owner_pid: u32) {}

/// Free a user address space's page tables (ledger-checked 4-level walk)
///
/// The aarch64 user root is a PURE TTBR0 tree: the kernel half lives in
/// TTBR1 (a separate root), so — unlike the riscv64 single-root twin —
/// there are no cloned kernel subtrees to skip and no device-window
/// leaves to restore. Every valid entry in the tree belongs to this mm:
/// L3 user leaves get rmap removal + the last-ref page release, and every
/// table frame (root included) returns through the ledger-checked free.
///
/// # Safety
/// root_ppn must be an orphaned user root (no live MmStruct or CPU
/// references it).
pub unsafe fn free_user_page_tables(
    lock: &crate::mm::mm_struct::PteLock,
    root_ppn: u64,
) {
    use crate::mm::phys_valid;

    crate::dfx::memwatch::FUT_CALLS.fetch_add(1, Ordering::Relaxed);
    // Serialize against fork's copy walk and COW/demand faults on any mm
    // (PTE lock; the freed frames are immediately reusable).
    let _pte_guard = lock.lock_irqsave();

    let root_phys = root_ppn << PAGE_SHIFT;
    if !phys_valid(root_phys as usize) {
        return;
    }
    let root_table = get_page_table_virt(root_phys);

    // L0: all 512 slots are user slots in a TTBR0 tree
    for l0 in 0..512usize {
        let e0 = (*root_table).get(l0);
        if !e0.is_valid() || !e0.is_table() {
            continue; // user trees never host L0 blocks
        }
        let t1_phys = e0.ppn() << PAGE_SHIFT;
        if !phys_valid(t1_phys as usize) || t1_phys == root_phys {
            continue;
        }
        let t1 = get_page_table_virt(t1_phys);
        for l1 in 0..512usize {
            let e1 = (*t1).get(l1);
            if !e1.is_valid() {
                continue;
            }
            if e1.is_leaf() {
                // 1GB block leaf (never installed by the user walker;
                // defensive release like the riscv64 twin's L2-leaf arm)
                release_user_leaf_mm(e1);
                continue;
            }
            if !e1.is_table() {
                continue;
            }
            let t2_phys = e1.ppn() << PAGE_SHIFT;
            if !phys_valid(t2_phys as usize) || t2_phys == root_phys {
                continue;
            }
            let t2 = get_page_table_virt(t2_phys);
            for l2 in 0..512usize {
                let e2 = (*t2).get(l2);
                if !e2.is_valid() {
                    continue;
                }
                if e2.is_leaf() {
                    release_user_leaf_mm(e2);
                    continue;
                }
                if !e2.is_table() {
                    continue;
                }
                let t3_phys = e2.ppn() << PAGE_SHIFT;
                if !phys_valid(t3_phys as usize) || t3_phys == root_phys {
                    continue;
                }
                let t3 = get_page_table_virt(t3_phys);
                for l3 in 0..512usize {
                    let e3 = (*t3).get(l3);
                    if !e3.is_valid() {
                        continue; // holes and swap markers (none today)
                    }
                    release_user_leaf_mm(e3);
                }
                if free_page_table_checked(t3_phys, "l3") {
                    crate::mm::page_alloc::free_pages(t3_phys as usize, 0);
                }
            }
            if free_page_table_checked(t2_phys, "l2") {
                crate::mm::page_alloc::free_pages(t2_phys as usize, 0);
            }
        }
        if free_page_table_checked(t1_phys, "l1") {
            crate::mm::page_alloc::free_pages(t1_phys as usize, 0);
        }
    }

    if free_page_table_checked(root_phys, "root") {
        crate::mm::page_alloc::free_pages(root_phys as usize, 0);
    }
}

/// Release ONE user leaf frame found by the teardown walk (rmap drop +
/// last-ref put; riscv64 twin's release_user_leaf minus the text-stamp
/// adjudication — aarch64's fault paths create no text stamps). Foreign
/// frames (boot-reserved, kernel-heap, framebuffer) are skipped: their
/// owner releases them through its own path.
unsafe fn release_user_leaf_mm(entry: PageTableEntry) {
    use crate::mm::{pfn_to_page, phys_to_pfn, phys_valid};

    if !entry.is_user() {
        return; // not this mm's page (defensive; user trees are U-only)
    }
    let phys = entry.output_addr();
    if phys == 0
        || !phys_valid(phys as usize)
        || crate::mm::page_alloc::is_foreign_user_frame(phys as usize)
        || crate::drivers::gpu::get_framebuffer_info()
            .map_or(false, |i| {
                phys >= i.addr && phys < i.addr + i.size as u64
            })
    {
        return;
    }
    let pfn = phys_to_pfn(phys as usize);
    let page = pfn_to_page(pfn);
    if page.is_null() {
        return;
    }
    // SAFETY: descriptor of a leaf frame this mm mapped until this walk.
    unsafe {
        if (*page).is_mapped() {
            crate::mm::rmap::page_remove_rmap(&*page);
        }
        (*page).put_page_site(crate::dfx::driftwatch::S_TEARDOWN_RV);
    }
}

/// Rewrite ONE present L3 user leaf's permission bits from a software
/// mask, preserving its output address (exec's per-segment W^X
/// tightening — the riscv64 twin walks inline in exec.rs; here the walk
/// and the rebuild live behind this one arch helper so no call site
/// hand-rolls descriptor bits). No TLB flush: exec uses it on a tree no
/// CPU has ever translated through.
///
/// # Safety
/// root_ppn must be a valid user root; va page-aligned.
pub unsafe fn set_user_leaf_flags(root_ppn: u64, va: u64, sw_flags: u64) {
    unsafe {
        let l3 = match user_l3_table_for(root_ppn, va) {
            Some(p) => p,
            None => return,
        };
        let idx = ((va >> PAGE_SHIFT) & 0x1FF) as usize;
        let old = (*l3).get(idx);
        if old.is_valid() {
            (*l3).set(idx, PageTableEntry::rebuild_user_leaf(old.bits(), sw_flags));
        }
    }
}

/// Descend to the L3 table for `va` WITHOUT allocating (present tables
/// only); None when any intermediate is missing or non-table.
pub unsafe fn user_l3_table_for(root_ppn: u64, va: u64) -> Option<*mut PageTable> {
    unsafe {
        let mut table_phys = root_ppn << PAGE_SHIFT;
        for cur in 0..3u32 {
            let shift = PAGE_SHIFT + 9 * (3 - cur) as u64;
            let idx = ((va >> shift) & 0x1FF) as usize;
            let table = get_page_table_virt(table_phys);
            let entry = (*table).get(idx);
            if !entry.is_valid() || !entry.is_table() {
                return None;
            }
            table_phys = entry.ppn() << PAGE_SHIFT;
        }
        Some(get_page_table_virt(table_phys))
    }
}

/// Page-table ledger — the live-table registry backing the double-free
/// REFUSED diagnostics (see the riscv64 twin). One bitmap bit per RAM
/// page frame covers the ledger window [PHYS_MEMORY_BASE, +2 GB); only
/// LATE-stage allocations (the buddy-backed user-tree frames) are
/// stamped, so a missing stamp cleanly classifies every boot-pool /
/// fixmap frame as permanent.
pub struct PtLedger {
    pub live: AtomicUsize,
    pub freed: AtomicUsize,
    pub refused: AtomicUsize,
}

/// Ledger bitmap words: 2 GB of 4 KiB frames = 524288 bits = 8192 words
const PT_LEDGER_WORDS: usize = 8192;

impl PtLedger {
    pub const fn new() -> Self {
        Self {
            live: AtomicUsize::new(0),
            freed: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
        }
    }

    pub fn live_count(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }

    /// Bitmap index for a ppn (None outside the ledger window)
    #[inline]
    fn index(ppn: u64) -> Option<usize> {
        let base = (PHYS_MEMORY_BASE >> PAGE_SHIFT) as u64;
        let span = PT_LEDGER_WORDS as u64 * 64;
        let idx = ppn.checked_sub(base)?;
        if idx < span {
            Some(idx as usize)
        } else {
            None
        }
    }

    /// Record a Late-stage table allocation (exactly-once discipline: a
    /// second stamp on a live bit is a producer bug — keep the first).
    pub fn stamp(&self, ppn: u64) {
        let Some(i) = Self::index(ppn) else { return };
        let w = i / 64;
        let b = i % 64;
        let old = LEDGER_BITMAP[w].fetch_or(1 << b, Ordering::AcqRel);
        if old & (1 << b) == 0 {
            self.live.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Remove the live stamp; false when the frame was never stamped
    /// (boot pool / foreign / double free — the free must be refused).
    pub fn take(&self, ppn: u64) -> bool {
        let Some(i) = Self::index(ppn) else {
            self.refused.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        let w = i / 64;
        let b = i % 64;
        let old = LEDGER_BITMAP[w].fetch_and(!(1 << b), Ordering::AcqRel);
        if old & (1 << b) != 0 {
            true
        } else {
            self.refused.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Whether a table frame is registered live
    pub fn is_live_table(ppn: u64) -> bool {
        let Some(i) = Self::index(ppn) else { return false };
        let w = i / 64;
        let b = i % 64;
        LEDGER_BITMAP[w].load(Ordering::Acquire) & (1 << b) != 0
    }
}

/// The ledger bitmap itself (64 KiB .bss)
#[link_section = ".bss"]
static LEDGER_BITMAP: [AtomicUsize; PT_LEDGER_WORDS] =
    [const { AtomicUsize::new(0) }; PT_LEDGER_WORDS];

/// Global PT ledger
pub static PT_LEDGER: PtLedger = PtLedger::new();

// ==================== Mapping primitives (kernel TTBR1 half) ====================

/// Software permission flags for a normal kernel mapping (RWX)
pub const KERN_MAP_FLAGS: u64 = PageTableEntry::V
    | PageTableEntry::R
    | PageTableEntry::W
    | PageTableEntry::X
    | PageTableEntry::A
    | PageTableEntry::D
    | PageTableEntry::G;

/// Software permission flags for a device mapping (kernel RW, no exec)
pub const KERN_DEVICE_FLAGS: u64 = PageTableEntry::V
    | PageTableEntry::R
    | PageTableEntry::W
    | PageTableEntry::A
    | PageTableEntry::G;

/// Walk/build the kernel (TTBR1) tables down to the table at
/// `leaf_level` (1 = L1 holding 1GB blocks, 2 = L2 holding 2MB blocks,
/// 3 = L3 holding 4KB pages), returning that table's frame PA.
/// Allocates missing intermediates.
///
/// Index shifts descend from the root: the L0 table is indexed by
/// VA[47:39], L1 by VA[38:30], L2 by VA[29:21] — i.e. the shift for the
/// table at level `cur` is PAGE_SHIFT + 9 * (3 - cur). (An earlier
/// draft applied the shifts ascending from VA[20:12], which installed
/// every kernel mapping under the wrong L0 slot: the MMU indexes the
/// root with VA[47:39] and would fault on the first linear-map access.)
///
/// # Safety
/// Kernel mappings are built during single-threaded boot or under
/// PTE_MODIFY_LOCK.
unsafe fn kernel_table_for(virt: u64, leaf_level: u8) -> Option<u64> {
    unsafe {
        debug_assert!((1..=3).contains(&leaf_level));
        let mut table_phys = root_page_table_ppn() << PAGE_SHIFT;

        // Descend L0 → L1 → L2, stopping at the requested level.
        for cur in 0..leaf_level {
            let shift = PAGE_SHIFT + 9 * (3 - cur as u64);
            let idx = ((virt >> shift) & 0x1FF) as usize;
            let table = get_page_table_virt(table_phys);
            let entry = (*table).get(idx);
            let next_phys = if entry.is_valid() {
                entry.ppn() << PAGE_SHIFT
            } else {
                let np = alloc_page_table()?;
                (*table).set(idx, PageTableEntry::new_table(np >> PAGE_SHIFT));
                np
            };
            table_phys = next_phys;
        }
        Some(table_phys)
    }
}

/// Map one 4KB kernel page (L3 leaf) WITHOUT a TLB flush (batch primitive)
///
/// # Safety
/// Kernel mapping context (boot or PTE_MODIFY_LOCK); `flags` is a Rux
/// software permission mask.
pub unsafe fn map_kernel_page_noflush(virt: u64, phys: u64, flags: u64) -> bool {
    unsafe {
        let l3_phys = match kernel_table_for(virt, 3) {
            Some(p) => p,
            None => return false,
        };
        let l3 = get_page_table_virt(l3_phys);
        let idx = ((virt >> PAGE_SHIFT) & 0x1FF) as usize;
        let entry = PageTableEntry::new_page_kernel_flags(phys >> PAGE_SHIFT, flags);
        (*l3).set(idx, entry);
        PT_LEDGER.live.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// Map one 4KB kernel page with a TLB flush
///
/// # Safety
/// See map_kernel_page_noflush.
pub unsafe fn map_kernel_page(virt: u64, phys: u64, flags: u64) -> bool {
    let ok = unsafe { map_kernel_page_noflush(virt, phys, flags) };
    if ok {
        crate::arch::cpu::local_flush_tlb_all();
    }
    ok
}

/// Map a kernel VA range page-by-page (one flush at the end)
///
/// # Safety
/// See map_kernel_page_noflush.
pub unsafe fn map_kernel_region(virt: u64, phys: u64, size: u64, flags: u64) {
    unsafe {
        let mut v = virt;
        let mut p = phys;
        let end = virt + size;
        while v < end {
            map_kernel_page_noflush(v, p, flags);
            v += PAGE_SIZE;
            p += PAGE_SIZE;
        }
        crate::arch::cpu::local_flush_tlb_all();
    }
}

/// Map a 2MB L2 block (kernel half)
///
/// # Safety
/// `phys` must be 2MB-aligned; kernel mapping context.
pub unsafe fn map_pmd_huge_page(virt: usize, phys: usize, flags: u64) {
    unsafe {
        assert!(phys % (2 * 1024 * 1024) == 0, "phys must be 2MB aligned");
        let l2_phys = kernel_table_for(virt as u64, 2).expect("map_pmd_huge_page: table alloc failed");
        let l2 = get_page_table_virt(l2_phys);
        let idx = ((virt >> PMD_SHIFT) & 0x1FF) as usize;
        let entry = PageTableEntry::new_block_kernel_flags(
            (phys >> PAGE_SHIFT) as u64,
            flags,
        );
        (*l2).set(idx, entry);
    }
}

/// Map a 1GB L1 block (kernel half)
///
/// # Safety
/// `phys` must be 1GB-aligned; kernel mapping context.
unsafe fn map_pud_huge_page(virt: usize, phys: usize, flags: u64) {
    unsafe {
        assert!(phys % (1 << 30) == 0, "phys must be 1GB aligned");
        // 1GB block descriptors live at L1, indexed by VA[38:30] —
        // descend one level from the L0 root first. (An earlier draft
        // wrote the block into the ROOT table at the L1 index: the MMU
        // indexes L0 with VA[47:39], so the entry was both in the wrong
        // slot and misinterpreted as a table pointer.)
        let l1_phys = kernel_table_for(virt as u64, 1).expect("map_pud_huge_page: table alloc failed");
        let l1 = get_page_table_virt(l1_phys);
        let idx = ((virt >> PUD_SHIFT) & 0x1FF) as usize;
        let entry = PageTableEntry::new_block_kernel_flags(
            (phys >> PAGE_SHIFT) as u64,
            flags,
        );
        (*l1).set(idx, entry);
    }
}

/// Map a device page (kernel RW, Device-nGnRE memory type)
///
/// # Safety
/// Kernel mapping context; `virt` must be kernel-half.
pub unsafe fn map_device_page(virt: usize, phys: usize, _flags: u64) {
    unsafe {
        let l3_phys = kernel_table_for(virt as u64, 3).expect("map_device_page: table alloc failed");
        let l3 = get_page_table_virt(l3_phys);
        let idx = ((virt >> PAGE_SHIFT) & 0x1FF) as usize;
        (*l3).set(idx, PageTableEntry::new_page_device((phys >> PAGE_SHIFT) as u64));
        crate::arch::cpu::local_flush_tlb_all();
    }
}

/// Best block size for a linear-map segment (1GB / 2MB / 4KB)
fn best_map_size(phys: usize, virt: usize, remaining: usize) -> usize {
    if phys % (1 << 30) == 0 && virt % (1 << 30) == 0 && remaining >= (1 << 30) {
        1 << 30
    } else if phys % (2 << 20) == 0 && virt % (2 << 20) == 0 && remaining >= (2 << 20) {
        2 << 20
    } else {
        PAGE_SIZE as usize
    }
}

// ==================== Boot integration ====================

/// MMU init — completes the kernel map on the static TTBR1 root.
///
/// Called from rust_main AFTER the boot.S early tables brought us into
/// the high half. Installs the full linear map (from the memblock
/// regions), the device windows and the fixmap.
pub fn init() {
    unsafe {
        // FIRST: adopt boot.S's early TTBR1 root so every mapping built
        // below lands in the table the MMU actually walks.
        adopt_boot_ttbr1();

        // The boot CPU's early tables already cover the kernel image
        // window; KERNEL_MAP was populated by boot.S.
        KERNEL_MAP.va_pa_offset = VA_PA_OFFSET;
        KERNEL_MAP.page_offset = PAGE_OFFSET;

        // Install the fixmap (UART) so console output survives the
        // switch off the early identity window
        fixmap::init_uart_fixmap();

        MMU_INITIALIZED.store(true, Ordering::Release);
    }
}

/// Build the linear mapping over the memblock regions (called from the
/// generic boot path with the DTB-parsed regions)
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    unsafe {
        KERNEL_MAP.va_pa_offset = VA_PA_OFFSET;

        for region in memory_regions {
            let phys_start = region.base;
            let phys_end = region.base + region.size;
            let virt_start = phys_start + VA_PA_OFFSET;

            let mut phys = phys_start;
            let mut virt = virt_start;
            while phys < phys_end {
                let remaining = phys_end - phys;
                let map_size = best_map_size(phys, virt, remaining);
                match map_size {
                    s if s == 1 << 30 => map_pud_huge_page(virt, phys, KERN_MAP_FLAGS),
                    s if s == 2 << 20 => map_pmd_huge_page(virt, phys, KERN_MAP_FLAGS),
                    _ => {
                        map_kernel_page_noflush(virt as u64, phys as u64, KERN_MAP_FLAGS);
                    }
                }
                phys += map_size;
                virt += map_size;
            }
        }

        crate::arch::cpu::local_flush_tlb_all();
    }
}

/// Install the permanent device windows: the kernel-half MMIO alias
/// (phys [0, 1GB) at MMIO_ALIAS_BASE) plus the ECAM window (phys
/// [PCIE_ECAM_BASE, +PCIE_ECAM_SIZE) at ECAM_ALIAS_BASE), with
/// Device-nGnRE attributes.
pub fn setup_device_mappings() {
    unsafe {
        // Device windows as (first phys, alias VA base, total size); both
        // map 2MB device blocks at L2.
        const WINDOWS: [(usize, usize, usize); 2] = [
            (0, MMIO_ALIAS_BASE, MMIO_ALIAS_SIZE),
            (
                PCIE_ECAM_BASE as usize,
                ECAM_ALIAS_BASE,
                ECAM_ALIAS_SIZE,
            ),
        ];
        for &(phys_base, va_base, size) in WINDOWS.iter() {
            let mut pa = phys_base;
            let mut installed = 0usize;
            while pa < phys_base + size {
                let va = va_base + (pa - phys_base);
                // 2MB device blocks live at L2 — descend to it, then index
                // by VA[29:21].
                let l2_phys = match kernel_table_for(va as u64, 2) {
                    Some(p) => p,
                    None => break,
                };
                let l2 = get_page_table_virt(l2_phys);
                let idx = ((va >> PMD_SHIFT) & 0x1FF) as usize;
                let entry = PageTableEntry::new_block_device((pa >> PAGE_SHIFT) as u64);
                (*l2).set(idx, entry);
                installed += 1;
                pa += 2 << 20;
            }
            // Raw console write: printk isn't wired until the console
            // driver registers later in boot (early println! is dropped).
            use core::fmt::Write;
            let ttbr1: u64;
            unsafe { asm!("mrs {}, ttbr1_el1", out(reg) ttbr1) };
            let mut w = crate::dfx::backtrace::ConsoleWriter::new();
            let _ = write!(
                w,
                "devmap: phys {:#x}+{:#x} -> va {:#x}: {} blocks (pools l1/l2/l3 {}/{}/{}, ttbr1={:#x})\n",
                phys_base, size, va_base, installed,
                EARLY_L1_NEXT.load(Ordering::Relaxed),
                EARLY_L2_NEXT.load(Ordering::Relaxed),
                EARLY_L3_NEXT.load(Ordering::Relaxed),
                ttbr1
            );
        }
        crate::arch::cpu::local_flush_tlb_all();
    }
}

/// Enable/commit the kernel address space (parity no-op: the MMU is on
/// from boot.S; the composed satp view is published)
pub fn enable() {
    let _ = get_current_satp_pub();
}

/// Read the composed kernel "satp"
pub fn get_current_satp_pub() -> u64 {
    super::pagetable::get_satp().0
}

/// Identity-map helper (riscv64 parity; aarch64 kernel half only)
///
/// # Safety
/// Kernel mapping context.
pub unsafe fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    unsafe {
        // Identity mappings only make sense for the low half on aarch64
        // (TTBR0); the kernel builds them in the TTBR1 view at the
        // aliased VA instead. Stage-2 refines if a consumer appears.
        let alias = MMIO_ALIAS_BASE as u64 + virt.bits();
        map_kernel_page_noflush(alias, phys.bits(), flags);
    }
}

/// Map into a USER root (TTBR0) — stage 2 (the user walker + COW)
///
/// # Safety
/// root_ppn must be a valid user root.
pub unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    if root_ppn == root_page_table_ppn() {
        // Kernel-root callers route through the kernel mapping path
        return map_kernel_page_noflush(virt.bits(), phys.bits(), flags);
    }
    super::mm_ops::map_user_page(root_ppn, virt, phys, flags)
}

/// Kernel-device window PTE probe (diagnostics parity)
pub fn kernel_device_window_pte(_va: u64) -> Option<u64> {
    None // stage-2: walk helper for the device window probes
}

// ==================== Satp view ====================

/// Read the current composed satp (alias)
pub fn read_satp_view() -> Satp {
    super::pagetable::get_satp()
}
