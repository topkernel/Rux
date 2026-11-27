//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Runtime-switchable DFX features.
//!
//! Compile-time features (`dfx-lock-owner` in Cargo.toml) gate the
//! hottest hooks — the spinlock owner field costs two atomic stores per
//! lock/unlock pair and is therefore a build-time opt-in used by the
//! wedge-hunting harness (test/hunt-wedge.sh).
//!
//! Switch catalog (see the RETIRED TOKENS note below for the ones
//! that already moved to the unified trace framework):
//!
//! RETIRED TOKENS (dfx-migrate rounds 4/4b/5, 2026-11, plus the
//! 2026-11-28 delete sweep): the hunt families and debug aids that
//! lived here moved into the unified trace framework
//! (`kernel/src/trace/`) — each family's gate became a `rux:` trace
//! event and its serial prints echo consumers of that enable. Old
//! `dfx=` tokens for migrated families are GONE (user directive
//! 2026-11-17: no compatibility aliases — use trace_event=). The
//! round-6 delete sweep (docs/development/debug-alignment.md S2)
//! then removed the instrument BODIES whose hunts had closed; their
//! event declarations remain in trace/ (armed default false, no
//! emitters) and their token names live only in git history. Four
//! dead tokens (`thpark`, `syncguard`, `syncwatch`, `pctlmon`) named
//! instruments whose probe bodies no longer exist.
//!
//! What survives here is deliberate, not leftover — BEHAVIOR INJECTION
//! only (`claiminject`, `oncpuinject`): these CHANGE kernel
//! scheduling/rejection behavior to reproduce wedge shapes — they are
//! fault injectors, not observability, and have no tracepoint form.
//! Per the observability policy the table must not grow — new
//! observability goes through tracepoints.

#![forbid(unsafe_code)]

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, Ordering};

/// The canonical switch table: enum variant → `dfx=` cmdline spelling.
///
/// This macro is the single source of truth for the catalog. It generates
/// the enum→name mapping, the `ALL` enumeration used by the userspace
/// control surface (the debugfs knobs under `/sys/kernel/debug/dfx/`,
/// see `debug/mod.rs`) and the per-switch control files. Adding a switch
/// therefore means adding one line here plus the variant — nothing else
/// has to be kept in sync.
macro_rules! dfx_switch_table {
    ($m:ident) => {
        $m! {
            ClaimInject => "claiminject",
            OncpuInject => "oncpuinject",
        }
    };
}

pub(crate) use dfx_switch_table;

/// Runtime DFX switches, indexed by name on the `dfx=` cmdline parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DfxSwitch {
    /// Adjudication injector (GNOME-wedge hunt): every 50 jiffies the
    /// timer softirq leaks a claim of CPU 7 (nonexistent on any board we
    /// boot) onto every LINKED CFS task that shows {on_cpu=false,
    /// claim=-1} — exactly the protocol-impossible state the g2 freeze
    /// left behind. On an unpatched kernel this reproduces the wedge
    /// deterministically (tasks stranded on the timeline, every pick
    /// skips them); with the pick-side stale-claim heal the system must
    /// keep scheduling. OFF unless explicitly requested; never enable in
    /// production or LTP runs. PERMANENT SWITCH (dfx-migrate round 5):
    /// a fault injector changes kernel behavior — no tracepoint form.
    ClaimInject,
    /// R85 (tail-phase PICK-SKIP wedge adjudication): every 50 jiffies,
    /// stamp the FULL lost-release ownership shape {on_cpu=true,
    /// claim=7(nonexistent), pk=LIVE} onto every LINKED CFS task that
    /// shows the pickable {on_cpu=false, claim=-1}. An unpatched kernel
    /// strands the task forever (every pick skips it why=1); a kernel
    /// with the R85 pick-side ownership adjudication sweeps the poison
    /// within ~50 jiffies (STALE-ONCPU-RESET) and keeps scheduling. OFF
    /// unless explicitly requested; never enable in production or LTP
    /// runs. PERMANENT SWITCH (dfx-migrate round 5): fault injector.
    OncpuInject,
}

const SWITCH_COUNT: usize = 2;

static SWITCHES: [AtomicBool; SWITCH_COUNT] = [const { AtomicBool::new(false) }; SWITCH_COUNT];

macro_rules! build_all {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// Every DFX switch, in the canonical order of the switch table.
        ///
        /// The order matches the `index()` mapping below, so
        /// `ALL[i].index() == i` holds for every entry.
        pub const ALL: [DfxSwitch; SWITCH_COUNT] = [$(DfxSwitch::$variant),*];
    };
}

dfx_switch_table!(build_all);

const _: () = assert!(ALL.len() == SWITCH_COUNT);

macro_rules! build_name {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// The `dfx=` cmdline spelling of this switch. Also the file name
        /// of its control file under `/sys/kernel/debug/dfx/`.
        pub fn name(self) -> &'static str {
            match self {
                $(DfxSwitch::$variant => $name),*
            }
        }
    };
}

impl DfxSwitch {
    fn index(self) -> usize {
        match self {
            // Renumbering note (dfx-migrate round 5): the 43 rows that
            // moved to the unified trace framework (39 families + the
            // 4 dead tokens thpark/syncguard/syncwatch/pctlmon whose
            // probe bodies no longer exist) left the table, so the six
            // permanent switches (injection + gdb-park + the PTE-mutating
            // detector) compacted to 0..1 in table order. Cmdline lookup
            // is by name, so the shift is invisible to users; indices
            // only key the SWITCHES array.
            DfxSwitch::ClaimInject => 0,
            DfxSwitch::OncpuInject => 1,
        }
    }

    dfx_switch_table!(build_name);

    fn from_name(name: &str) -> Option<DfxSwitch> {
        ALL.iter().find(|s| s.name() == name).copied()
    }
}

/// Whether a runtime switch is enabled.
pub fn enabled(switch: DfxSwitch) -> bool {
    SWITCHES[switch.index()].load(Ordering::Relaxed)
}

/// The retired dfx= tokens — the dfx-migrate migration ledger.
///
/// Rounds 4/4b/5 of the dfx-trace migration moved the switch families
/// out of the bespoke table and into the unified trace framework: each
/// family's gate is now a `rux:` trace event (armed via `trace_event=`
/// or tracefs `set_event`), the bespoke serial prints became echo
/// consumers of that same enable, and the old `dfx=` token survives as
/// a boot ALIAS that arms the events the family used to arm — the
/// retired family's own event FIRST, then the same reference-family
/// events `trace_alias` armed for it before the migration (so an
/// alias-armed boot records the identical timeline a pre-migration
/// `dfx=` boot recorded, plus the new family event).
///
/// The switch table above SHRANK with every row that moved here (62 →
/// 49 in round 4, 49 → 6 in round 5); this table is the compatibility
/// tail, not a second switch catalog.
/// Retired dfx= tokens whose instruments NO LONGER EXIST — the probe
/// bodies were removed when their hunts closed (dfx-migrate round 5
/// found them dead: no call sites anywhere in the tree). The tokens
/// are recognized so old hunt rigs get an honest notice instead of the
/// generic unknown-switch warning, but there is nothing to arm:
///
/// - `thpark` — the user-stack park-dump rode the txn-hunt audit,
///   which the round-4 `rux:txnhang` migration replaced; the token
///   arms the audit timeline via trace events, so it
///   is NOT in this list.
/// - `syncguard` / `syncwatch` — the reference-parity same-proc
///   transaction rejection became UNCONDITIONAL kernel behavior
///   (9349d7459 "the wedge family's door"); the switch no longer
///   gated anything.
/// Force a switch on/off programmatically (used by the debugfs control
/// files under /sys/kernel/debug/dfx/).
pub fn set(switch: DfxSwitch, on: bool) {
    SWITCHES[switch.index()].store(on, Ordering::Relaxed);
    #[cfg(not(feature = "x86_64"))]
    {
        let _ = (switch, on);
    }
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
                crate::pr_warn!(
                    "dfx: unknown switch '{}' (use trace_event= instead)",
                    name
                );
            }
        }
    }
}
