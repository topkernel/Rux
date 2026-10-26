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
            WatchdogDump => "watchdog",
            TaskDumpKey => "taskdump",
            PeriodicDump => "periodic",
            MemWatch => "memwatch",
            MmForensics => "mmforensics",
            Scribble => "scribble",
            ScribblePark => "scribblepark",
            ScribblePid1 => "scribblepid1",
            ScribbleSweep => "scribblesweep",
            InputTrace => "inputtrace",
            EpollTrace => "epolltrace",
            ClaimInject => "claiminject",
            Strace1 => "strace1",
            SchedTrace => "schedtrace",
            Steptrace => "steptrace",
            WriteTrace => "writetrace",
            OncpuInject => "oncpuinject",
            Ledger => "ledger",
            BinderTrace => "bindertrace",
            SyscallTrace => "syscalltrace",
            FutexTrace => "futextrace",
            MonoWatch => "monowatch",
            TcpTrace => "tcptrace",
            SleepSnap => "sleepsnap",
            SockSnap => "socksnap",
            DataTap => "datatap",
            WildWatch => "wildwatch",
            MetaWatch => "metawatch",
            ParkDump => "parkdump",
            Schb => "schb",
            KsProbe => "ksprobe",
            R83HeapScan => "r83scan",
            LeakHunt => "leakhunt",
            WwCensus => "wwcensus",
        }
    };
}

pub(crate) use dfx_switch_table;

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
    /// khungtaskd-cadence snapshot of every S-state task's user entry
    /// frame (comm, tid, syscall nr, rip, a0/a1) — the appspawn-server
    /// wedge hunt (b11): clients write to the AppSpawn socket and the
    /// server never accepts, while its kickdog thread lives. Names the
    /// exact syscall the server main thread is parked in. OFF unless
    /// asked for with dfx=sleepsnap.
    SleepSnap,
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
    /// TCP per-segment forensics (hdc file-send corruption hunt): every
    /// inbound and outbound TCP segment prints seq/ack/len/flags plus
    /// receiver rcv_nxt/rcv_wnd state, budget-limited. The b143
    /// flags-only trace could not distinguish a data segment from a
    /// zero-window probe or a dup-ACK storm; the first-send corruption
    /// and mid-transfer connection drops need sequence-level timelines.
    TcpTrace,
    /// khungtaskd-cadence dump of the AF_UNIX namespace and every epoll
    /// interest set (appspawn-server wedge hunt, b15): one line per
    /// LISTENING socket (key, accept-queue depth, backlog, file id) and
    /// one per epoll instance reachable through the wake registry
    /// (registered fd, events mask, file id, ET snapshot). Cross-
    /// referencing the listener's file id against the epoll entries
    /// answers "is the listener still registered" without a guest-side
    /// /proc. OFF unless asked for with dfx=socksnap.
    SockSnap,
    /// /data namespace mutation trace (el2/100 bundle-dir hunt, dt lane):
    /// every mkdir/rmdir/unlink/rename/chown/xattr under /data prints one
    /// line with its result, bindwalk ENOENTs dump the kernel's cached
    /// children of the deepest resolved dir, and negative-dentry hits
    /// under /data are named. sc21 showed the baked per-user-100 bundle
    /// dirs replaced or emptied at runtime (el2/100/log -> ino 172,
    /// el1/100/database -> ino 44; neither is the baked inode) with no
    /// guest-side record of who did it — this tap answers that from the
    /// kernel side. OFF unless asked for with dfx=datatap.
    DataTap,
    /// Wild-write attribution tripwire (wildwrite-hunt line): the hpet
    /// quiet-statics guard cluster (plus its canary page) goes READ-ONLY
    /// in the kernel page tables after init, in every alias (kernel-half
    /// link VAs, linear map, low identity). The b77/wt1 wild-write
    /// engine's smash then page-faults AT THE WRITER — the existing
    /// kernel-#PF printer hands over rip + rbp walk + KERNPANIC dump.
    /// The canary page is also checksum-verified from the timer cadence
    /// (DMA-class writers bypass PTEs). One arming print per boot; zero
    /// traffic until a hit. See dfx/wildwatch.rs.
    WildWatch,
    /// Buddy-meta shadow scan (wildwrite-hunt line): every 10 s the
    /// timer cadence derives a shadow free set from the buddy free lists
    /// and cross-checks INV-HEAP-1/2/3 against the per-page bitmap and
    /// linkage; a lock-CONFIRMED persistent mismatch prints META-SMASH
    /// with the finding details (the buddy face of the wild writer —
    /// the ww-v2 order-18 grind). One arming print per boot; zero
    /// traffic until a hit. See dfx/metawatch.rs.
    MetaWatch,
    /// Futex park-dump family (sc1xx launcher-chain hunts): the
    /// RWFRZ-PARK writer-parking dump, the APP-PARK app-child dump and
    /// its whole-VMA APPMAP table + fd-table companions. Each park event
    /// prints hundreds-to-thousands of lines (sc179: 17.5k APPMAP lines
    /// from 6 budgeted dumps), and every kernel line is copied by OH's
    /// hilogd.rd_kmsg out of /dev/kmsg — the b128/b129 lesson: that one
    /// thread eats a vCPU at KVM speed and starves samgr. The chain
    /// questions these probes answered (routing, park sites, fd
    /// identities) are settled; OFF unless a park needs re-diagnosing.
    ParkDump,
    /// Soft-wedge heartbeat (sc-series quiet-quiet hunt, session R): the
    /// `schbd` kthread prints one ~90-byte heartbeat line every 10s
    /// straight to the UART (pid frontier, task/binder waiter counts),
    /// names each new binder process, and on a 60s pid-flow plateau fires
    /// a one-shot census (all binder read-waiters regardless of age,
    /// runnable/D-state scheduler views, stall_report with the loglevel
    /// briefly lifted, and the printk-ring tail that carries quietcon's
    /// diverted userspace console stream). At loglevel=3 every pr_warn!
    /// census record is dropped BEFORE the ring (printk_bytes pre-filter),
    /// so the quiet-quiet soft-wedge leaves no kernel evidence by design;
    /// this switch is the budgeted replacement. See dfx/schb.rs.
    Schb,
    /// Leg-16 kstack high-water probe: zero-every-fresh-kernel-stack at
    /// alloc + scan-at-free + the SC-KREC record stash. Adds a ~size-of-
    /// stack memset to EVERY spawn — the anatomy is banked and the
    /// sc218-224 pattern (probe-on boots lost the mid-boot RS lottery
    /// 3-of-4, leg-15's probe-off boots won 3/3) argues it modulates the
    /// race, so verification rolls run dfx=schb WITHOUT this token.
    KsProbe,
    /// R83/R74 wild-write hunt: the periodic 2MiB-slice kernel-heap
    /// fingerprint scan (dfx/r83hunt.rs magic_scan). Runs read_volatile
    /// sweeps over the heap from timer context; two boots were lost to
    /// faults inside the sweep (including one read of a corrupted
    /// va_pa_offset mid-scan), so the scan is opt-in like every other
    /// dfx instrument. The cheap per-tick header audits stay
    /// unconditional.
    R83HeapScan,
    /// Leak-origin probe (dfx/leakorigin.rs): records every task's
    /// OUTERMOST PREEMPT_OFFSET acquire site and trips a report when a
    /// task returns to userspace (or is seen parked, PREEMPT-LEAK class)
    /// with a nonzero preempt_count — the producer of the leaked-count
    /// wake-strand family, named at its acquire site. Hot-path cost when
    /// armed: one side-table store per outermost guard acquire plus one
    /// count load per user-mode trap return. OFF by default.
    LeakHunt,
    /// The ~1 Hz wakewatch/timer census heartbeat lines (WW/WW2/WW3 and
    /// the wakewatch wake/pick line) plus the periodic PREEMPT-LEAK
    /// reporter. These are hunt instruments: they made the serial
    /// unusable for interactive work when always on, so they are now
    /// opt-in via `dfx=wwcensus` (census/forensics boots keep them by
    /// passing the token; ordinary boots get a clean prompt).
    WwCensus,
}

const SWITCH_COUNT: usize = 34;

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
            DfxSwitch::SleepSnap => 23,
            DfxSwitch::FutexTrace => 20,
            DfxSwitch::MonoWatch => 21,
            DfxSwitch::TcpTrace => 22,
            DfxSwitch::SockSnap => 24,
            DfxSwitch::DataTap => 25,
            DfxSwitch::WildWatch => 26,
            DfxSwitch::MetaWatch => 27,
            DfxSwitch::ParkDump => 28,
            DfxSwitch::Schb => 29,
            DfxSwitch::KsProbe => 30,
            DfxSwitch::R83HeapScan => 31,
            DfxSwitch::LeakHunt => 32,
            DfxSwitch::WwCensus => 33,
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

/// Force a switch on/off programmatically (used by tests and the sysrq hook).
pub fn set(switch: DfxSwitch, on: bool) {
    SWITCHES[switch.index()].store(on, Ordering::Relaxed);
    if let DfxSwitch::MemWatch = switch {
        super::memwatch::ENABLED.store(on, Ordering::Relaxed);
    }
    if let DfxSwitch::LeakHunt = switch {
        super::leakorigin::ENABLED.store(on, Ordering::Relaxed);
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
