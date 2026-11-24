//! The MM POLICY SERVICE CRATE (crate-flip M1a): the policy half of the
//! per-arch mm_ops.rs / page_fault.rs — VMA placement decisions
//! (mmap/munmap/find_free_area), brk, stack allocation, mprotect range
//! decisions, fork's VMA-decision half, stack expansion, and the fault
//! ROUTING (which arm) — under a crate-level unsafe-forbid boundary.
//! The framework keeps the MECHANISM (kernel/src/arch/*/mm/: walks, PTE
//! atomics, fill/copy frame primitives, uaccess bottoms) plus the hook
//! table (kernel/src/mm/mm_hooks.rs); the composition bin registers
//! this crate's rows before the framework impl boots.
//!
//! Migration state (M1a): all three arches' carves are flipped — the
//! policy files below are the crates' sole compilation copies, and the
//! in-tree registrars are empty (removable in a follow-up cleanup).

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the policy file's crate::X paths resolve unchanged (the net recipe).
// (`mm` is NOT in this list: this crate's own policy/reclaim modules
// live flat at the root and reach the framework's mm as `rux::mm::...`;
// `syscall` is added for the compact/swap edges consumers.)
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno, fs, interrupt, ipc,
     safety, sched, security, signal, sync, syscall, timer, trace,
};
// The process service tree (crate-flip rung: process): the merged
// view (moved policy + the framework-staying process modules) —
// crate::process::X paths keep resolving, one-way dependency.
pub use ::process;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The reclaim-policy files, flat at the crate root (seed-core-design
// 2c.2); they were the children of the src/mm.rs module, which also
// glob-re-exported the framework's mm — these files now reach the
// framework's items by their full path (`rux::mm::...`).
pub mod hugepage;
pub mod kswapd;
pub mod oom_kill;
pub mod vmscan;
pub mod lru;
pub mod compact;
pub mod swap;

// The carved policy tree. The files were PHYSICALLY MOVED from
// kernel/src/mm/policy/ (the crate-plan recipe's step 5, done by the
// root-cold lane 2026-11-24): the framework's module tree never named
// policy/, so this crate remains their sole compilation home, under
// forbid — the #[path] indirection is gone and the crate owns the
// bytes.
#[cfg(feature = "x86_64")]
#[path = "policy/x86_64.rs"]
pub mod policy;
#[cfg(feature = "riscv64")]
#[path = "policy/riscv64.rs"]
pub mod policy;
#[cfg(feature = "aarch64")]
#[path = "policy/aarch64.rs"]
pub mod policy;

/// Registration: fills the framework's MmHooks table (the M1a data
/// seam). Called by the composition bin BEFORE the framework impl —
/// OnceSlot first-wins, so these rows prevail over any later in-tree
/// registration attempt.
pub fn register_all() {
    #[cfg(any(feature = "x86_64", feature = "riscv64", feature = "aarch64"))]
    rux::mm::mm_hooks::mm_register_hooks(rux::mm::mm_hooks::MmHooks {
        handle_mm_fault: crate::policy::handle_mm_fault,
        mmap: crate::policy::mmap,
        munmap: crate::policy::munmap,
        find_free_area: crate::policy::find_free_area,
        zap_page_range: crate::policy::zap_page_range,
        fork: crate::policy::fork,
    });
    #[cfg(not(any(feature = "x86_64", feature = "riscv64", feature = "aarch64")))]
    // Arch not carved yet: the framework's in-tree registration serves.
    {}

    // M1b: the reclaim-policy rows. While a family is still unflipped
    // its row points at the framework module (identical behavior);
    // each flip repoints the row at this crate's module. OnceSlot
    // first-wins over the framework's later in-tree registration.
    rux::mm::mm_hooks::mm_register_policy_hooks(rux::mm::mm_hooks::MmPolicyHooks {
        kswapd_init: crate::kswapd::init,
        kswapd_enable_oom: crate::kswapd::enable_oom,
        wakeup_kswapd: crate::kswapd::wakeup_kswapd,
        try_to_free_pages: crate::vmscan::try_to_free_pages,
        drain_file_cache_before_oom: crate::vmscan::drain_file_cache_before_oom,
        vscan_heartbeat: crate::vmscan::vscan_heartbeat,
        vscan_census_line: crate::vmscan::vscan_census_line,
        zone_dirty_writeback_state: crate::vmscan::zone_dirty_writeback_state,
        zone_below_dirty_writeback_mark: crate::vmscan::zone_below_dirty_writeback_mark,
        fault_oom_kill_or_retry: crate::oom_kill::fault_oom_kill_or_retry,
        kernel_fault_oom_kill: crate::oom_kill::kernel_fault_oom_kill,
        sysrq_oom_kill: crate::oom_kill::sysrq_oom_kill,
        oom_badness: crate::oom_kill::oom_badness,
        migration_in_progress: crate::compact::migration_in_progress,
        compact_zone: crate::compact::compact_zone,
        swap_init: crate::swap::swap_init,
        swap_activate: crate::swap::swap_activate,
        swap_deactivate: crate::swap::swap_deactivate,
        swap_boot_disk: crate::swap::swap_boot_disk,
        nr_active_swap: crate::swap::nr_active_swap,
        swap_free_slot: crate::swap::swap_free_slot,
        swap_stats: crate::swap::swap_stats,
        swap_write_census: crate::swap::swap_write_census,
        page_add_anon_lru: crate::lru::page_add_anon_lru,
        page_remove_lru: crate::lru::page_remove_lru,
        page_add_file_lru: crate::lru::page_add_file_lru,
        page_add_file_lru_young: crate::lru::page_add_file_lru_young,
        page_add_file_lru_young_batch: crate::lru::page_add_file_lru_young_batch,
        lru_activate_young: crate::lru::lru_activate_young,
        lru_tail: crate::lru::lru_tail,
        lru_demote_active_file_oldest: crate::lru::lru_demote_active_file_oldest,
    });
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot) when mm-side guest tests flip out with a carve. Reporting goes
/// through the framework's counters, so pass/fail lands in the same
/// totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    // The x86_64 carve moved no guest-test bodies (the mm tests call
    // the syscall layer, which dispatches through the hook table); this
    // entry stays wired for the arches' carves that do.
}
