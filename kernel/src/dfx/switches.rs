//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Runtime-switchable DFX features.
//!
//! Expensive diagnostics are OFF by default and enabled per boot via the
//! kernel command line, so production runs pay nothing unless a problem
//! is being hunted:
//!
//! ```text
//! -append "root=/dev/vda rw dfx=taskdump,watchdog ..."
//! ```
//!
//! Compile-time features (`dfx-lock-owner` in Cargo.toml) gate the
//! hottest hooks — the spinlock owner field costs two atomic stores per
//! lock/unlock pair and is therefore a build-time opt-in used by the
//! wedge-hunting harness (test/hunt-wedge.sh).
//!
//! Switch catalog:
//! - `watchdog` — spinlock deadlock warnings also dump all task states
//!   (the deadlock print itself is always on; this only gates the extra
//!   task snapshot after it).
//! - `taskdump` — manual task-state snapshots via the DFX sysrq hook
//!   (same dump code path, different trigger).

use core::sync::atomic::{AtomicBool, Ordering};

/// Runtime DFX switches, indexed by name on the `dfx=` cmdline parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DfxSwitch {
    /// Deadlock watchdog prints a full task-state snapshot afterwards.
    WatchdogDump,
    /// The UART sysrq-style trigger for on-demand task snapshots is armed.
    TaskDumpKey,
    /// Periodic task snapshots (every PERIODIC_SECS) from the timer
    /// softirq — catches silent hangs that never trip any watchdog.
    PeriodicDump,
}

const SWITCH_COUNT: usize = 3;

static SWITCHES: [AtomicBool; SWITCH_COUNT] = [const { AtomicBool::new(false) }; SWITCH_COUNT];

impl DfxSwitch {
    fn index(self) -> usize {
        match self {
            DfxSwitch::WatchdogDump => 0,
            DfxSwitch::TaskDumpKey => 1,
            DfxSwitch::PeriodicDump => 2,
        }
    }

    fn from_name(name: &str) -> Option<DfxSwitch> {
        match name {
            "watchdog" => Some(DfxSwitch::WatchdogDump),
            "taskdump" => Some(DfxSwitch::TaskDumpKey),
            "periodic" => Some(DfxSwitch::PeriodicDump),
            _ => None,
        }
    }
}

/// Whether a runtime switch is enabled.
pub fn enabled(switch: DfxSwitch) -> bool {
    SWITCHES[switch.index()].load(Ordering::Relaxed)
}

/// Force a switch on/off programmatically (used by tests and the sysrq hook).
pub fn set(switch: DfxSwitch, on: bool) {
    SWITCHES[switch.index()].store(on, Ordering::Relaxed);
}

/// Parse `dfx=` from the kernel command line. Unknown names are ignored
/// with a warning rather than failing the boot.
pub fn init_from_cmdline() {
    let raw = match crate::cmdline::get_param("dfx") {
        Some(v) => v,
        None => return,
    };
    for name in raw.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        match DfxSwitch::from_name(name) {
            Some(s) => {
                set(s, true);
                crate::pr_info!("dfx: runtime switch '{}' enabled", name);
            }
            None => {
                crate::pr_warn!("dfx: unknown switch '{}' ignored", name);
            }
        }
    }
}
