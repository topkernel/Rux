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
//! Migration state (M1a): the x86_64 and riscv64 carves are flipped
//! (sole consumers of the policy files below); aarch64 still runs its
//! in-tree policy through the same table until its carve commit.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the policy file's crate::X paths resolve unchanged (the net recipe).
pub use rux::{
    arch, cmdline, config, console, dfx, drivers, errno, fs, interrupt, ipc, mm,
    process, safety, sched, sync, timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The carved policy tree. The file is NOT declared by the framework's
// module tree (kernel/src/mm/mod.rs never names policy/) — this crate
// is its sole compilation home, under forbid.
#[cfg(feature = "x86_64")]
#[path = "../../src/mm/policy/x86_64.rs"]
pub mod mm_policy;
#[cfg(feature = "riscv64")]
#[path = "../../src/mm/policy/riscv64.rs"]
pub mod mm_policy;

/// Registration: fills the framework's MmHooks table (the M1a data
/// seam). Called by the composition bin BEFORE the framework impl —
/// OnceSlot first-wins, so these rows prevail over any later in-tree
/// registration attempt.
pub fn register_all() {
    #[cfg(any(feature = "x86_64", feature = "riscv64"))]
    rux::mm::mm_hooks::mm_register_hooks(rux::mm::mm_hooks::MmHooks {
        handle_mm_fault: crate::mm_policy::handle_mm_fault,
        mmap: crate::mm_policy::mmap,
        munmap: crate::mm_policy::munmap,
        find_free_area: crate::mm_policy::find_free_area,
        zap_page_range: crate::mm_policy::zap_page_range,
        fork: crate::mm_policy::fork,
    });
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    // Arch not carved yet: the framework's in-tree registration serves.
    {}
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
