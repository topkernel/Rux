//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Device initcall registration (the R4 crate seam, rung-4 recipe).
//!
//! The reference kernel's driver model boots through leveled initcall
//! tables (`early_initcall` / `arch_initcall` / `subsys_initcall` /
//! `device_initcall` / `late_initcall`): the boot core drains the table
//! for a level and each registered probe EITHER binds a device (returns
//! success) or reports "no device" — a NEGATIVE probe that is a normal
//! outcome, never a boot failure (the reference's driver_probe_defer /
//! ENODEV discipline). Rux's boot previously named every driver family
//! directly (`drivers::probe::init_block_devices()` et al. from
//! lib.rs); this module replaces the named call-graph with the same
//! table shape so the upcoming drivers service crate can register
//! its probes through one seam instead of being named by the framework.
//!
//! Registration happens before the drain (the framework's own probe
//! module registers during early boot today; the service crate's
//! `register_all` will append its rows before `run_initcalls` fires —
//! the composition bin calls it ahead of `rust_main_impl`).
//!
//! Kernel layer policy (docs/architecture/kernel-layers.md):
//! this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::types::Spinlock;

// InitLevel — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use super::types::{InitLevel};

// DeviceInit — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use super::types::{DeviceInit};

const MAX_INITCALLS: usize = 32;

static TABLE: Spinlock<[Option<DeviceInit>; MAX_INITCALLS]> =
    Spinlock::new([const { None }; MAX_INITCALLS]);
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Register a device probe row (the initcall-table write side). Rows
/// beyond the table capacity are dropped with a loud report — a silent
/// driver no-probe is the exact failure class this seam exists to make
/// visible.
pub fn register_initcall(row: DeviceInit) {
    let mut t = TABLE.lock();
    let i = COUNT.load(Ordering::Relaxed);
    if i >= MAX_INITCALLS {
        crate::pr_err!(
            "initcall: table full ({}), dropped probe '{}'",
            MAX_INITCALLS,
            row.name
        );
        return;
    }
    t[i] = Some(row);
    COUNT.store(i + 1, Ordering::Release);
}

/// Drain one level's probes in registration order (the reference's
/// `do_initcalls`). Negative probes (0 devices) print nothing at the
/// call site — the BOOT REPORT line only appears for bound devices,
/// matching the old named-call behavior exactly (the lib.rs print
/// sites keyed off nonzero counts).
pub fn run_initcalls(level: InitLevel) -> usize {
    let n = COUNT.load(Ordering::Acquire);
    let mut bound = 0usize;
    let table = TABLE.lock();
    for i in 0..n.min(MAX_INITCALLS) {
        let Some(row) = &table[i] else { continue };
        if row.level != level {
            continue;
        }
        let got = (row.probe)();
        if got > 0 {
            crate::pr_info!("initcall: {} bound {} device(s)", row.name, got);
        }
        bound += got;
    }
    bound
}

/// The boot-report helper the framework's print sites use (keeps the
/// existing `print_status` strings identical to the named-call era).
pub fn level_summary(level: InitLevel) -> alloc::string::String {
    let n = COUNT.load(Ordering::Acquire);
    let mut parts: Vec<&'static str> = Vec::new();
    let table = TABLE.lock();
    for i in 0..n.min(MAX_INITCALLS) {
        let Some(row) = &table[i] else { continue };
        if row.level == level {
            parts.push(row.name);
        }
    }
    format!("{:?}[{}]", level, parts.len())
}
