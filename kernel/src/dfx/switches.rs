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
//! - `schedtrace` — scheduler lifecycle forensic prints: the deferred
//!   exit-notify delivery log (EXITNOTIFY lines, one per child exit —
//!   a bare `ls` in the shell fires one) and the R56 orphan-harvest
//!   diagnostics (R56-ORPHAN-HARVESTED / ORPHAN-CTX parked-context
//!   dumps from the idle-loop harvester). All are hunt aids for the
//!   dbus-activation and reparent-safety investigations; they pollute
//!   the structured boot table, so they are OFF unless requested.

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
    /// Heap-leak hunt: size-class alloc/free counters + big-object call
    /// site histogram, reported with the periodic dump (see memwatch.rs).
    MemWatch,
    /// mm forensic rings: PTE-install ledger (PTEI_RING), double-teardown
    /// detector (FUT_RING scan) and PT stamp/free rings. All per-PTE or
    /// per-exit costs on the fork/exit hot path — OFF in production boots
    /// (LTP fork_procs lost seconds per 1000 forks to them); enable only
    /// while hunting the fake-OOM / stale-tree families.
    MmForensics,
    /// Byte-level scribble hunter (dfx/scribble.rs): shadow-canary over
    /// every live Task's thread.{fs,gs,sp,ret} + parked pt_regs control
    /// words, verified at switch-out quiesce and from the timer tick.
    Scribble,
    /// Also park (scribble_park breakpoint) on the first confirmed hit
    /// so a gdb session can arm QEMU watchpoints on the victim address.
    ScribblePark,
    /// Park when pid 1 registers — the init task's Task struct and
    /// kernel stack live at boot-fixed addresses for the whole run, so
    /// a gdb session can arm persistent watchpoints on them.
    ScribblePid1,
    /// Park when the 64th task registers (early in the fork/exec storm)
    /// so gdb can arm quiet-region watchpoints across the live set.
    ScribbleSweep,
    /// Input-event payload trace: every event pushed into the evdev queues
    /// prints a one-line record (device, type, code, value) plus the push
    /// count. Used to hunt phantom input streams; printing happens on the
    /// event path so it is OFF in production boots.
    InputTrace,
    /// epoll delivery trace: every (rate-limited) epoll_wait return prints
    /// pid/comm plus each ready fd's number and event bits — hunts
    /// permanently ready fds that spin epoll_pwait/ppoll callers.
    EpollTrace,
    /// Adjudication injector (GNOME-wedge hunt): every 50 jiffies the
    /// timer softirq leaks a claim of CPU 7 (nonexistent on any board we
    /// boot) onto every LINKED CFS task that shows {on_cpu=false,
    /// claim=-1} — exactly the protocol-impossible state the g2 freeze
    /// left behind. On an unpatched kernel this reproduces the wedge
    /// deterministically (tasks stranded on the timeline, every pick
    /// skips them); with the pick-side stale-claim heal the system must
    /// keep scheduling. OFF unless explicitly requested; never enable in
    /// production or LTP runs.
    ClaimInject,
    /// Syscall trace for PID 1 (loader hunts): the file/memory syscalls
    /// ld.so issues (openat/fstat/stat/mmap/read/lseek/close/...) each
    /// print one line with decoded pathnames and results, so a musl
    /// "library not found" can be attributed to the failing step instead
    /// of guessed at.
    Strace1,
    /// Scheduler lifecycle forensic prints (EXITNOTIFY delivery log +
    /// R56 orphan-harvest diagnostics). See the module doc for details.
    SchedTrace,
    /// Per-syscall return line + per-trap vector line. Used during the
    /// SIGHUP / fake-crash hunts; left on, every syscall printed two
    /// lines (trap entry + return) — a single OH boot wrote a 2.7GB
    /// serial log and the UART traffic slowed the system enough to cost
    /// samgr the hdf_devmgr race. OFF unless asked for.
    Steptrace,
    /// R85 (tail-phase PICK-SKIP wedge adjudication): every 50 jiffies,
    /// stamp the FULL lost-release ownership shape {on_cpu=true,
    /// claim=7(nonexistent), pk=LIVE} onto every LINKED CFS task that
    /// shows the pickable {on_cpu=false, claim=-1}. This is the exact
    /// terminal microstate of the pid-1 tail wedge (linked, marked,
    /// owned by no slot and no in-flight switch). An unpatched kernel
    /// strands the task forever (every pick skips it why=1); a kernel
    /// with the R85 pick-side ownership adjudication sweeps the poison
    /// within ~50 jiffies (STALE-ONCPU-RESET) and keeps scheduling. OFF
    /// unless explicitly requested; never enable in production or LTP
    /// runs.
    OncpuInject,
    /// Per-short-write printable capture (OH bring-up): binary hilog /
    /// binder frames still leak past the printable-head filter and spam
    /// the console; default OFF, enable with dfx=writetrace. Mirrors
    /// main's d0fb238c+b18f683f (this branch adds fd + a hex prefix for
    /// offline frame decoding).
    WriteTrace,
    /// Full physical-accounting ledger (fake-OOM family): arms the UART
    /// magic "LEDGER!" for an on-demand census + reconciliation dump and
    /// includes the OOM-moment ledger automatically. See dfx/ledger.rs.
    Ledger,
    /// Per-transaction binder trace (HDF devmgr<->devhost handshake hunt):
    /// one "btx:" line per resolved BC_TRANSACTION/BC_REPLY (from pid,
    /// target pid, handle, code, oneway/twoway, data sizes). Default
    /// OFF - a full OH boot resolves tens of thousands of transactions.
    BinderTrace,
    /// OH bring-up syscall trace family (b128/b129 hunts): the per-dlopen
    /// `so-open[...]` trace, per-mmap `FIXED-MAP`/`PROTNONE-MAP`, the
    /// mremap ENTER/EXIT pair and the binder `GET_SENDER_INFO` round-trip
    /// prints. All were unconditional pr_info during the dlopen-retry and
    /// VMA-surgery hunts; at OH scale they emit ~57k lines per boot, and
    /// OH's hilogd.rd_kmsg thread copies every one of them from /dev/kmsg
    /// into /data log files — at KVM speed that single thread eats a
    /// whole vCPU and starves samgr's in-flight transactions (b128/b129:
    /// 130s+ binder stalls, the whole display chain queued behind samgr).
    /// Same failure shape as Steptrace's 2.7GB serial log. OFF unless
    /// asked for with dfx=syscalltrace.
    SyscallTrace,
    /// Futex wake-miss probe (b133 hunt): when a FUTEX_WAKE wants more
    /// waiters than it finds, scan every bucket for same-uaddr waiters
    /// under a non-matching key and report both mm identities — the
    /// lost-wake signature when a PRIVATE-futex waiter and its waker
    /// disagree on the address-space identity. Rate-limited to 32 lines;
    /// the scan walks all buckets so it stays behind this switch.
    FutexTrace,
    /// CLOCK_MONOTONIC jump tripwire (timer-lane hunt): ~1 Hz samples of
    /// the shared timebase from the timer softirq; reports backward
    /// motion or multi-second spikes with the active clocksource named
    /// (see dfx/monowatch.rs). Near-zero cost; meant to ride along on
    /// every hunt boot so clock jumps are caught live instead of
    /// inferred from impossible probe totals afterwards.
    MonoWatch,
}

const SWITCH_COUNT: usize = 22;

static SWITCHES: [AtomicBool; SWITCH_COUNT] = [const { AtomicBool::new(false) }; SWITCH_COUNT];

impl DfxSwitch {
    fn index(self) -> usize {
        match self {
            DfxSwitch::WatchdogDump => 0,
            DfxSwitch::TaskDumpKey => 1,
            DfxSwitch::PeriodicDump => 2,
            DfxSwitch::MemWatch => 3,
            DfxSwitch::MmForensics => 4,
            DfxSwitch::Scribble => 5,
            DfxSwitch::ScribblePark => 6,
            DfxSwitch::ScribblePid1 => 7,
            DfxSwitch::ScribbleSweep => 8,
            DfxSwitch::InputTrace => 9,
            DfxSwitch::EpollTrace => 10,
            DfxSwitch::ClaimInject => 11,
            DfxSwitch::Strace1 => 12,
            DfxSwitch::SchedTrace => 13,
            DfxSwitch::Steptrace => 14,
            // 15 used to be shared by WriteTrace AND OncpuInject (both
            // mapped to 15) — `dfx=writetrace` silently armed the
            // adjudication injector (and vice versa). Split while adding
            // the Ledger switch: each switch now owns its bit.
            DfxSwitch::WriteTrace => 15,
            DfxSwitch::OncpuInject => 16,
            DfxSwitch::Ledger => 17,
            DfxSwitch::BinderTrace => 18,
            DfxSwitch::SyscallTrace => 19,
            DfxSwitch::FutexTrace => 20,
            DfxSwitch::MonoWatch => 21,
        }
    }

    fn from_name(name: &str) -> Option<DfxSwitch> {
        match name {
            "watchdog" => Some(DfxSwitch::WatchdogDump),
            "taskdump" => Some(DfxSwitch::TaskDumpKey),
            "periodic" => Some(DfxSwitch::PeriodicDump),
            "memwatch" => Some(DfxSwitch::MemWatch),
            "mmforensics" => Some(DfxSwitch::MmForensics),
            "scribble" => Some(DfxSwitch::Scribble),
            "scribblepark" => Some(DfxSwitch::ScribblePark),
            "scribblepid1" => Some(DfxSwitch::ScribblePid1),
            "scribblesweep" => Some(DfxSwitch::ScribbleSweep),
            "inputtrace" => Some(DfxSwitch::InputTrace),
            "epolltrace" => Some(DfxSwitch::EpollTrace),
            "claiminject" => Some(DfxSwitch::ClaimInject),
            "strace1" => Some(DfxSwitch::Strace1),
            "schedtrace" => Some(DfxSwitch::SchedTrace),
            "steptrace" => Some(DfxSwitch::Steptrace),
            "writetrace" => Some(DfxSwitch::WriteTrace),
            "oncpuinject" => Some(DfxSwitch::OncpuInject),
            "ledger" => Some(DfxSwitch::Ledger),
            "bindertrace" => Some(DfxSwitch::BinderTrace),
            "syscalltrace" => Some(DfxSwitch::SyscallTrace),
            "futextrace" => Some(DfxSwitch::FutexTrace),
            "monowatch" => Some(DfxSwitch::MonoWatch),
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
    if let DfxSwitch::MemWatch = switch {
        super::memwatch::ENABLED.store(on, Ordering::Relaxed);
    }
    if let DfxSwitch::Scribble = switch {
        #[cfg(feature = "x86_64")]
        super::scribble::ENABLED.store(on, Ordering::Relaxed);
    }
    if let DfxSwitch::ScribblePark = switch {
        #[cfg(feature = "x86_64")]
        {
            super::scribble::PARK.store(on, Ordering::Relaxed);
            if on {
                #[cfg(feature = "x86_64")]
                super::scribble::ENABLED.store(true, Ordering::Relaxed);
            }
        }
    }
    if let DfxSwitch::ScribbleSweep = switch {
        #[cfg(feature = "x86_64")]
        {
            super::scribble::PARK_SWEEP.store(on, Ordering::Relaxed);
            if on {
                #[cfg(feature = "x86_64")]
                super::scribble::ENABLED.store(true, Ordering::Relaxed);
            }
        }
    }
    if let DfxSwitch::ScribblePid1 = switch {
        #[cfg(feature = "x86_64")]
        {
            super::scribble::PARK_PID1.store(on, Ordering::Relaxed);
            if on {
                #[cfg(feature = "x86_64")]
                super::scribble::ENABLED.store(true, Ordering::Relaxed);
            }
        }
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
                crate::pr_warn!("dfx: unknown switch '{}' ignored", name);
            }
        }
    }
}
