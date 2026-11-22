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
// (`mm` is NOT in this list — it is the src/mm.rs module, which owns
// the flipped §4 policy files and re-exports everything else from the
// framework; `syscall` is added for the compact/swap edges consumers.)
pub use rux::{
    arch, cmdline, config, console, dfx, drivers, errno, fs, interrupt, ipc,
    process, safety, sched, security, signal, sync, syscall, timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

/// The mm tree (see src/mm.rs — the rux-fs nesting recipe).
pub mod mm;

// The carved policy tree. The file is NOT declared by the framework's
// module tree (kernel/src/mm/mod.rs never names policy/) — this crate
// is its sole compilation home, under forbid.
#[cfg(feature = "x86_64")]
#[path = "../../src/mm/policy/x86_64.rs"]
pub mod mm_policy;
#[cfg(feature = "riscv64")]
#[path = "../../src/mm/policy/riscv64.rs"]
pub mod mm_policy;
#[cfg(feature = "aarch64")]
#[path = "../../src/mm/policy/aarch64.rs"]
pub mod mm_policy;

/// Registration: fills the framework's MmHooks table (the M1a data
/// seam). Called by the composition bin BEFORE the framework impl —
/// OnceSlot first-wins, so these rows prevail over any later in-tree
/// registration attempt.
pub fn register_all() {
    #[cfg(any(feature = "x86_64", feature = "riscv64", feature = "aarch64"))]
    rux::mm::mm_hooks::mm_register_hooks(rux::mm::mm_hooks::MmHooks {
        handle_mm_fault: crate::mm_policy::handle_mm_fault,
        mmap: crate::mm_policy::mmap,
        munmap: crate::mm_policy::munmap,
        find_free_area: crate::mm_policy::find_free_area,
        zap_page_range: crate::mm_policy::zap_page_range,
        fork: crate::mm_policy::fork,
    });
    #[cfg(not(any(feature = "x86_64", feature = "riscv64", feature = "aarch64")))]
    // Arch not carved yet: the framework's in-tree registration serves.
    {}

    // M1b: the reclaim-policy rows. While a family is still unflipped
    // its row points at the framework module (identical behavior);
    // each flip repoints the row at this crate's module. OnceSlot
    // first-wins over the framework's later in-tree registration.
    rux::mm::mm_hooks::mm_register_policy_hooks(rux::mm::mm_hooks::MmPolicyHooks {
        kswapd_init: crate::mm::kswapd::init,
        kswapd_enable_oom: crate::mm::kswapd::enable_oom,
        wakeup_kswapd: crate::mm::kswapd::wakeup_kswapd,
        try_to_free_pages: crate::mm::vmscan::try_to_free_pages,
        drain_file_cache_before_oom: crate::mm::vmscan::drain_file_cache_before_oom,
        vscan_heartbeat: crate::mm::vmscan::vscan_heartbeat,
        vscan_census_line: crate::mm::vmscan::vscan_census_line,
        zone_dirty_writeback_state: crate::mm::vmscan::zone_dirty_writeback_state,
        zone_below_dirty_writeback_mark: crate::mm::vmscan::zone_below_dirty_writeback_mark,
        fault_oom_kill_or_retry: crate::mm::oom_kill::fault_oom_kill_or_retry,
        kernel_fault_oom_kill: crate::mm::oom_kill::kernel_fault_oom_kill,
        sysrq_oom_kill: crate::mm::oom_kill::sysrq_oom_kill,
        oom_badness: crate::mm::oom_kill::oom_badness,
        migration_in_progress: crate::mm::compact::migration_in_progress,
        compact_zone: crate::mm::compact::compact_zone,
        swap_init: crate::mm::swap::swap_init,
        swap_activate: crate::mm::swap::swap_activate,
        swap_deactivate: crate::mm::swap::swap_deactivate,
        swap_boot_disk: crate::mm::swap::swap_boot_disk,
        nr_active_swap: crate::mm::swap::nr_active_swap,
        swap_free_slot: crate::mm::swap::swap_free_slot,
        swap_stats: crate::mm::swap::swap_stats,
        swap_write_census: crate::mm::swap::swap_write_census,
        page_add_anon_lru: crate::mm::lru::page_add_anon_lru,
        page_remove_lru: crate::mm::lru::page_remove_lru,
        page_add_file_lru: crate::mm::lru::page_add_file_lru,
        page_add_file_lru_young: crate::mm::lru::page_add_file_lru_young,
        page_add_file_lru_young_batch: crate::mm::lru::page_add_file_lru_young_batch,
        lru_activate_young: crate::mm::lru::lru_activate_young,
        lru_tail: crate::mm::lru::lru_tail,
        lru_demote_active_file_oldest: crate::mm::lru::lru_demote_active_file_oldest,
    });
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (rux-bin registers it at
/// boot) when mm-side guest tests flip out with a carve. Reporting goes
/// through the framework's counters, so pass/fail lands in the same
/// totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    // The x86_64 carve moved no guest-test bodies (the mm tests call
    // the syscall layer, which dispatches through the hook table); this
    // entry stays wired for the arches' carves that do.
}
