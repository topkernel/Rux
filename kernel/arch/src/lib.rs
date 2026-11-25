//! The ARCH COLD SERVICE CRATE (arch-cold lane): the three
//! architectures' cold diagnostic halves — the fatal-fault forensic
//! walkers, the segv-history and ripsample report formatters, the
//! remote-BT requester — under a crate-level unsafe-forbid boundary.
//!
//! Crate shape (one crate, per-arch modules — the brief's
//! `arch/{x86_64,riscv64,aarch64}` family realized inside ONE
//! package): the three arches never compile together (one target per
//! link), so the family-as-three-crates form would triple the seam
//! wiring (bin dependency rows, feature forwarding, registration
//! calls, negative probes) for zero extra enforcement — the forbid
//! boundary is per-crate and identical either way. Each arch's module
//! is compiled only under its feature, exactly like the framework's
//! arch/mod.rs backends.
//!
//! What stayed in the framework (the lane's stay/leave record, the
//! seed-core §2 floor plus the §1a.2 stay rules): boot entries and
//! boot-time parsing (multiboot/e820/DTB — F-BOOT; they run before
//! rust_main, the earliest point any table can be published), the
//! context switch, the trap handler bodies, the uaccess bottoms, the
//! page-table mechanism (mmu_init/mm_ops/asid/fixmap — called by name
//! from framework mm's fault/fork/exit paths), cpu register ops,
//! thread FPU save, smp per-CPU setup and bringup, the panic printers,
//! and the tick-path samplers. See the lane report for the per-file
//! disposition and the scan-disagreement record (the core-size
//! worklists' arch cold columns over-count: the mm/smp/ipi families
//! are framework-called mechanism, not movable policy).
//!
//! The seam: `rux::api::arch::ArchDiagHooks` — cold rows
//! only, published from rust_main via `arch::register_all`
//! (OnceSlot; INV-SVCTEST-1's no-Spinlock rule). Raw reads cross
//! through the audited framework diag edges.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root
// so the moved files' crate::X paths resolve unchanged (the net
// recipe, process precedent).
pub use rux::{
    arch, config, console, debug, drivers, errno, fs, interrupt, ipc, mm, net,
    perf, process, sched, sync, syscall, timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug};
extern crate alloc;

#[cfg(feature = "x86_64")]
pub mod x86_64;
#[cfg(feature = "riscv64")]
pub mod riscv64;

/// Publish this build's arch rows into the framework's ArchDiagHooks
/// slot (cold diagnostics; OnceSlot first-wins — the process/ipc seam
/// shape). Called from rust_main before rust_main_impl: every consumer
/// of these rows (fatal user-fault deaths, armed reports, hunter
/// pokes) fires strictly after that point.
pub fn register_all() {
    #[cfg(feature = "x86_64")]
    {
        use rux::api::arch::{ArchDiagHooks, ARCH_DIAG_HOOKS};
        let _ = ARCH_DIAG_HOOKS.publish(ArchDiagHooks {
            scribble_foreign_entry: x86_64::trap_diag::scribble_foreign_entry,
            user_segv_forensics: x86_64::trap_diag::user_segv_forensics,
            gp_regmem_dump: x86_64::trap_diag::gp_regmem_dump,
            gp_mmhistory_dump: x86_64::trap_diag::gp_mmhistory_dump,
            segv_hist_age: x86_64::trap_diag::segv_hist_age,
            stime_profile_top: x86_64::trap_diag::stime_profile_top,
            utime_ripsample_report: x86_64::trap_diag::utime_ripsample_report,
            remote_backtrace: x86_64::ipi_diag::remote_backtrace,
            ..ArchDiagHooks::NOOP
        });
    }
    #[cfg(feature = "riscv64")]
    {
        use rux::api::arch::{ArchDiagHooks, ARCH_DIAG_HOOKS};
        let _ = ARCH_DIAG_HOOKS.publish(ArchDiagHooks {
            sigill_mapping_forensics: riscv64::trap_diag::sigill_mapping_forensics,
            ..ArchDiagHooks::NOOP
        });
    }
    #[cfg(feature = "aarch64")]
    {
        // The aarch64 cold remainder is floor (build-only arch; see the
        // lane report) — nothing to publish.
    }
}
