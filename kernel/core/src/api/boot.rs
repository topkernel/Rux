//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The BOOT SCAN seam (root-cold lane, 2026-11-24): the four rows the
//! framework's boot core uses to reach the FDT walkers now living in
//! the drivers service crate (kernel/drivers/src/of_fdt.rs —
//! the drivers/of/fdt.c shape). Split from DriverHooks on purpose
//! (crate-plan §3a rule 1: split by concern — machine-description
//! scanning is not device lifecycle); four rows, one consumer set.
//!
//! Call sites, all boot-time COLD: cmdline::init (parse_bootargs),
//! lib.rs's memblock phase and paging re-read (parse_memory_regions,
//! parse_initrd_region — both on the early identity mapping, before
//! the heap exists: the row bodies allocate nothing), and
//! vext::prime_hart_has_v (parse_cpu_isa_has_v). Frequency: a handful
//! of calls per boot; the row indirection is a few ns on paths doing
//! µs–ms of work.
//!
//! Registration: drivers' registrar publishes before the framework
//! impl boots (OnceSlot first-wins). Publication is a plain store —
//! the framework-side dispatchers below may be read from rust_main
//! before the per-CPU area exists (no Spinlock here, by rule).

#![forbid(unsafe_code)]
use crate::cmdline::{MemoryRegion, MAX_MEMORY_REGIONS};
use alloc::string::String;
use crate::safety::slot::OnceSlot;

/// The boot-machine-description scan rows.
#[derive(Clone, Copy)]
pub struct BootScanHooks {
    /// /chosen/bootargs as an owned String (None = no DTB/no property;
    /// the caller falls back to the default cmdline).
    pub parse_bootargs: fn(u64) -> Option<String>,
    /// The /memory nodes' reg pairs, stack array (tail zeroed).
    pub parse_memory_regions: fn(u64) -> [MemoryRegion; MAX_MEMORY_REGIONS],
    /// linux,initrd-start/end from /chosen (physical addresses).
    pub parse_initrd_region: fn(u64) -> Option<(usize, usize)>,
    /// First /cpus/cpu@* riscv,isa lists the V extension.
    pub parse_cpu_isa_has_v: fn(u64) -> bool,
}

impl BootScanHooks {
    /// The pre-registration table: the exact answers the in-tree walkers
    /// gave for a missing/invalid DTB, so a framework-only build keeps
    /// its historical behavior (default cmdline, empty region set, no
    /// initrd, no V).
    pub const EMPTY: BootScanHooks = BootScanHooks {
        parse_bootargs: |_| None,
        parse_memory_regions: |_| [MemoryRegion { base: 0, size: 0 }; MAX_MEMORY_REGIONS],
        parse_initrd_region: |_| None,
        parse_cpu_isa_has_v: |_| false,
    };
}

static BOOT_SCAN_HOOKS: OnceSlot<BootScanHooks> = OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins).
pub fn register_boot_scan_hooks(hooks: BootScanHooks) {
    let _ = BOOT_SCAN_HOOKS.publish(hooks);
}

/// The live table (EMPTY before registration).
#[inline]
pub fn boot_scan_hooks() -> &'static BootScanHooks {
    BOOT_SCAN_HOOKS.get().unwrap_or(&BootScanHooks::EMPTY)
}
