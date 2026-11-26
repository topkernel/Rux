//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The debug subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct`, `enum` and type alias of the
//! subsystem lives here, together with the constants those definitions
//! name (array lengths); every `impl` block lives in the topic files
//! beside the functions that use it — an impl is code, a definition is
//! data layout. The exceptions are definitions local to code: a type
//! declared inside a function stays there.
//!
//! This file holds no `unsafe`: the attribute below makes the compiler
//! say so (docs/development/forbid-registry.txt). A field that was
//! private to its old topic file is `pub(super)` here because the code
//! that touches it lives in that sibling file: the debug directory is
//! the smallest visibility boundary Rust offers for that split.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU8, AtomicU32, AtomicU64, AtomicUsize};

// ---------------------------------------------------------------------------
// the reference-ring entry and the tree-hit record
// ---------------------------------------------------------------------------

/// One recorded descriptor refcount transition on a PageCache frame.
pub struct RefRingEntry {
    pub ppn: AtomicU64,
    /// kind (0 = get, 1 = put) in the high byte, driftwatch site below.
    pub kind_site: AtomicU64,
    /// Refcount BEFORE the transition (the value the actor observed).
    pub old: AtomicI32,
    /// Descriptor mapcount at the transition.
    pub mc: AtomicI32,
    pub pid: AtomicU32,
}

/// One live-tree reference to the target frame, found by the bounded
/// reverse scan. `level` semantics (x86_64 naming; riscv64 Sv39 maps
/// analogously): 0 = the root itself, 1..3 = an upper-table link at the
/// given depth, 4 = a LEAF entry (the frame is mapped as DATA by that
/// tree). `idx0/idx1` are the walk indices naming the exact entry.
#[derive(Clone, Copy)]
pub struct TreeHit {
    pub pid: u32,
    pub root_ppn: u64,
    pub level: u8,
    pub idx0: u32,
    pub idx1: u32,
}

// ---------------------------------------------------------------------------
// the kernel taint bitmask (bitflags)
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Kernel taint flags
    ///
    /// Each flag indicates a specific class of anomaly. Once set, flags are never cleared.
    /// The taint string can be read via `/proc/sys/kernel/tainted`.
    #[derive(Debug, Clone, Copy)]
    pub struct TaintFlags: u32 {
        /// Proprietary module was loaded
        const PROPRIETARY_MODULE = 0x0001;
        /// Module was force-loaded
        const FORCED_MODULE      = 0x0002;
        /// Unsafe SMP processor detected
        const UNSAFE_SMP         = 0x0004;
        /// Module was force-unloaded
        const FORCED_RMMOD       = 0x0008;
        /// Machine check error occurred
        const MACHINE_CHECK      = 0x0010;
        /// Bad page reference
        const BAD_PAGE           = 0x0020;
        /// User-set taint flag
        const USER               = 0x0040;
        /// Kernel has died (OOPS)
        const DIE                = 0x0080;
        /// ACPI table overridden
        const OVERRIDDEN_ACPI    = 0x0100;
        /// Warning occurred (WARN)
        const WARN               = 0x0200;
        /// Crappy module loaded
        const CRAP               = 0x0400;
        /// Firmware workaround active
        const FIRMWARE_WORKAROUND= 0x0800;
        /// Out-of-tree module loaded
        const OOT_MODULE         = 0x1000;
        /// Unsigned module loaded
        const UNSIGNED_MODULE    = 0x2000;
        /// Soft lockup occurred
        const SOFTLOCKUP         = 0x4000;
        /// Live patch applied
        const LIVEPATCH          = 0x8000;
    }
}

// ---------------------------------------------------------------------------
// the class-lock holder row and its RAII guard token
// ---------------------------------------------------------------------------

pub(super) struct Holder {
    /// (site | class << 8) the stamp was written for; also the exit key.
    pub(super) key: AtomicU32,
    /// Monotonic acquire epoch; exit only clears a matching epoch so an
    /// overwritten slot cannot be cleared by a stale release.
    pub(super) epoch: AtomicU64,
    pub(super) pid: AtomicU32,
    pub(super) tgid: AtomicU32,
    pub(super) jif: AtomicU64,
    /// Last report jiffies for THIS slot (per-slot throttle).
    pub(super) jif_report: AtomicU64,
}

pub struct AwGuard {
    pub(super) site: u8,
    pub(super) class: u8,
    pub(super) ep: u64,
}

// ---------------------------------------------------------------------------
// the frame-print console writer
// ---------------------------------------------------------------------------

/// Lockless console writer that bypasses printk and CONSOLE_LOGLEVEL.
///
/// Writes staged bytes through the lockless batched TX path. Safe to
/// use in panic/OOPS contexts where normal printk infrastructure may be
/// unavailable or recursive.
pub struct ConsoleWriter {
    /// Whether this writer holds the printk console emit lock (bounded
    /// best-effort acquire in `new()`): keeps diagnostic reports from
    /// interleaving mid-line with an in-flight printk record on another
    /// CPU (OH b140: "…len BUG: soft lockup" garble). On timeout —
    /// e.g. a wedged holder, or this CPU's own interrupted task holds
    /// the lock — output proceeds locklessly exactly as before.
    pub(super) emit_locked: bool,
    /// Staged-byte buffer (perf-console): write_str fragments coalesce
    /// here and flush through ONE batched TX pass per 128 bytes instead
    /// of one register-IO pair per byte — a softlockup stack dump is
    /// thousands of bytes through this writer.
    pub(super) buf: [u8; 128],
    pub(super) len: usize,
}

// ---------------------------------------------------------------------------
// the debug= instrument-switch enum (DfxSwitch)
// ---------------------------------------------------------------------------

/// Runtime DEBUG switches, indexed by name on the `debug=` cmdline parameter.
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
    /// production or LTP runs. PERMANENT SWITCH (debug-migrate round 5):
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
    /// runs. PERMANENT SWITCH (debug-migrate round 5): fault injector.
    OncpuInject,
    /// Also park (scribble_park breakpoint) on the first confirmed hit
    /// so a gdb session can arm QEMU watchpoints on the victim address.
    /// PERMANENT SWITCH (debug-migrate round 5): parking a CPU for an
    /// external debugger is an action, not observation. Arming it also
    /// arms the `rux:scribble` detection engine.
    ScribblePark,
    /// Park when pid 1 registers — the init task's Task struct and
    /// kernel stack live at boot-fixed addresses for the whole run, so
    /// a gdb session can arm persistent watchpoints on them.
    /// PERMANENT SWITCH (debug-migrate round 5, gdb-park action); arms
    /// `rux:scribble` as a side effect.
    ScribblePid1,
    /// Park when the 64th task registers (early in the fork/exec storm)
    /// so gdb can arm quiet-region watchpoints across the live set.
    /// PERMANENT SWITCH (debug-migrate round 5, gdb-park action); arms
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
    /// traffic until a hit. See debug/wildwatch.rs. PERMANENT SWITCH
    /// (debug-migrate round 5): the detection mechanism itself mutates
    /// kernel state (PTE write-bits + CR0.WP), so arming cannot be a
    /// runtime-safe trace-event enable.
    WildWatch,
}

// ---------------------------------------------------------------------------
// the dump stage, the emit budget, the frozen answer and
// the deferred-emit/focus rings of the task dumper
// ---------------------------------------------------------------------------

pub(super) struct DumpStage {
    pub(super) buf: [u8; 512],
    pub(super) n: usize,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum EmitBudget {
    /// 50 ms — dump contexts that hold no scheduler-critical lock and
    /// run with interrupts ENABLED (the stage guard is dropped before
    /// the flush wait; see `staged_putc`).
    Full,
    /// 100 µs — sched-path prints under the GRQ and irq-context echoes.
    Brief,
}

/// Answer block filled by the IPI callback ON the frozen CPU. Static so
/// the CSD callback (IRQ context) never touches the caller's stack.
#[cfg(feature = "x86_64")]
pub(super) struct FrozenAnswer {
    pub(super) answered: core::sync::atomic::AtomicBool,
    pub(super) gs_cpu: core::sync::atomic::AtomicU32,
    pub(super) gs_magic: core::sync::atomic::AtomicU64,
    pub(super) lapic: core::sync::atomic::AtomicU32,
    pub(super) int_rip: core::sync::atomic::AtomicU64,
    pub(super) int_rsp: core::sync::atomic::AtomicU64,
    pub(super) stack: [core::sync::atomic::AtomicU64; 12],
    pub(super) stack_n: core::sync::atomic::AtomicUsize,
}

pub(super) const DEFER_LINE_MAX: usize = 256;

pub(super) struct DeferredEmit {
    pub(super) ring: [Option<alloc::string::String>; DEFER_LINE_MAX],
    pub(super) head: usize,
    pub(super) tail: usize,
    pub(super) dropped: u64,
}

pub(super) const DEFER_FOCUS_MAX: usize = 512;

pub(super) struct DeferredFocus {
    pub(super) ring: [Option<alloc::string::String>; DEFER_FOCUS_MAX],
    pub(super) head: usize,
    pub(super) tail: usize,
    pub(super) dropped: u64,
}

// ---------------------------------------------------------------------------
// heap/page accounting site rows (memwatch)
// ---------------------------------------------------------------------------

/// Frames recorded per site: the raw_vec/alloc growth machinery at -O0 is
/// deeper than the kernel code (realloc: this method <- __rust_realloc <-
/// realloc_nonnull <- grow_impl <- Allocator::grow <- finish_grow <-
/// grow_amortized <- do_reserve <- reserve <- append_elements <- THE
/// KERNEL CODE), so 12 slots are needed before the kernel frame appears.
pub const SITE_FRAMES: usize = 12;

pub(super) struct Site {
    pub(super) key: AtomicUsize,
    pub(super) count: AtomicUsize,
    pub(super) bucket: AtomicUsize,
    /// First-seen exact byte size at this site (disambiguates within a
    /// power-of-two bucket — 129..256 vs exactly 256 matters).
    pub(super) size: AtomicUsize,
    pub(super) frames: [AtomicUsize; SITE_FRAMES],
}

pub struct BigRingSlot {
    pub size: AtomicUsize,
    pub ptr: AtomicUsize,
    pub pid: AtomicUsize,
    pub frames: [AtomicUsize; 4],
}

pub struct LiveSite {
    pub key: AtomicUsize,
    pub allocs: AtomicUsize,
    pub frees: AtomicUsize,
    pub size: AtomicUsize,
    pub bucket: AtomicUsize,
    pub frames: [AtomicUsize; SITE_FRAMES],
}

pub struct PageSite {
    pub key: AtomicUsize,
    pub allocs: AtomicUsize,
    pub frees: AtomicUsize,
    pub order: AtomicUsize,
    pub frames: [AtomicUsize; SITE_FRAMES],
}

// ---------------------------------------------------------------------------
// the physical-accounting ledger's line and census rows
// ---------------------------------------------------------------------------

/// Append a decimal to a stack line buffer. The cap always leaves one
/// byte for the terminator: `emit` writes buf[len] = '\n', and a push
/// that consumed the whole buffer would push that store out of bounds —
/// the exact shape of the pt3/pt4 audit-boot panics ("index out of
/// bounds: the len is 160 but the index is 160", ledger.rs emit). pt5
/// survived only because its census digit widths happened to sum to 159.
pub(super) struct Line {
    pub(super) buf: [u8; 160],
    pub(super) len: usize,
}

/// One bucket row of the census.
#[derive(Default)]
pub(super) struct Census {
    /// refcount==0 and OnFreelist — the LEADER page of a buddy free block
    /// (the flag marks leaders only, see PageFlag::OnFreelist / R15-6).
    pub(super) free_leader: u64,
    /// refcount==0, not reserved, not a leader — the MEMBER pages of
    /// higher-order free blocks. Not a leak: the leader owns them.
    pub(super) free_member: u64,
    pub(super) reserved: u64,
    pub(super) page_tables: u64,
    pub(super) anon: u64,
    pub(super) cow: u64,
    pub(super) mapped_file: u64,
    pub(super) lru_file: u64,
    pub(super) other_used: u64,
    // refcount distribution of used pages (pinned/shared shape)
    pub(super) rc_eq1: u64,
    pub(super) rc_2_8: u64,
    pub(super) rc_gt8: u64,
}

// ---------------------------------------------------------------------------
// the gdb-audit slot, ring, audit record and report buffer
// ---------------------------------------------------------------------------

#[cfg(feature = "x86_64")]
pub(super) struct ScribbleSlot {
    /// Task pointer this slot tracks (0 = free).
    pub(super) task: AtomicU64,
    /// 0 = pre-first-run (init writes not yet absorbed), 1 = armed.
    pub(super) armed: AtomicU32,
    /// Seqlock: odd while quiesce() is rewriting the shadows, bumped on
    /// every re-sync. verify() only trusts a read set bracketed by an
    /// unchanged even generation.
    pub(super) gen: AtomicU32,
    pub(super) sh_fs: AtomicU64,
    pub(super) sh_gs: AtomicU64,
    pub(super) sh_sp: AtomicU64,
    pub(super) sh_ret: AtomicU64,
    pub(super) sh_rip: AtomicU64,
    pub(super) sh_rsp: AtomicU64,
    pub(super) sh_cs: AtomicU64,
    /// Anomaly-class rate-limit bitmask for verify_consistency.
    pub(super) armed_swapped: AtomicU32,
}

#[cfg(feature = "x86_64")]
pub struct RingEntry {
    pub stamp: AtomicU64, // jiffies at append
    pub cpu: AtomicU32,
    pub kind: AtomicU32, // 1 = mark_picked(next), 2 = publish(prev,next)
    pub a: AtomicU64,    // marked/prev task
    pub b: AtomicU64,    // next task (publish) / 0
}

#[cfg(feature = "x86_64")]
pub struct AuditRec {
    pub kind: AtomicU32,
    pub addr: AtomicU64,
    pub order: AtomicU32,
    pub cpu: AtomicU32,
    pub frames: [AtomicU64; 6],
}

/// Full text of the last HIT report (also readable via gdb after the
/// run dies: serial interleaving cannot destroy this copy).
#[cfg(feature = "x86_64")]
pub struct ReportBuf(pub(super) core::cell::UnsafeCell<[u8; 4096]>);

// ---------------------------------------------------------------------------
// the per-op cost table rows (syscost)
// ---------------------------------------------------------------------------

pub struct SysStat {
    pub count: AtomicU64,
    pub cyc: AtomicU64,
    pub max: AtomicU64,
}

pub struct RipSlot {
    pub page: AtomicU64,
    pub count: AtomicU64,
}

pub struct SlowEnt {
    pub nr: AtomicU64,
    pub cyc: AtomicU64,
    pub comm: [AtomicU8; 16],
}

pub struct EofSpin {
    pub comm: [AtomicU8; 16],
    pub path: [AtomicU8; 96],
    pub n: AtomicU64,
}

/// pa9 positive-read census slot: which open file description is
/// actually RETURNING data, per comm. pa3-pa8 proved hilogd.rd_kmsg's
/// 92K reads/s never touch /dev/kmsg and never return EOF/EAGAIN —
/// they return data, from an unknown fd. Keyed by comm only (pa7
/// lesson: anything finer lets init's config reads starve the table
/// before the spinner starts); the slot FOLLOWS the comm's current
/// open-file-description (`file_id`, never reused — R36-B1). A read on
/// the tracked description costs one u64 compare + increments; a read
/// on a different one retargets the slot (one dentry-path resolve +
/// the ops-table pointer, symbolized host-side via nm). At dump time a
/// spin loop shows one description with a huge n_cur.
pub struct RdDataSlot {
    /// First 8 comm bytes as one u64 — fast scan pre-filter (comm
    /// compare is the per-read hot path; "hilogd.s*" vs "hilogd.r*"
    /// differ here, and a filter miss is one load).
    pub comm0: AtomicU64,
    pub comm: [AtomicU8; 16],
    pub file_id: AtomicU64,
    pub fd: AtomicU64,
    pub ops: AtomicU64,
    pub n_cur: AtomicU64,
    pub n_total: AtomicU64,
    pub bytes: AtomicU64,
    pub path: [AtomicU8; 96],
}

/// pa10 failing-read census: (comm, errno, fd-resolution) of reads that
/// return a negative errno other than EAGAIN. pa9 closed the remaining
/// gap: hilogd.rd_kmsg issues 15.4M reads/boot that return neither data
/// (positive-read census: 195 reads total) nor EOF/EAGAIN (censuses
/// fire and list other comms) — the only shape left is a failing errno,
/// and WHICH errno (EBADF from a closed/write-only fd? EFAULT?) decides
/// kernel-fix vs OH-behavior routing. Once per comm.
pub struct RdFailSlot {
    pub comm0: AtomicU64,
    pub comm: [AtomicU8; 16],
    pub ret: AtomicI64,
    pub n: AtomicU64,
    /// 1 = get_file_fd(fd) resolved at census time (fd open), 0 = it
    /// did not (fd closed), 2 = resolved and file is O_WRONLY (read
    /// mode check would EBADF before any file-layer read).
    pub fd_state: AtomicU8,
    pub fd: AtomicU64,
    pub ops: AtomicU64,
    pub path: [AtomicU8; 96],
}

pub(super) const ZSTK_DEPTH: usize = 4;

pub struct RestSite {
    /// `Location as *const () as u64`; 0 = free slot.
    pub loc: AtomicU64,
    /// Opens charged to this site.
    pub n: AtomicU64,
    /// Closed span (raw TSC) charged to this site.
    pub cyc: AtomicU64,
    pub max: AtomicU64,
}

pub struct If0Zone {
    /// Our model of IF while the probe hooks are live (false = enabled).
    pub if0: [AtomicU8; crate::config::MAX_CPUS],
    /// Raw-TSC stamp of the currently open IF=0 segment (valid if if0).
    pub t0: [AtomicU64; crate::config::MAX_CPUS],
    /// Zone stack (nesting), ZD = current depth.
    pub zstk: [[AtomicU8; ZSTK_DEPTH]; crate::config::MAX_CPUS],
    pub zd: [AtomicU8; crate::config::MAX_CPUS],
    /// Cumulative raw TSC per zone per CPU.
    pub cum: [[AtomicU64; 4]; crate::config::MAX_CPUS],
    /// Number of IF=0 segments closed per zone (window-count shape).
    pub nseg: [[AtomicU64; 4]; crate::config::MAX_CPUS],
    /// Longest closed segment per zone (raw TSC).
    pub seg_max: [[AtomicU64; 4]; crate::config::MAX_CPUS],
    /// REST_SITES row (or SLOT_*) the open segment charges.
    pub slot: [AtomicU8; crate::config::MAX_CPUS],
    /// v3 self-heal evidence: modeled-open spans closed on IF=1 evidence.
    pub heal_n: [AtomicU64; crate::config::MAX_CPUS],
    pub heal_cyc: [AtomicU64; crate::config::MAX_CPUS],
    /// v3 missed-edge counter: disable seen with real IF=0, model closed.
    pub miss_n: [AtomicU64; crate::config::MAX_CPUS],
}

pub const UART_HIST: usize = 40;

pub struct UartStat {
    pub n: AtomicU64,
    pub cyc: AtomicU64,
    pub max: AtomicU64,
    pub polls: AtomicU64,
    pub polls_gt1: AtomicU64,
    pub hist: [AtomicU64; UART_HIST],
}

pub struct PrSlot {
    pub comm0: AtomicU64,
    pub comm: [AtomicU8; 16],
    pub recs: AtomicU64,
    pub ring_bytes: AtomicU64,
    pub con_bytes: AtomicU64,
    pub con_if0_bytes: AtomicU64,
}

pub struct RxSlot {
    pub comm0: AtomicU64,
    pub comm: [AtomicU8; 16],
    pub nr: AtomicU64,
    pub fd: AtomicU64,
    pub file_id: AtomicU64,
    pub flags: AtomicI64,
    pub nonblock: AtomicU8,
    pub n_total: AtomicU64,
    pub n_cur: AtomicU64,
    pub n_data: AtomicU64,
    pub data_bytes: AtomicU64,
    pub n_eagain: AtomicU64,
    pub n_zero: AtomicU64,
    pub err: AtomicI64,
    pub n_err: AtomicU64,
    pub ops: AtomicU64,
    pub desc: [AtomicU8; 96],
}

pub struct ErrSlot {
    pub nr: AtomicU64,
    pub err: AtomicI64,
    pub n: AtomicU64,
    /// First-seen boot time, ms (only written on cell creation).
    pub first_ms: AtomicU64,
    /// First-seen comm (16 bytes, NUL-padded).
    pub comm: [AtomicU8; 16],
    /// First-seen a0 (fd or path pointer, depending on the syscall).
    pub a0: AtomicU64,
    /// First-seen PATH STRING (NUL-padded snapshot; only meaningful for
    /// path-taking syscalls — the pointer is read ONCE at cell creation).
    pub path: [AtomicU8; 64],
}

pub struct CommCount {
    pub comm: [AtomicU8; 16],
    pub n: AtomicU64,
}

// ---------------------------------------------------------------------------
// the panic-store record header, cell and writer head
// ---------------------------------------------------------------------------

/// The record header, 32 bytes, at the region start.
#[repr(C)]
pub(crate) struct RecordHeader {
    pub(super) magic: u64,
    pub(super) version: u32,
    /// Payload length in bytes (after this header).
    pub(super) length: u32,
    /// jiffies at the capture (the record's timestamp).
    pub(super) jiffies: u32,
    /// Record kind: 0 = console (the panic report, exposed as
    /// console-ramoops-0), 1 = dmesg (the oops report, exposed as
    /// dmesg-ramoops-0) — the reference pstore's two ramoops flavors.
    pub(super) kind: u32,
    pub(super) _reserved: [u32; 2],
}

/// A byte cell the panic sink can write without any lock (atomics only;
/// `AtomicU8` directly would also do, this keeps the array decl short).
pub(crate) struct AtomicU8Cell(pub(super) core::sync::atomic::AtomicU8);

/// Stack-buffer fmt sink for the record head (no allocation).
pub(crate) struct HeadWriter {
    pub(super) buf: [u8; 512],
    pub(super) len: usize,
}

// ---------------------------------------------------------------------------
// the kernel-fault kind enum of the oops report
// ---------------------------------------------------------------------------

/// What class of kernel fault produced the oops — selects the head
/// line's wording (the reference derives the same distinction from the
/// trap cause / fault address).
#[derive(Clone, Copy)]
pub enum Kind {
    /// A kernel page fault with no fixup entry. `addr` chooses the
    /// reference's wording: below one page it is a NULL pointer
    /// dereference, above it a paging request.
    PageFault { addr: u64 },
    /// Kernel-mode illegal instruction (no fixup class).
    IllegalInstruction,
    /// A kernel-mode exception the arch has no specific handler for
    /// (`cause` is the arch's trap-cause word, riscv64 scause).
    UnknownException { cause: u64 },
}

// ---------------------------------------------------------------------------
// the /dev/kmsg sender row
// ---------------------------------------------------------------------------

pub(super) const COMM: usize = 16;

/// The /dev/kmsg writer row (read by the crate-side dump through this
/// pub surface — the kmsg marker stayed framework-side with printk's
/// call site).
pub struct KmsgSender {
    pub pid: AtomicU32,
    pub n: AtomicU64,
    pub comm: [AtomicU8; COMM],
    /// Last line head (raw, ≤32 bytes) — the repeating-pattern sample.
    pub head: [AtomicU8; 32],
    pub head_len: AtomicU32,
}

// ---------------------------------------------------------------------------
// the per-task epoll-spinner watch row
// ---------------------------------------------------------------------------

/// Per-task watch state, direct-mapped by pid (512 slots — attribution
/// fidelity at census granularity, the hilogcensus trade-off).
pub(super) struct Watch {
    pub(super) pid: AtomicU32,
    /// zero-event returns in the current window.
    pub(super) n: AtomicU32,
    /// jiffies when the window opened.
    pub(super) win: AtomicU64,
    /// jiffies when this task last REPORTED (rate limit).
    pub(super) last_rep: AtomicU64,
}

// ---------------------------------------------------------------------------
// the leak-origin probe slot
// ---------------------------------------------------------------------------

pub(crate) struct LoSlot {
    pub(super) task: AtomicUsize,
    /// `&'static Location` of the most recent PREEMPT_OFFSET acquire.
    pub(super) site: AtomicUsize,
    /// jiffies at that acquire.
    pub(super) jif: AtomicU64,
    /// preempt_count value immediately AFTER that acquire (1 = outermost).
    pub(super) cnt_after: AtomicI32,
    /// `&'static Location` of the most recent HARDIRQ_OFFSET acquire
    /// (s14: the stranded-0x10000 face — pc=0x10000/0x10001 incumbents
    /// and the 0x10000-grid strand family). With `irq_enter` marked
    /// #[track_caller] this names the trap dispatch site (timer/soft/
    /// ext), so a leaked hardirq bit reports WHICH bracket booked it.
    pub(super) hsite: AtomicUsize,
    /// jiffies at that HARDIRQ acquire.
    pub(super) hjif: AtomicU64,
    // ---- phantom-bits (2026-11-01): SOFTIRQ-bit shadow ----
    // The monopolist face of the eighth successor's lb60 dump (pid 552,
    // preempt=0x100, RUNNING, CFS-undefeatable) strands exactly ONE
    // SOFTIRQ_OFFSET whose release never executed anywhere (zero
    // sub=0x100 underflows all boot; SOFTIRQ-BRACKET-LEAK silent). Only
    // two +0x100 bookers exist on this tree (__do_softirq's bracket and
    // SeqLock::write's stats sections) — this shadow names which one
    // booked a stranded bit, and the tick-side orphan scan
    /// (preempt::tick_orphan_scan) reports it while the bit sits.
    /// `&'static Location` of the most recent SOFTIRQ_OFFSET acquire.
    pub(super) ssite: AtomicUsize,
    /// jiffies at that SOFTIRQ acquire.
    pub(super) sjif: AtomicU64,
    /// CPU id where it ran.
    pub(super) scpu: AtomicU32,
    /// 1 = at acquire time this CPU's sched slot named a DIFFERENT task
    /// (booking resolved through tp; the slot is the heal's observer —
    /// divergence means the heal watches the wrong task for this bit).
    pub(super) smis: AtomicU32,
    /// One report per leak episode (cleared on each new acquire).
    pub(super) reported: AtomicBool,
    // ---- preempt-hunt (2026-11-01): OUTERMOST acquire tap ----
    // The `site` shadow above is last-acquire-wins: after a leak, any
    // correctly-paired inner guard pair (1->2->1) overwrites it, so the
    // reported site is dominated by the hottest lock family (the four
    // allocator sites) even when the leak's acquire was elsewhere. The
    // OUTERmost acquire (base-field 0->1) whose matching 1->0 release
    // never ran is, by construction, the leak's origin: the base count
    // is still nonzero precisely because THAT release never executed.
    // Inner pairs (1->2->1) do not touch these fields, so the record is
    // stable from the leaking acquire until the count returns to zero
    // (or a heal zeroes it — at which point the WAKE-HEAL report prints
    // this record and names the producer's home site directly).
    /// Task that executed the outermost base acquire (0 = none).
    pub(super) otask: AtomicUsize,
    /// `&'static Location` of that outermost acquire.
    pub(super) osite: AtomicUsize,
    /// jiffies at that acquire.
    pub(super) ojif: AtomicU64,
    /// CPU id where it ran.
    pub(super) ocpu: AtomicU32,
    /// 1 = at acquire time this CPU's sched slot named a DIFFERENT task
    /// (tp vs slot divergence — the stale-tp / mid-switch window family).
    pub(super) omis: AtomicU32,
    /// When omis=1: the task pointer the sched slot named at acquire
    /// time (0 = unreadable). Identifies WHICH window the divergent
    /// booking ran in (preempt-hunt).
    pub(super) oslot: AtomicUsize,
    /// jiffies of the last user-return report for this slot
    /// (preempt-hunt rate gate: one stuck +1 re-reports on every guard
    /// re-acquire; 50-jiffy spacing keeps the episode signature without
    /// flooding the budget).
    pub(super) urrep_jif: AtomicU64,
}

// ---------------------------------------------------------------------------
// the last-open path stash slot
// ---------------------------------------------------------------------------

pub(super) const CAP: usize = 128;

pub(crate) struct Slot {
    pub(super) tid: AtomicU32,
    pub(super) op: AtomicU32,
    pub(super) len: AtomicU32,
    pub(super) buf: [u8; CAP],
}

// ---------------------------------------------------------------------------
// the slow-kernel-entry note row
// ---------------------------------------------------------------------------

/// One tracking slot: the entry tuple and when it was first/last seen.
pub(super) struct SlowtaskSlot {
    pub(super) pid: u32,
    pub(super) sys: u64,
    pub(super) epc: u64,
    pub(super) state: u32,
    pub(super) first_j: u64,
    pub(super) last_j: u64,
    pub(super) printed_j: u64,
}

// ---------------------------------------------------------------------------
// the fork/exec/exit phase event and stat rows
// ---------------------------------------------------------------------------

pub(super) struct TEv {
    pub(super) tsc: AtomicU64,
    pub(super) site: AtomicU8,
    pub(super) pid: AtomicU32,
}

pub(super) struct ForkPhaseStat {
    pub(super) count: AtomicU64,
    pub(super) cyc: AtomicU64,
    pub(super) max: AtomicU64,
}

// ---------------------------------------------------------------------------
// the mmap/mprotect/munmap phase stat and comm rows
// ---------------------------------------------------------------------------

pub struct PhaseStat {
    pub count: AtomicU64,
    pub cyc: AtomicU64,
    pub max: AtomicU64,
}

pub struct CommEnt {
    pub comm: [AtomicU8; 16],
    pub n: AtomicU64,
}

// ---------------------------------------------------------------------------
// the reaper forensics name and exit records
// ---------------------------------------------------------------------------

/// PUB write/peek surface (crate-flip rung debug): the debug crate's
/// SPAWN/ZEXIT arms lock and write these maps; the ZLINGER scan below
/// reads EXIT_MAP under the same lock.
#[derive(Clone, Copy)]
pub struct NameRec {
    pub pid: u32,
    pub t_set: u64,
}

#[derive(Clone, Copy)]
pub struct ExitRec {
    pub pid: u32,
    pub t_exit: u64,
    pub code: i32,
    pub last_report: u64,
}
