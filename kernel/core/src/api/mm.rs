//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! M1a FLIP SEAM: the mm-policy hook table (the NetHooks/FsHooks
//! pattern, applied to the design doc's rung M1). The framework's mm
//! entry points that are POLICY — mmap/munmap placement, the free-area
//! search, the zap range, fork's VMA-decision half, and the fault
//! ROUTING (`handle_mm_fault`) — dispatch through THIS nullable table.
//! Until the per-arch carve lands in the `mm` crate, the framework
//! registers its own in-tree policy (method/fn pointers, behavior
//! identical); each arch's carve replaces its rows with the crate's.
//!
//! THE SELF-RECURSION RULE (the S1 lesson, binding): a registered row
//! body must NEVER forward back into the accessor that dispatches the
//! same slot — registered bodies are the REAL policy fns (in-tree
//! method pointers pre-carve, crate fns post-carve), and call sites
//! read the table only through these accessors.

#![forbid(unsafe_code)]
use crate::arch::mm::VirtAddr as ArchVirtAddr;
use crate::drivers::types::GenDisk;
use crate::mm::types::MmStruct;
use crate::mm::types::VirtAddr as PageVirtAddr;
use crate::mm::types::Page;
use crate::mm::types::{MapError, Perm};
use crate::mm::types::{VmaFlags, VmaType};
use crate::mm::types::Zone;
use crate::process::types::Task;

// ==================== The seam vocabulary ====================
// FaultFlags and MmFaultResult moved here with the M1a carve (from the
// per-arch page_fault.rs): the trap layer, uaccess, ptrace/futex/gup and
// this table all name them, so they belong to the seam, not to either
// side. Each arch's page_fault.rs re-exports them so historical paths
// keep resolving.

pub struct FaultFlags;
impl FaultFlags {
    /// Read fault
    pub const READ: u32 = 1 << 0;
    /// Write fault
    pub const WRITE: u32 = 1 << 1;
    /// Execute fault (instruction fetch)
    pub const EXEC: u32 = 1 << 2;
    /// User mode access
    pub const USER: u32 = 1 << 3;
    /// Kernel mode access
    pub const KERNEL: u32 = 1 << 4;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Fault fully handled — retry the instruction
    Handled,
    /// Anonymous page materialized — retry
    Fixed,
    /// COW resolved — retry
    CowPending,
    /// Mapping already exists — retry
    AlreadyMapped,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Permission denied (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Bus error — file-backed fault past EOF (SIGBUS to the task)
    BusError,
    /// Kernel exception fixed (via exception table)
    KernelPanic,
}

/// The mm-policy hook set (one row per framework->policy entry point;
/// the reverse surface measured in docs/development/mm-split-rung.md —
/// 6 rows cover the ~40 framework call sites).
#[derive(Clone, Copy)]
pub struct MmHooks {
    /// The fault ROUTING (which arm handles the fault). Signature is
    /// identical on all three arches; the types are the compiling
    /// arch's own (`ArchVirtAddr`/`MmFaultResult`).
    pub handle_mm_fault: fn(&MmStruct, ArchVirtAddr, u32) -> MmFaultResult,
    /// mmap placement + VMA insertion.
    pub mmap: fn(
        &MmStruct,
        PageVirtAddr,
        usize,
        VmaFlags,
        VmaType,
        Perm,
        u32,
    ) -> Result<PageVirtAddr, MapError>,
    /// munmap VMA-piece removal + PTE teardown orchestration.
    pub munmap: fn(&MmStruct, PageVirtAddr, usize) -> Result<(), MapError>,
    /// The top-down free-area search.
    pub find_free_area: fn(&MmStruct, usize) -> Result<PageVirtAddr, MapError>,
    /// Zap a PTE range (keep the VMA).
    pub zap_page_range: fn(&MmStruct, PageVirtAddr, usize) -> Result<(), MapError>,
    /// fork's decision half (snapshot, COW-exempt scan, VMA replay);
    /// the PTE walk itself stays core-side (`copy_page_table_cow`).
    pub fork: fn(&MmStruct) -> Result<MmStruct, MapError>,
}

/// An unfilled row is a kernel misbuild, not a degraded mode: every
/// user address-space op routes through this table from the first
/// exec. Name it loudly instead of failing softly.
impl MmHooks {
    pub const EMPTY: MmHooks = MmHooks {
        handle_mm_fault: no_policy_handle_mm_fault,
        mmap: no_policy_mmap,
        munmap: no_policy_munmap,
        find_free_area: no_policy_find_free_area,
        zap_page_range: no_policy_zap_page_range,
        fork: no_policy_fork,
    };
}

fn no_policy_handle_mm_fault(_: &MmStruct, _: ArchVirtAddr, _: u32) -> MmFaultResult {
    panic!("M1a seam: mm policy not registered (handle_mm_fault)")
}
fn no_policy_mmap(
    _: &MmStruct,
    _: PageVirtAddr,
    _: usize,
    _: VmaFlags,
    _: VmaType,
    _: Perm,
    _: u32,
) -> Result<PageVirtAddr, MapError> {
    panic!("M1a seam: mm policy not registered (mmap)")
}
fn no_policy_munmap(_: &MmStruct, _: PageVirtAddr, _: usize) -> Result<(), MapError> {
    panic!("M1a seam: mm policy not registered (munmap)")
}
fn no_policy_find_free_area(_: &MmStruct, _: usize) -> Result<PageVirtAddr, MapError> {
    panic!("M1a seam: mm policy not registered (find_free_area)")
}
fn no_policy_zap_page_range(_: &MmStruct, _: PageVirtAddr, _: usize) -> Result<(), MapError> {
    panic!("M1a seam: mm policy not registered (zap_page_range)")
}
fn no_policy_fork(_: &MmStruct) -> Result<MmStruct, MapError> {
    panic!("M1a seam: mm policy not registered (fork)")
}

pub static MM_HOOKS: crate::safety::slot::OnceSlot<MmHooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins —
/// the composition bin's `mm::register_all` runs before the
/// framework's in-tree registration, so a carved arch's crate rows
/// prevail).
pub fn mm_register_hooks(hooks: MmHooks) {
    let _ = MM_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static MmHooks {
    MM_HOOKS.get().unwrap_or(&MmHooks::EMPTY)
}

// ---- accessors: the framework's only door into mm policy ----

#[inline]
pub fn handle_mm_fault(mm: &MmStruct, addr: ArchVirtAddr, flags: u32) -> MmFaultResult {
    (hooks().handle_mm_fault)(mm, addr, flags)
}

#[inline]
pub fn mmap(
    mm: &MmStruct,
    addr: PageVirtAddr,
    size: usize,
    flags: VmaFlags,
    vma_type: VmaType,
    perm: Perm,
    map_flags: u32,
) -> Result<PageVirtAddr, MapError> {
    (hooks().mmap)(mm, addr, size, flags, vma_type, perm, map_flags)
}

#[inline]
pub fn munmap(mm: &MmStruct, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
    (hooks().munmap)(mm, addr, size)
}

#[inline]
pub fn find_free_area(mm: &MmStruct, size: usize) -> Result<PageVirtAddr, MapError> {
    (hooks().find_free_area)(mm, size)
}

#[inline]
pub fn zap_page_range(
    mm: &MmStruct,
    start: PageVirtAddr,
    size: usize,
) -> Result<(), MapError> {
    (hooks().zap_page_range)(mm, start, size)
}

#[inline]
pub fn fork(mm: &MmStruct) -> Result<MmStruct, MapError> {
    (hooks().fork)(mm)
}

// ==================== The M1b seam (the reclaim-policy family) ====================
// The nine §4 policy files split into: seven whose framework surface is
// FN-shaped (kswapd, vmscan, oom_kill, compact, swap, lru, hugepage) and
// two whose surface is TYPE-shaped (vma, mm_struct — shelved; see
// docs/development/mm-split-rung.md). The fn-shaped rows dispatch
// through THIS table (same shape as MmHooks above: OnceSlot,
// panic-not-noop defaults, direct-read accessors); the PTE-entry codec
// and the two result types below are pure seam VOCABULARY (the
// FaultFlags/MmFaultResult precedent): compiled framework-side, called
// directly by both sides, no dispatch.

// ---- Seam vocabulary: the swap/migration PTE-entry codec ----
// (moved from mm/swap.rs: the arch page_fault/mmu_init mechanism and the
// swap policy both name it, so it belongs to the seam, not to either
// side; swap.rs re-exports these for its historical paths.)

/// Signature bit in a swap PTE — distinguishes swap entries from
/// genuinely-empty (zeroed) PTEs. Stored in bit 62.
pub const SWAP_ENTRY_SIGNATURE: u64 = 1u64 << 62;

/// Signature bit in a migration-entry PTE (a page being relocated by
/// compaction). Stored in bit 61.
pub const MIGRATION_ENTRY_SIGNATURE: u64 = 1u64 << 61;

/// Build a swap entry value suitable for storing in a PTE.
///
/// Layout:
///   Bit 62: signature (1)
///   Bits [9:8]: swap type (up to 4 devices)
///   Bits [53:10]: swap offset (up to 2^44 pages per device)
///   Bit 0 (V): 0 (triggers page fault)
#[inline]
pub fn make_swap_entry(swap_type: u32, swap_offset: u64) -> u64 {
    SWAP_ENTRY_SIGNATURE
        | ((swap_type as u64 & 0x3) << 8)
        | ((swap_offset & 0x003F_FFFF_FFFF) << 10)
}

/// True when the PTE holds a swap entry.
#[inline]
pub fn is_swap_entry(pte: u64) -> bool {
    pte & SWAP_ENTRY_SIGNATURE != 0
}

/// Build a migration-entry PTE (offset/type fields unused).
#[inline]
pub fn make_migration_entry() -> u64 {
    MIGRATION_ENTRY_SIGNATURE
}

/// True when the PTE holds a migration entry.
#[inline]
pub fn is_migration_entry(pte: u64) -> bool {
    pte & MIGRATION_ENTRY_SIGNATURE != 0
}

/// Extract the swap type from a swap entry.
#[inline]
pub fn swap_entry_type(pte: u64) -> u32 {
    ((pte >> 8) & 0x3) as u32
}

/// Extract the swap offset from a swap entry.
#[inline]
pub fn swap_entry_offset(pte: u64) -> u64 {
    (pte >> 10) & 0x003F_FFFF_FFFF
}

// ---- Seam vocabulary: shared result types ----

/// Result of a compaction attempt (moved from mm/compact.rs: the
/// framework caller matches on it, the policy produces it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompactResult {
    /// Found a free block of the requested order.
    Success,
    /// Zone fully scanned but no suitable block found.
    Complete,
    /// No movable pages to compact.
    Skipped,
}

/// Swap statistics for /proc/meminfo (moved from mm/swap.rs: same
/// producer/consumer split).
pub struct SwapStats {
    pub swap_total: usize,
    pub swap_free: usize,
}

/// The reclaim-side mm policy rows (framework -> the seven §4 files).
/// Same contract as MmHooks: registered by the composition bin (or the
/// in-tree registrar below, pre-flip); an unfilled row is a misbuild.
#[derive(Clone, Copy)]
pub struct MmPolicyHooks {
    // kswapd
    pub kswapd_init: fn(),
    pub kswapd_enable_oom: fn(),
    pub wakeup_kswapd: fn(i32),
    // vmscan
    pub try_to_free_pages: fn(i32) -> usize,
    pub drain_file_cache_before_oom: fn(usize) -> usize,
    pub vscan_heartbeat: fn(),
    pub vscan_census_line: fn() -> alloc::string::String,
    pub zone_dirty_writeback_state: fn() -> Option<(usize, usize, usize)>,
    pub zone_below_dirty_writeback_mark: fn() -> bool,
    // oom_kill
    pub fault_oom_kill_or_retry: fn(u32) -> bool,
    pub kernel_fault_oom_kill: fn() -> bool,
    pub sysrq_oom_kill: fn() -> bool,
    pub oom_badness: fn(&Task, u64) -> u64,
    // compact
    pub migration_in_progress: fn() -> bool,
    pub compact_zone: fn(*mut Zone, usize) -> CompactResult,
    // swap
    pub swap_init: fn(),
    pub swap_activate: fn(&'static GenDisk) -> Result<(u64, usize), i32>,
    pub swap_deactivate: fn() -> Result<(), i32>,
    pub swap_boot_disk: fn() -> Option<&'static GenDisk>,
    pub nr_active_swap: fn() -> bool,
    pub swap_free_slot: fn(u32, u64),
    pub swap_stats: fn() -> SwapStats,
    pub swap_write_census: fn() -> (u64, u64, u64),
    // lru
    pub page_add_anon_lru: fn(&Page),
    pub page_remove_lru: fn(&Page),
    pub page_add_file_lru: fn(&Page),
    pub page_add_file_lru_young: fn(&Page),
    pub page_add_file_lru_young_batch: fn(&[*mut Page]),
    pub lru_activate_young: fn(&Page),
    pub lru_tail: fn(usize) -> usize,
    pub lru_demote_active_file_oldest: fn(usize) -> usize,
}

impl MmPolicyHooks {
    pub const EMPTY: MmPolicyHooks = MmPolicyHooks {
        kswapd_init: no_policy_kswapd_init,
        kswapd_enable_oom: no_policy_kswapd_enable_oom,
        wakeup_kswapd: no_policy_wakeup_kswapd,
        fault_oom_kill_or_retry: no_policy_fault_oom_kill_or_retry,
        kernel_fault_oom_kill: no_policy_kernel_fault_oom_kill,
        sysrq_oom_kill: no_policy_sysrq_oom_kill,
        oom_badness: no_policy_oom_badness,
        try_to_free_pages: no_policy_try_to_free_pages,
        drain_file_cache_before_oom: no_policy_drain_file_cache_before_oom,
        vscan_heartbeat: no_policy_vscan_heartbeat,
        vscan_census_line: no_policy_vscan_census_line,
        zone_dirty_writeback_state: no_policy_zone_dirty_writeback_state,
        zone_below_dirty_writeback_mark: no_policy_zone_below_dirty_writeback_mark,
        migration_in_progress: no_policy_migration_in_progress,
        compact_zone: no_policy_compact_zone,
        swap_init: no_policy_swap_init,
        swap_activate: no_policy_swap_activate,
        swap_deactivate: no_policy_swap_deactivate,
        swap_boot_disk: no_policy_swap_boot_disk,
        nr_active_swap: no_policy_nr_active_swap,
        swap_free_slot: no_policy_swap_free_slot,
        swap_stats: no_policy_swap_stats,
        swap_write_census: no_policy_swap_write_census,
        page_add_anon_lru: no_policy_page_add_anon_lru,
        page_remove_lru: no_policy_page_remove_lru,
        page_add_file_lru: no_policy_page_add_file_lru,
        page_add_file_lru_young: no_policy_page_add_file_lru_young,
        page_add_file_lru_young_batch: no_policy_page_add_file_lru_young_batch,
        lru_activate_young: no_policy_lru_activate_young,
        lru_tail: no_policy_lru_tail,
        lru_demote_active_file_oldest: no_policy_lru_demote_active_file_oldest,
    };
}

fn no_policy_kswapd_init() {
    panic!("M1b seam: mm policy not registered (kswapd_init)")
}
fn no_policy_kswapd_enable_oom() {
    panic!("M1b seam: mm policy not registered (kswapd_enable_oom)")
}
fn no_policy_wakeup_kswapd(_: i32) {
    panic!("M1b seam: mm policy not registered (wakeup_kswapd)")
}
fn no_policy_try_to_free_pages(_: i32) -> usize {
    panic!("M1b seam: mm policy not registered (try_to_free_pages)")
}
fn no_policy_drain_file_cache_before_oom(_: usize) -> usize {
    panic!("M1b seam: mm policy not registered (drain_file_cache_before_oom)")
}
fn no_policy_vscan_heartbeat() {
    panic!("M1b seam: mm policy not registered (vscan_heartbeat)")
}
fn no_policy_vscan_census_line() -> alloc::string::String {
    panic!("M1b seam: mm policy not registered (vscan_census_line)")
}
fn no_policy_zone_dirty_writeback_state() -> Option<(usize, usize, usize)> {
    panic!("M1b seam: mm policy not registered (zone_dirty_writeback_state)")
}
fn no_policy_zone_below_dirty_writeback_mark() -> bool {
    panic!("M1b seam: mm policy not registered (zone_below_dirty_writeback_mark)")
}
fn no_policy_fault_oom_kill_or_retry(_: u32) -> bool {
    panic!("M1b seam: mm policy not registered (fault_oom_kill_or_retry)")
}
fn no_policy_kernel_fault_oom_kill() -> bool {
    panic!("M1b seam: mm policy not registered (kernel_fault_oom_kill)")
}
fn no_policy_sysrq_oom_kill() -> bool {
    panic!("M1b seam: mm policy not registered (sysrq_oom_kill)")
}
fn no_policy_oom_badness(_: &Task, _: u64) -> u64 {
    panic!("M1b seam: mm policy not registered (oom_badness)")
}
fn no_policy_migration_in_progress() -> bool {
    panic!("M1b seam: mm policy not registered (migration_in_progress)")
}
fn no_policy_compact_zone(_: *mut Zone, _: usize) -> CompactResult {
    panic!("M1b seam: mm policy not registered (compact_zone)")
}
fn no_policy_swap_init() {
    panic!("M1b seam: mm policy not registered (swap_init)")
}
fn no_policy_swap_activate(_: &'static GenDisk) -> Result<(u64, usize), i32> {
    panic!("M1b seam: mm policy not registered (swap_activate)")
}
fn no_policy_swap_deactivate() -> Result<(), i32> {
    panic!("M1b seam: mm policy not registered (swap_deactivate)")
}
fn no_policy_swap_boot_disk() -> Option<&'static GenDisk> {
    panic!("M1b seam: mm policy not registered (swap_boot_disk)")
}
fn no_policy_nr_active_swap() -> bool {
    panic!("M1b seam: mm policy not registered (nr_active_swap)")
}
fn no_policy_swap_free_slot(_: u32, _: u64) {
    panic!("M1b seam: mm policy not registered (swap_free_slot)")
}
fn no_policy_swap_stats() -> SwapStats {
    panic!("M1b seam: mm policy not registered (swap_stats)")
}
fn no_policy_swap_write_census() -> (u64, u64, u64) {
    panic!("M1b seam: mm policy not registered (swap_write_census)")
}
fn no_policy_page_add_anon_lru(_: &Page) {
    panic!("M1b seam: mm policy not registered (page_add_anon_lru)")
}
fn no_policy_page_remove_lru(_: &Page) {
    panic!("M1b seam: mm policy not registered (page_remove_lru)")
}
fn no_policy_page_add_file_lru(_: &Page) {
    panic!("M1b seam: mm policy not registered (page_add_file_lru)")
}
fn no_policy_page_add_file_lru_young(_: &Page) {
    panic!("M1b seam: mm policy not registered (page_add_file_lru_young)")
}
fn no_policy_page_add_file_lru_young_batch(_: &[*mut Page]) {
    panic!("M1b seam: mm policy not registered (page_add_file_lru_young_batch)")
}
fn no_policy_lru_activate_young(_: &Page) {
    panic!("M1b seam: mm policy not registered (lru_activate_young)")
}
fn no_policy_lru_tail(_: usize) -> usize {
    panic!("M1b seam: mm policy not registered (lru_tail)")
}
fn no_policy_lru_demote_active_file_oldest(_: usize) -> usize {
    panic!("M1b seam: mm policy not registered (lru_demote_active_file_oldest)")
}

pub static MM_POLICY_HOOKS: crate::safety::slot::OnceSlot<MmPolicyHooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins —
/// same contract as `mm_register_hooks`).
pub fn mm_register_policy_hooks(hooks: MmPolicyHooks) {
    let _ = MM_POLICY_HOOKS.publish(hooks);
}

#[inline]
fn policy_hooks() -> &'static MmPolicyHooks {
    MM_POLICY_HOOKS.get().unwrap_or(&MmPolicyHooks::EMPTY)
}

/// The pre-flip in-tree registrar: fills the reclaim rows from the
/// still-framework-side §4 files, behavior identical to the crate rows
/// that replace them file by file. Called from rust_main_impl (after
/// the composition bin's registration — OnceSlot first-wins, so a
/// flipped file's crate rows prevail over these).
pub fn register_in_tree_mm_policy() {
    // ALL families flipped (M1b complete): every row serves from the
    // mm crate, whose registration ran earlier in the composition bin
    // (OnceSlot first-wins) — these defaults never execute from the mm crate, whose
    // registration ran earlier in the composition bin (OnceSlot
    // first-wins) — their rows here stay at the EMPTY defaults and
    // never execute.
    mm_register_policy_hooks(MmPolicyHooks {
        kswapd_init: no_policy_kswapd_init,
        kswapd_enable_oom: no_policy_kswapd_enable_oom,
        wakeup_kswapd: no_policy_wakeup_kswapd,
        fault_oom_kill_or_retry: no_policy_fault_oom_kill_or_retry,
        kernel_fault_oom_kill: no_policy_kernel_fault_oom_kill,
        sysrq_oom_kill: no_policy_sysrq_oom_kill,
        oom_badness: no_policy_oom_badness,
        try_to_free_pages: no_policy_try_to_free_pages,
        drain_file_cache_before_oom: no_policy_drain_file_cache_before_oom,
        vscan_heartbeat: no_policy_vscan_heartbeat,
        vscan_census_line: no_policy_vscan_census_line,
        zone_dirty_writeback_state: no_policy_zone_dirty_writeback_state,
        zone_below_dirty_writeback_mark: no_policy_zone_below_dirty_writeback_mark,
        page_add_anon_lru: no_policy_page_add_anon_lru,
        page_remove_lru: no_policy_page_remove_lru,
        page_add_file_lru: no_policy_page_add_file_lru,
        page_add_file_lru_young: no_policy_page_add_file_lru_young,
        page_add_file_lru_young_batch: no_policy_page_add_file_lru_young_batch,
        lru_activate_young: no_policy_lru_activate_young,
        lru_tail: no_policy_lru_tail,
        lru_demote_active_file_oldest: no_policy_lru_demote_active_file_oldest,
        migration_in_progress: no_policy_migration_in_progress,
        compact_zone: no_policy_compact_zone,
        swap_init: no_policy_swap_init,
        swap_activate: no_policy_swap_activate,
        swap_deactivate: no_policy_swap_deactivate,
        swap_boot_disk: no_policy_swap_boot_disk,
        nr_active_swap: no_policy_nr_active_swap,
        swap_free_slot: no_policy_swap_free_slot,
        swap_stats: no_policy_swap_stats,
        swap_write_census: no_policy_swap_write_census,
    });
}

// ---- accessors: the framework's only door into the reclaim policy ----

#[inline]
pub fn kswapd_init() {
    (policy_hooks().kswapd_init)()
}
#[inline]
pub fn kswapd_enable_oom() {
    (policy_hooks().kswapd_enable_oom)()
}
#[inline]
pub fn wakeup_kswapd(order: i32) {
    (policy_hooks().wakeup_kswapd)(order)
}
#[inline]
pub fn try_to_free_pages(order: i32) -> usize {
    (policy_hooks().try_to_free_pages)(order)
}
#[inline]
pub fn drain_file_cache_before_oom(nr_to_reclaim: usize) -> usize {
    (policy_hooks().drain_file_cache_before_oom)(nr_to_reclaim)
}
#[inline]
pub fn vscan_heartbeat() {
    (policy_hooks().vscan_heartbeat)()
}
#[inline]
pub fn vscan_census_line() -> alloc::string::String {
    (policy_hooks().vscan_census_line)()
}
#[inline]
pub fn zone_dirty_writeback_state() -> Option<(usize, usize, usize)> {
    (policy_hooks().zone_dirty_writeback_state)()
}
#[inline]
pub fn zone_below_dirty_writeback_mark() -> bool {
    (policy_hooks().zone_below_dirty_writeback_mark)()
}
#[inline]
pub fn fault_oom_kill_or_retry(pid: u32) -> bool {
    (policy_hooks().fault_oom_kill_or_retry)(pid)
}
#[inline]
pub fn kernel_fault_oom_kill() -> bool {
    (policy_hooks().kernel_fault_oom_kill)()
}
#[inline]
pub fn sysrq_oom_kill() -> bool {
    (policy_hooks().sysrq_oom_kill)()
}
#[inline]
pub fn oom_badness(task: &Task, totalpages: u64) -> u64 {
    (policy_hooks().oom_badness)(task, totalpages)
}
#[inline]
pub fn migration_in_progress() -> bool {
    (policy_hooks().migration_in_progress)()
}
#[inline]
pub fn compact_zone(zone: *mut Zone, order: usize) -> CompactResult {
    (policy_hooks().compact_zone)(zone, order)
}
#[inline]
pub fn swap_init() {
    (policy_hooks().swap_init)()
}
#[inline]
pub fn swap_activate(disk: &'static GenDisk) -> Result<(u64, usize), i32> {
    (policy_hooks().swap_activate)(disk)
}
#[inline]
pub fn swap_deactivate() -> Result<(), i32> {
    (policy_hooks().swap_deactivate)()
}
#[inline]
pub fn swap_boot_disk() -> Option<&'static GenDisk> {
    (policy_hooks().swap_boot_disk)()
}
#[inline]
pub fn nr_active_swap() -> bool {
    (policy_hooks().nr_active_swap)()
}
#[inline]
pub fn swap_free_slot(swap_type: u32, offset: u64) {
    (policy_hooks().swap_free_slot)(swap_type, offset)
}
#[inline]
pub fn swap_stats() -> SwapStats {
    (policy_hooks().swap_stats)()
}
#[inline]
pub fn swap_write_census() -> (u64, u64, u64) {
    (policy_hooks().swap_write_census)()
}
#[inline]
pub fn page_add_anon_lru(page: &Page) {
    (policy_hooks().page_add_anon_lru)(page)
}
#[inline]
pub fn page_remove_lru(page: &Page) {
    (policy_hooks().page_remove_lru)(page)
}
#[inline]
pub fn page_add_file_lru(page: &Page) {
    (policy_hooks().page_add_file_lru)(page)
}
#[inline]
pub fn page_add_file_lru_young(page: &Page) {
    (policy_hooks().page_add_file_lru_young)(page)
}
#[inline]
pub fn page_add_file_lru_young_batch(pages: &[*mut Page]) {
    (policy_hooks().page_add_file_lru_young_batch)(pages)
}
#[inline]
pub fn lru_activate_young(page: &Page) {
    (policy_hooks().lru_activate_young)(page)
}
#[inline]
pub fn lru_tail(lru_type: usize) -> usize {
    (policy_hooks().lru_tail)(lru_type)
}
#[inline]
pub fn lru_demote_active_file_oldest(nr: usize) -> usize {
    (policy_hooks().lru_demote_active_file_oldest)(nr)
}
