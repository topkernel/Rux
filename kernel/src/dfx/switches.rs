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
//! RETIRED TOKENS (dfx-migrate rounds 4/4b/5, 2026-11): the hunt
//! families and debug aids that lived here — rounds 4/4b:
//! `schedtrace`, `fuxstuck`, `txnhunt`, `mmforensics`, `sysring`,
//! `ohhunt`, `sigtrace`, `futextrace`, `epolltrace`, `taskdump`,
//! `periodic`, `watchdog`, `memwatch`; round 5: `schb`, `parkdump`,
//! `sleepsnap`, `ksprobe`, `wwcensus`, `bindertrace`,
//! `syscalltrace`, `inputtrace`, `exthunt`, `sfddbg`, `dbtrace`,
//! `datatap`, `ams46`, `vmap`, `poolcensus`, `slowtask`, `execcost`,
//! `tfdcensus`, `scmtap`, `sysforensic`, `filerd`, `lostpark`,
//! `allocwatch`, `r83scan`, `leakhunt`, `acqdiv`, `metawatch`,
//! `scribble`, `monowatch`, `epollscan`, `treehunt`, `tcptrace`,
//! `strace1`, `ripsample`, `socksnap`, `writetrace`, `ledger`,
//! `pid1spin`, `steptrace` — moved into the unified trace framework
//! (`kernel/src/trace/`): each family's gate is now a `rux:` trace
//! event, its serial prints are echo consumers of that event's
//! enable. Old `dfx=` tokens for migrated families are GONE (user
//! directive 2026-11-17: no compatibility aliases — use trace_event=).
//! (`thpark`, `syncguard`, `syncwatch`, `pctlmon`) name instruments
//! whose probe bodies no longer exist — see `dead_dfx_tokens`.
//!
//! What survives here is deliberate, not leftover:
//!
//! - BEHAVIOR INJECTION (`claiminject`, `oncpuinject`): these CHANGE
//!   kernel scheduling/rejection behavior to reproduce wedge shapes —
//!   they are fault injectors, not observability, and have no
//!   tracepoint form.
//! - GDB-PARK ACTIONS (`scribblepark`, `scribblepid1`,
//!   `scribblesweep`): they PARK a CPU at a chosen moment so an
//!   external gdb session can arm watchpoints — an action taken on
//!   the machine, not an observation of it. They arm the `rux:scribble`
//!   detection engine as a side effect.
//! - ACTIVE DETECTOR (`wildwatch`): its detection mechanism IS state
//!   mutation — arming write-protects kernel page-table regions and
//!   sets CR0.WP. The #PF hit dump rides the existing kernel fault
//!   printer; arming must stay a boot-time switch.
//!
//! Per the observability policy the table must not grow — new
//! observability goes through tracepoints.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, Ordering};

/// The canonical switch table: enum variant → `dfx=` cmdline spelling.
///
/// This macro is the single source of truth for the catalog. It generates
/// the enum→name mapping, the `ALL` enumeration used by the userspace
/// control surface (the debugfs knobs under `/sys/kernel/debug/dfx/`,
/// see `dfx/mod.rs`) and the per-switch control files. Adding a switch
/// therefore means adding one line here plus the variant — nothing else
/// has to be kept in sync.
macro_rules! dfx_switch_table {
    ($m:ident) => {
        $m! {
            ClaimInject => "claiminject",
            OncpuInject => "oncpuinject",
            ScribblePark => "scribblepark",
            ScribblePid1 => "scribblepid1",
            ScribbleSweep => "scribblesweep",
            WildWatch => "wildwatch",
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
    /// Also park (scribble_park breakpoint) on the first confirmed hit
    /// so a gdb session can arm QEMU watchpoints on the victim address.
    /// PERMANENT SWITCH (dfx-migrate round 5): parking a CPU for an
    /// external debugger is an action, not observation. Arming it also
    /// arms the `rux:scribble` detection engine.
    ScribblePark,
    /// Park when pid 1 registers — the init task's Task struct and
    /// kernel stack live at boot-fixed addresses for the whole run, so
    /// a gdb session can arm persistent watchpoints on them.
    /// PERMANENT SWITCH (dfx-migrate round 5, gdb-park action); arms
    /// `rux:scribble` as a side effect.
    ScribblePid1,
    /// Park when the 64th task registers (early in the fork/exec storm)
    /// so gdb can arm quiet-region watchpoints across the live set.
    /// PERMANENT SWITCH (dfx-migrate round 5, gdb-park action); arms
    /// `rux:scribble` as a side effect.
    ScribbleSweep,
    /// Wild-write attribution tripwire (wildwrite-hunt line): the hpet
    /// quiet-statics guard cluster (plus its canary page) goes READ-ONLY
    /// in the kernel page tables after init, in every alias (kernel-half
    /// link VAs, linear map, low identity). The b77/wt1 wild-write
    /// engine's smash then page-faults AT THE WRITER — the existing
    /// kernel-#PF printer hands over rip + rbp walk + KERNPANIC dump.
    /// The canary page is also checksum-verified from the timer cadence
    /// (DMA-class writers bypass PTEs). One arming print per boot; zero
    /// traffic until a hit. See dfx/wildwatch.rs. PERMANENT SWITCH
    /// (dfx-migrate round 5): the detection mechanism itself mutates
    /// kernel state (PTE write-bits + CR0.WP), so arming cannot be a
    /// runtime-safe trace-event enable.
    WildWatch,
}

const SWITCH_COUNT: usize = 6;

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
            // wildwatch) compact to 0..5 in table order. Cmdline lookup
            // is by name, so the shift is invisible to users; indices
            // only key the SWITCHES array.
            DfxSwitch::ClaimInject => 0,
            DfxSwitch::OncpuInject => 1,
            DfxSwitch::ScribblePark => 2,
            DfxSwitch::ScribblePid1 => 3,
            DfxSwitch::ScribbleSweep => 4,
            DfxSwitch::WildWatch => 5,
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

/// OH hunt-print master gate (110th): the always-on hunt probes that
/// rode every perf boot — mtprobe / bindwalk / bindrec / sbxbind /
/// devopen-probe / mntslow / appclone / the drm ioctl census — printed
/// ~1.7k lines per boot at LOGLEVEL=4, and every line costs TWICE (the
/// busy-poll UART emit charged to the printing task, plus hilogd's
/// /dev/kmsg drain). They had only the `sprobe=0` silencer before, so
/// the default was ON. This gate flips the default: armed ONLY when
/// the unified-framework event `rux:ohhunt` is armed (`trace_event=`,
/// `set_event`, or the retired `dfx=ohhunt` boot token — see
/// or the legacy `sprobe=1` opt-in. Hunt
/// rigs that still want the prints add one of those tokens.
pub fn oh_hunt_prints_on() -> bool {
    crate::trace::EVENT_OHHUNT.armed()
        || crate::cmdline::get_param("sprobe").map(|v| v == "1").unwrap_or(false)
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
    // The gdb-park switches park the machine for an external debugger
    // (an ACTION — why they stayed out of the trace framework) but
    // they also need the scribble detection engine live, so arming
    // them arms `rux:scribble` too.
    #[cfg(feature = "x86_64")]
    {
        if let DfxSwitch::ScribblePark = switch {
            super::scribble::PARK.store(on, Ordering::Relaxed);
            if on {
                crate::trace::EVENT_SCRIBBLE
                    .enabled
                    .store(true, Ordering::Relaxed);
            }
        }
        if let DfxSwitch::ScribblePid1 = switch {
            super::scribble::PARK_PID1.store(on, Ordering::Relaxed);
            if on {
                crate::trace::EVENT_SCRIBBLE
                    .enabled
                    .store(true, Ordering::Relaxed);
            }
        }
        if let DfxSwitch::ScribbleSweep = switch {
            super::scribble::PARK_SWEEP.store(on, Ordering::Relaxed);
            if on {
                crate::trace::EVENT_SCRIBBLE
                    .enabled
                    .store(true, Ordering::Relaxed);
            }
        }
    }
    #[cfg(not(feature = "x86_64"))]
    {
        let _ = (switch, on);
    }
}

/// Parse `dfx=` from the kernel command line. Unknown names are ignored
/// with a warning rather than failing the boot.
pub fn init_from_cmdline() {
    // sc57: the fuxwatch write-watchpoint target is its own boot param
    // (fuxwatch=0xADDR[,DELAYSEC]) — parsed here so it is live from the
    // first censusd round.
    crate::sync::futex::fuxwatch_parse();
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
