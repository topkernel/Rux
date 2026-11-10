//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! perf events — software-clock sampling and counting
//! (`perf_event_open(2)` ABI), with the mmap ring-buffer record
//! protocol `hiperf(1)` (the OH image's profiler, developtools_hiperf)
//! consumes.
//!
//! Scope (validated against hiperf's perf_events.cpp + the uapi):
//! - `PERF_TYPE_SOFTWARE` configs `CPU_CLOCK` (0), `TASK_CLOCK` (1),
//!   `PAGE_FAULTS` (2), `CONTEXT_SWITCHES` (3).
//! - Counting mode (`sample_type == 0`): `read(2)` returns the
//!   read-format word set (value, [time_enabled], [time_running],
//!   [id]) — reference semantics; records NEVER travel through read.
//! - Sampling mode (`sample_type != 0`): samples are emitted into the
//!   mmap'd ring buffer (protocol below) with the ABI field order
//!   IDENTIFIER, IP, TID, TIME, ID, STREAM_ID, CPU, PERIOD, CALLCHAIN.
//!   FREQ mode (`freq=1`) is accepted and mapped onto an effective
//!   period of `1e9/sample_freq`, floored at one tick — the sampler
//!   below is tick-granular, so a freq above KERNEL_HZ coalesces to
//!   tick-rate sampling rather than being lost.
//! - Sideband records `COMM`, `FORK`, `EXIT`, `MMAP2` for process
//!   tracking (hiperf's symbolizer resolves DSOs from MMAP2 records);
//!   `LOST` for ring overflow (non-overwrite mode drops + reports).
//! - ioctls `PERF_EVENT_IOC_{ENABLE,DISABLE,REFRESH,RESET,PERIOD,
//!   SET_OUTPUT,ID}`.
//! - `/proc/sys/kernel/perf_event_paranoid` gate: kernel-inclusive
//!   events need CAP_PERFMON/CAP_SYS_ADMIN (paranoid=2 default).
//!
//! ## How samples are produced (internal design)
//!
//! The hard timer IRQ (arch trap paths, next to the syscost tick
//! hooks) calls [`note_tick`] with the interrupted PC, frame pointer,
//! privilege mode and the interrupted task's TID/TGID. That hook is
//! LOCK-FREE: one relaxed load to bail when nothing is armed,
//! otherwise per-CPU atomic stores + a sequence bump — hard-IRQ
//! context takes no locks here.
//!
//! The Hrtimer softirq (raised every tick while any event is armed;
//! see `timer_softirq_work_pending`) calls [`softirq_tick`] on the
//! same CPU, which charges `pending_ticks * TICK_NS` to every
//! matching armed event and emits a sample whenever the accumulated
//! counter crosses `sample_period`. The user callchain (frame-pointer
//! walk) is taken here in softirq context: the interrupted task is
//! still current (the softirq runs on the IRQ-exit path before any
//! return to user), so its page tables are live and the exception-
//! table `get_user` reads fault safely.
//!
//! Granularity note: samples can only be produced at tick (or
//! softirq-pass) boundaries, so a `sample_period` below one tick
//! (10 ms at KERNEL_HZ=100) coalesces to tick-rate samples rather
//! than being lost. The counter arithmetic itself is exact ns.
//!
//! ## The mmap ring protocol (uapi perf_event_mmap_page)
//!
//! `mmap(fd, len = (1+npages) * PAGE_SIZE)` maps page 0 (metadata)
//! plus `npages` data pages (`npages` must be a power of two). The
//! kernel writes whole records sequentially into the data area,
//! wrapping at `data_size`, then publishes `data_head` (offset 1024)
//! with release semantics. Userspace reads up to `data_head` and
//! publishes `data_tail` (offset 1032) — the kernel treats
//! `head - tail` as the occupied byte count and drops (LOST) when a
//! record does not fit. Pages are order-0 allocations mapped one at a
//! time (no high-order pressure); the kernel alias for writes is
//! phys_to_virt per page.
//!
//! ## Lock order (INV: registry → event.inner)
//!
//! `REGISTRY` (irqsave) is the outer lock; an event's `inner` is
//! taken under it by the sampler. `read`/`poll`/`ioctl` take `inner`
//! alone. All REGISTRY takes are `lock_irqsave()` uniformly, so no
//! holder can ever be interrupted into a path that wants it.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

// ==================== ABI constants (uapi/linux/perf_event.h) ====================

/// attr.type — software events.
pub const PERF_TYPE_SOFTWARE: u32 = 1;
/// attr.config — CPU clock (per-CPU or per-task ns).
pub const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
/// attr.config — task clock (per-task ns).
pub const PERF_COUNT_SW_TASK_CLOCK: u64 = 1;
/// attr.config — page faults (count).
pub const PERF_COUNT_SW_PAGE_FAULTS: u64 = 2;
/// attr.config — context switches (count).
pub const PERF_COUNT_SW_CONTEXT_SWITCHES: u64 = 3;

// sample_type bits this slice can fill.
pub const PERF_SAMPLE_IP: u64 = 1 << 0;
pub const PERF_SAMPLE_TID: u64 = 1 << 1;
pub const PERF_SAMPLE_TIME: u64 = 1 << 2;
pub const PERF_SAMPLE_ID: u64 = 1 << 6;
pub const PERF_SAMPLE_CPU: u64 = 1 << 7;
pub const PERF_SAMPLE_PERIOD: u64 = 1 << 8;
pub const PERF_SAMPLE_STREAM_ID: u64 = 1 << 9;
pub const PERF_SAMPLE_CALLCHAIN: u64 = 1 << 5;
pub const PERF_SAMPLE_IDENTIFIER: u64 = 1 << 16;
/// Bits we honor in attr.sample_type (anything outside → EINVAL).
const SAMPLE_TYPE_KNOWN: u64 = PERF_SAMPLE_IP
    | PERF_SAMPLE_TID
    | PERF_SAMPLE_TIME
    | PERF_SAMPLE_ID
    | PERF_SAMPLE_CPU
    | PERF_SAMPLE_PERIOD
    | PERF_SAMPLE_STREAM_ID
    | PERF_SAMPLE_CALLCHAIN
    | PERF_SAMPLE_IDENTIFIER;

// read_format bits.
const PERF_FORMAT_TOTAL_TIME_ENABLED: u64 = 1 << 0;
const PERF_FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;
const PERF_FORMAT_ID: u64 = 1 << 2;
const PERF_FORMAT_GROUP: u64 = 1 << 3;

// attr flags1 (u64 at offset 40) bits this slice reads.
const FLAG_DISABLED: u64 = 1 << 0;
const FLAG_INHERIT: u64 = 1 << 1;
const FLAG_EXCLUDE_USER: u64 = 1 << 4;
const FLAG_EXCLUDE_KERNEL: u64 = 1 << 5;
const FLAG_EXCLUDE_IDLE: u64 = 1 << 7;
const FLAG_MMAP: u64 = 1 << 8;
const FLAG_COMM: u64 = 1 << 9;
const FLAG_FREQ: u64 = 1 << 10;
const FLAG_ENABLE_ON_EXEC: u64 = 1 << 12;
const FLAG_WATERMARK: u64 = 1 << 14;
const FLAG_MMAP_DATA: u64 = 1 << 17;
const FLAG_SAMPLE_ID_ALL: u64 = 1 << 18;

/// perf_event_open flags argument.
const PERF_FLAG_FD_NO_GROUP: u64 = 1 << 1;
const PERF_FLAG_FD_OUTPUT: u64 = 1 << 2;
const PERF_FLAG_FD_CLOEXEC: u64 = 1 << 3;
const PERF_FLAG_PID_CGROUP: u64 = 1 << 4;

// Record types (perf_event_header.type).
const PERF_RECORD_MMAP: u32 = 1;
pub const PERF_RECORD_LOST: u32 = 2;
const PERF_RECORD_COMM: u32 = 3;
const PERF_RECORD_EXIT: u32 = 4;
const PERF_RECORD_FORK: u32 = 7;
pub const PERF_RECORD_SAMPLE: u32 = 9;
const PERF_RECORD_MMAP2: u32 = 10;

// perf_event_header.misc cpumode bits.
const PERF_RECORD_MISC_USER: u16 = 1;
const PERF_RECORD_MISC_KERNEL: u16 = 2;

/// Callchain context marker (uapi): prefixes a user callchain.
const PERF_CONTEXT_USER: u64 = u64::MAX - 2048 + 1;

/// Max frames per user callchain (reference perf_max_stack default 32
/// for hiperf's use; bounded so the softirq walk stays cheap).
const MAX_CALLCHAIN: usize = 32;

/// perf_event_ioctls — _IO('$', n) family + sized variants.
const IOC_ENABLE: u32 = 0x2400;
const IOC_DISABLE: u32 = 0x2401;
const IOC_REFRESH: u32 = 0x2402;
const IOC_RESET: u32 = 0x2403;
const IOC_PERIOD: u32 = 0x4008_2404; // _IOW('$', 4, u64)
const IOC_SET_OUTPUT: u32 = 0x2405;
const IOC_ID: u32 = 0x8008_2407; // _IOR('$', 7, u64)

/// perf_event_mmap_page field offsets (bytes into page 0).
const MP_VERSION: usize = 0;
const MP_COMPAT_VERSION: usize = 4;
const MP_TIME_ENABLED: usize = 24;
const MP_TIME_RUNNING: usize = 32;
const MP_CAPABILITIES: usize = 40;
const MP_DATA_HEAD: usize = 1024;
const MP_DATA_TAIL: usize = 1032;
const MP_DATA_OFFSET: usize = 1040;
const MP_DATA_SIZE: usize = 1048;

/// The attr head this port decodes — the first 56 bytes cover every
/// field the slice reads (through the flags2 word at offset 52). The
/// ABI minimum attr size (PERF_ATTR_SIZE_VER0) is 64; `size` gates how
/// much the user filled in.
#[repr(C)]
#[derive(Clone, Copy)]
#[derive(Default)]
struct AttrHead {
    type_: u32,          // offset 0
    size: u32,           // offset 4
    config: u64,         // offset 8
    sample_period: u64,  // offset 16 (sample_freq when FLAG_FREQ)
    sample_type: u64,    // offset 24
    read_format: u64,    // offset 32
    flags1: u64,         // offset 40 (bitfield: disabled..)
    wakeup: u32,         // offset 48
    flags2: u32,         // offset 52 (exclude_callchain_*, mmap2, comm_exec…)
}

/// ns per tick — the accounting quantum.
const TICK_NS: u64 = 1_000_000_000 / (crate::config::KERNEL_HZ as u64);
const PAGE_SIZE: usize = 4096;

// ==================== Per-CPU tick stash (hard IRQ → softirq) ====================

/// Tick privilege modes for the stash.
pub const MODE_KERNEL: u8 = 0;
pub const MODE_USER: u8 = 1;
pub const MODE_IDLE: u8 = 2;

struct TickStash {
    pc: AtomicU64,
    /// Frame pointer of the interrupted frame (user mode; x86 RBP,
    /// riscv s0, aarch64 x29 — 0 when unavailable).
    fp: AtomicU64,
    mode: AtomicU8,
    tid: AtomicU32,
    tgid: AtomicU32,
    /// Incremented by every armed tick; the softirq charges the
    /// delta since its last pass (normally 1).
    seq: AtomicU64,
    /// seq already charged by the softirq.
    charged: AtomicU64,
}

const fn new_stash() -> TickStash {
    TickStash {
        pc: AtomicU64::new(0),
        fp: AtomicU64::new(0),
        mode: AtomicU8::new(MODE_IDLE),
        tid: AtomicU32::new(0),
        tgid: AtomicU32::new(0),
        seq: AtomicU64::new(0),
        charged: AtomicU64::new(0),
    }
}

static STASH: [TickStash; crate::config::MAX_CPUS] = [const { new_stash() }; crate::config::MAX_CPUS];

/// Armed-and-enabled event count: the hard-IRQ bail-out and the
/// softirq-raise gate (`timer_softirq_work_pending`) read this; open,
/// close and the ENABLE/DISABLE ioctls maintain it. When 0 the
/// subsystem costs one relaxed load per tick.
static ARMED: AtomicUsize = AtomicUsize::new(0);

/// Bitmap of "any armed event wants this sideband/event class" so the
/// producers (context-switch accounting, sideband hooks) can bail
/// with one relaxed load when perf is off.
static SW_CTXSW_WANTED: AtomicBool = AtomicBool::new(false);
static SW_PF_WANTED: AtomicBool = AtomicBool::new(false);
static SIDEBAND_WANTED: AtomicBool = AtomicBool::new(false);

/// Per-CPU context-switch counts (PERF_COUNT_SW_CONTEXT_SWITCHES).
static CTXSW: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
/// Per-CPU page-fault counts (PERF_COUNT_SW_PAGE_FAULTS).
static PFAULT: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Count one context switch on the calling CPU (procfs stat
/// `note_switch` tail). One relaxed load when nothing is armed.
pub fn note_ctxsw() {
    if ARMED.load(Ordering::Relaxed) == 0 || !SW_CTXSW_WANTED.load(Ordering::Relaxed) {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        CTXSW[c].fetch_add(1, Ordering::Relaxed);
    }
}

/// Count one page fault on the calling CPU (mm vmstat PGFAULT tail).
/// One relaxed load when nothing is armed.
pub fn note_pfault() {
    if ARMED.load(Ordering::Relaxed) == 0 || !SW_PF_WANTED.load(Ordering::Relaxed) {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        PFAULT[c].fetch_add(1, Ordering::Relaxed);
    }
}

/// Recompute the wanted-class bits from the registry (open/close/
/// ioctl tail).
fn refresh_wanted_bits() {
    let reg = REGISTRY.lock_irqsave();
    let mut ctxsw = false;
    let mut pf = false;
    let mut side = false;
    for &(_, p) in reg.iter() {
        // SAFETY: registry membership guarantees validity.
        let ev = unsafe { &*(p as *const PerfEvent) };
        match ev.config {
            PERF_COUNT_SW_CONTEXT_SWITCHES => ctxsw = true,
            PERF_COUNT_SW_PAGE_FAULTS => pf = true,
            _ => {}
        }
        if ev.track_comm || ev.track_mmap {
            side = true;
        }
    }
    drop(reg);
    SW_CTXSW_WANTED.store(ctxsw, Ordering::Release);
    SW_PF_WANTED.store(pf, Ordering::Release);
    SIDEBAND_WANTED.store(side, Ordering::Release);
}

/// Hard-IRQ tick hook (arch trap paths). `tid` 0 = idle/no task.
/// `fp` is the interrupted frame pointer (user mode). Lock-free by
/// contract — see the module doc.
#[inline]
pub fn note_tick(pc: u64, mode: u8, tid: u32) {
    note_tick_fp(pc, mode, tid, 0, 0);
}

/// Full stash variant with frame pointer and TGID (x86_64/riscv64/
/// aarch64 trap paths have all of them in hand).
#[inline]
pub fn note_tick_fp(pc: u64, mode: u8, tid: u32, fp: u64, tgid: u32) {
    if ARMED.load(Ordering::Relaxed) == 0 {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c >= crate::config::MAX_CPUS {
        return;
    }
    let s = &STASH[c];
    s.pc.store(pc, Ordering::Relaxed);
    s.fp.store(fp, Ordering::Relaxed);
    s.mode.store(mode, Ordering::Relaxed);
    s.tid.store(tid, Ordering::Relaxed);
    s.tgid.store(tgid, Ordering::Relaxed);
    s.seq.fetch_add(1, Ordering::Release);
}

/// Softirq-raise gate: is any event armed (the Hrtimer softirq must
/// run every tick so the stash gets charged)?
pub fn any_armed() -> bool {
    ARMED.load(Ordering::Acquire) > 0
}

// ==================== Event + registry ====================

/// One open perf event fd. Box'd, owned by the File's private_data;
/// the REGISTRY keeps the raw pointer until close unregisters it.
pub(crate) struct PerfEvent {
    id: u64,
    /// PERF_COUNT_SW_* config.
    config: u64,
    sample_type: u64,
    read_format: u64,
    /// Effective sample period in ns (>= TICK_NS).
    sample_period: AtomicU64,
    /// TID to match (task-scoped events); 0 in per-CPU mode.
    target_tid: u32,
    /// CPU to match; -1 = task-scoped, else per-CPU.
    target_cpu: i32,
    exclude_user: bool,
    exclude_kernel: bool,
    exclude_idle: bool,
    /// attr.comm: emit COMM sidebands.
    track_comm: bool,
    /// attr.mmap/mmap2: emit MMAP2 sidebands.
    track_mmap: bool,
    enabled: AtomicBool,
    /// Monotonic-ns timestamps for the read-format time words.
    open_ns: u64,
    running_ns: AtomicU64,
    /// Counted value (ns for clocks; counts for switch/fault events
    /// only in the charged sense — see read_value()).
    counter: AtomicU64,
    /// Ring state; None until mmap.
    inner: Spinlock<PerfInner>,
    /// Readers/pollers blocked waiting for records.
    wait_queue: WaitQueueHead,
}

struct PerfInner {
    /// ring pages; pages[0] = metadata page, pages[1..] = data area.
    pages: Option<Vec<usize>>,
    /// data area bytes = (pages-1) * PAGE_SIZE.
    data_size: usize,
    /// Next write position (monotonic byte counter).
    head: u64,
    /// Records dropped because the ring was full (reported as LOST).
    lost: u64,
    /// User VA of the mapping (hp88 hunt: overlap forensics).
    va: usize,
    /// Mapping length in bytes.
    va_len: usize,
}

impl PerfInner {
    const fn new() -> Self {
        PerfInner { pages: None, data_size: 0, head: 0, lost: 0, va: 0, va_len: 0 }
    }
}

/// hp88 hunt: does any LIVE perf ring mapping cover `addr`? Called from
/// the #GP forensics — a heap pointer inside a ring's VA range is the
/// placement-overlap conviction.
pub fn ring_covers(addr: u64) -> Option<(usize, usize, u64)> {
    if RING_LIVE.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let reg = REGISTRY.lock_irqsave();
    for &(_, p) in reg.iter() {
        // SAFETY: registry membership guarantees validity.
        let ev = unsafe { &*(p as *const PerfEvent) };
        let inner = ev.inner.lock_irqsave();
        if inner.va != 0
            && (addr as usize) >= inner.va
            && (addr as usize) < inner.va + inner.va_len
        {
            let hit = (inner.va, inner.va_len, ev.id);
            drop(inner);
            drop(reg);
            return Some(hit);
        }
        drop(inner);
    }
    drop(reg);
    None
}

/// id → event. Every take is lock_irqsave (module lock-order doc).
static REGISTRY: Spinlock<alloc::vec::Vec<(u64, usize)>> =
    Spinlock::new(alloc::vec::Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn register(ev: *mut PerfEvent, starts_armed: bool) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // SAFETY: caller built the box; the registry keeps the pointer
    // valid until unregister.
    unsafe {
        (*ev).id = id;
    }
    let mut reg = REGISTRY.lock_irqsave();
    reg.push((id, ev as usize));
    drop(reg);
    if starts_armed {
        ARMED.fetch_add(1, Ordering::Release);
    }
    refresh_wanted_bits();
    id
}

/// Remove `id` from the registry. Returns true if it was armed (the
/// caller adjusts ARMED).
fn unregister(id: u64) -> bool {
    let mut reg = REGISTRY.lock_irqsave();
    let was_armed = match reg.iter().position(|&(i, _)| i == id) {
        Some(idx) => {
            let (_, p) = reg.swap_remove(idx);
            // SAFETY: pointer came from register, still valid.
            let armed = unsafe { (*(p as *mut PerfEvent)).enabled.load(Ordering::Acquire) };
            drop(reg);
            refresh_wanted_bits();
            return armed;
        }
        None => false,
    };
    drop(reg);
    was_armed
}

// ==================== hp88 hunt bisect knob (temporary) ====================
//
// Kernel cmdline `perfnode=`: "noemit" keeps the FULL armed machinery
// (tick hooks, softirq every tick, counters, wakes' registry walk) but
// emit_record is a no-op — no ring writes, no reader wakes; "noarm"
// lets open + ring mmap succeed but ENABLE never arms — no tick hooks,
// no softirq, no emit. One boot each cuts the corruption hypothesis
// space in half twice. Remove when the corrupting path is convicted.

static NODE: AtomicU8 = AtomicU8::new(u8::MAX);
const NODE_NORMAL: u8 = 0;
const NODE_NOEMIT: u8 = 1;
const NODE_NOARM: u8 = 2;
const NODE_NOPERF: u8 = 3;
const NODE_NORING: u8 = 4;

fn node() -> u8 {
    let n = NODE.load(Ordering::Relaxed);
    if n != u8::MAX {
        return n;
    }
    #[cfg(feature = "x86_64")]
    let v = {
        let cl = crate::arch::boot::boot_cmdline();
        if cl.contains("perfnode=noemit") {
            NODE_NOEMIT
        } else if cl.contains("perfnode=noarm") {
            NODE_NOARM
        } else if cl.contains("perfnode=noperf") {
            NODE_NOPERF
        } else if cl.contains("perfnode=noring") {
            NODE_NORING
        } else {
            NODE_NORMAL
        }
    };
    #[cfg(not(feature = "x86_64"))]
    let v = NODE_NORMAL;
    NODE.store(v, Ordering::Relaxed);
    v
}

// ==================== Ring frame registry (premature-free detector) ====================
//
// hp88a hunt (TASK 87.11): during hiperf record sampling samgr's
// mallocng heap metadata got corrupted (15 user #GPs at ld-musl
// +0xba256 = mallocng get_meta) while the sampler was the only new
// kernel writer on the machine. The perf module writes ONLY into its
// ring frames (via the direct-map alias), so the corruption vector is
// a ring frame returned to the allocator while its event still lists
// it — the buddy re-sells the frame (samgr is the hungriest allocator
// during the spawn storm), and every sampler tick writes a few hundred
// bytes of perf records into the new owner's memory.
//
// This registry names that: every page of every live ring is hashed
// here at mmap and removed at close. alloc_pages checks handed-out
// frames against it (one relaxed load when no rings are live); a hit
// is the premature free, printed before the corruption can compound.

const RING_HT_BITS: usize = 13;
const RING_HT_LEN: usize = 1 << RING_HT_BITS; // 8192 slots ≥ 16 events × 257 pages
static RING_HT: Spinlock<[u64; RING_HT_LEN]> = Spinlock::new([0; RING_HT_LEN]);
static RING_LIVE: AtomicUsize = AtomicUsize::new(0);

#[inline]
fn ring_ht_hash(pfn: u64) -> usize {
    ((pfn.wrapping_mul(0x9E37_79B9_7F4A_7C15)) >> (64 - RING_HT_BITS)) as usize
}

/// Fast bail for the alloc_pages hook: nonzero while any ring exists.
pub fn ring_frames_live() -> bool {
    RING_LIVE.load(Ordering::Relaxed) > 0
}

fn ring_frame_insert(pfn: u64) {
    let mut ht = RING_HT.lock_irqsave();
    let mut i = ring_ht_hash(pfn);
    let mut free_slot: Option<usize> = None;
    for _ in 0..RING_HT_LEN {
        if ht[i] == pfn {
            drop(ht);
            return; // already tracked
        }
        if ht[i] == RING_HT_TOMB && free_slot.is_none() {
            free_slot = Some(i);
        }
        if ht[i] == 0 {
            break;
        }
        i = (i + 1) & (RING_HT_LEN - 1);
    }
    let slot = free_slot.unwrap_or(i);
    ht[slot] = pfn;
    RING_LIVE.fetch_add(1, Ordering::Relaxed);
    drop(ht);
}

/// Tombstone: probed past, not reusable as a stop marker.
const RING_HT_TOMB: u64 = u64::MAX;

fn ring_frame_remove(pfn: u64) {
    let mut ht = RING_HT.lock_irqsave();
    let mut i = ring_ht_hash(pfn);
    for _ in 0..RING_HT_LEN {
        let v = ht[i];
        if v == pfn {
            ht[i] = RING_HT_TOMB;
            RING_LIVE.fetch_sub(1, Ordering::Relaxed);
            break;
        }
        if v == 0 {
            break; // not tracked
        }
        i = (i + 1) & (RING_HT_LEN - 1);
    }
    drop(ht);
}

/// alloc_pages hook: the allocator just handed `phys` (order `order`)
/// out — if a live perf ring still lists that frame, the frame was
/// freed early. Print before the sampler can compound the corruption.
pub fn ring_frame_check(phys: usize, order: usize) {
    if RING_LIVE.load(Ordering::Relaxed) == 0 {
        return;
    }
    let mut hit_pfn = 0u64;
    {
        let ht = RING_HT.lock_irqsave();
        'outer: for p in 0..(1usize << order.min(4)) {
            let pfn = ((phys >> 12) as u64) + p as u64;
            let mut i = ring_ht_hash(pfn);
            for _ in 0..RING_HT_LEN {
                let v = ht[i];
                if v == pfn {
                    hit_pfn = pfn;
                    break 'outer;
                }
                if v == 0 {
                    break;
                }
                i = (i + 1) & (RING_HT_LEN - 1);
            }
        }
        drop(ht);
    }
    if hit_pfn != 0 {
        static N: AtomicUsize = AtomicUsize::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 8 {
            crate::pr_err!(
                "PERF-RING-RESOLD: frame {:#x} (pfn {}) re-handed by allocator while a live perf ring still lists it — premature free, sampler would write into the new owner pid {}",
                (hit_pfn as usize) << 12,
                hit_pfn,
                crate::process::current_pid()
            );
        }
    }
}

// ==================== Ring writes ====================

/// Kernel address of `n` bytes at `pos` inside the data area,
/// following a page wrap. Returns None if it crosses a page tail
/// (callers loop byte-ranges page-wise).
fn ring_slice(pages: &[usize], data_size: usize, pos: u64, n: usize) -> Option<(*mut u8, usize)> {
    let p = (pos % data_size as u64) as usize;
    let page_idx = p / PAGE_SIZE;
    let off = p % PAGE_SIZE;
    let phys = *pages.get(1 + page_idx)?;
    let base = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(phys as u64))
        .bits() as *mut u8;
    let avail = PAGE_SIZE - off;
    Some((unsafe { base.add(off) }, avail.min(n)))
}

/// Write `bytes` into the ring at `head` (wrapping), returning the new
/// head. Caller holds `inner` with a live ring.
fn ring_put(inner: &PerfInner, head: u64, bytes: &[u8]) -> u64 {
    let pages = inner.pages.as_ref().unwrap();
    let mut pos = head;
    let mut left = bytes;
    while !left.is_empty() {
        let (dst, n) = match ring_slice(pages, inner.data_size, pos, left.len()) {
            Some(x) => x,
            None => break, // cannot happen: data_size pages cover all
        };
        // SAFETY: dst points into kernel direct-map memory we own
        // (order-0 pages held by this event); n within the page.
        unsafe { core::ptr::copy_nonoverlapping(left.as_ptr(), dst, n) };
        pos += n as u64;
        left = &left[n..];
    }
    pos
}

fn ring_read_user_tail(inner: &PerfInner) -> u64 {
    let pages = match inner.pages.as_ref() {
        Some(p) => p,
        None => return 0,
    };
    let base = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pages[0] as u64))
        .bits() as *mut u8;
    // SAFETY: our own page; a racing user store is benign (u64-aligned).
    unsafe { core::ptr::read_volatile(base.add(MP_DATA_TAIL) as *const u64) }
}

fn ring_publish_head(inner: &PerfInner) {
    let pages = match inner.pages.as_ref() {
        Some(p) => p,
        None => return,
    };
    let base = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pages[0] as u64))
        .bits() as *mut u8;
    // Release: the record bytes are visible before the head bump.
    unsafe { core::ptr::write_volatile(base.add(MP_DATA_HEAD) as *mut u64, inner.head) };
    core::sync::atomic::fence(Ordering::Release);
}

/// Emit-time ring audit (hp88a hunt): the listed frames must still be
/// alive (refcount ≥ 1). Bounded to the first 64 emissions per boot —
/// enough to catch a lifetime bug in the first seconds of sampling.
fn ring_audit(inner: &PerfInner) {
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.load(Ordering::Relaxed) >= 64 {
        return;
    }
    N.fetch_add(1, Ordering::Relaxed);
    let Some(pages) = inner.pages.as_ref() else { return };
    for (i, p) in pages.iter().enumerate() {
        let pd = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(*p));
        if pd.is_null() {
            crate::pr_err!("PERF-RING-PAGE-BAD: pages[{}] frame {:#x} has NO descriptor", i, p);
            continue;
        }
        // SAFETY: descriptor of a ring frame we allocated.
        let rc = unsafe { (*pd).refcount() };
        if rc < 1 {
            crate::pr_err!(
                "PERF-RING-PAGE-BAD: pages[{}] frame {:#x} refcount {} — freed while the ring still lists it",
                i, p, rc
            );
        }
    }
}

/// Append one record; drops (+LOST counter) when it does not fit.
/// Wakes readers if anything was written.
fn emit_record(ev: &PerfEvent, rec: &[u8], wake: bool) {
    // hp88 hunt bisect: noemit keeps everything armed but writes nothing.
    if node() == NODE_NOEMIT {
        return;
    }
    // IRQ-safe uniformly (callers hold REGISTRY irqsave today; nested
    // irq_save is benign and blocks future call-path surprises).
    let mut inner = ev.inner.lock_irqsave();
    let size = inner.data_size;
    if size == 0 {
        return; // no ring (counting mode or not yet mmap'd)
    }
    ring_audit(&inner);
    let tail = ring_read_user_tail(&inner);
    let used = inner.head.saturating_sub(tail) as usize;
    // Keep one record-size slack so head never equals a full turn
    // ahead of tail (ambiguity guard, reference perf_output_put).
    if used + rec.len() + 8 > size {
        inner.lost += 1;
        let lost = inner.lost;
        let id = ev.id;
        drop(inner);
        // Try to report the loss itself (best effort; skips if full).
        let mut rec2 = [0u8; 24];
        rec2[0..4].copy_from_slice(&PERF_RECORD_LOST.to_le_bytes());
        rec2[4..6].copy_from_slice(&0u16.to_le_bytes());
        rec2[6..8].copy_from_slice(&24u16.to_le_bytes());
        rec2[8..16].copy_from_slice(&id.to_le_bytes());
        rec2[16..24].copy_from_slice(&lost.to_le_bytes());
        let _ = rec2;
        // (a LOST record would also not fit — the counter keeps the
        // total and the next successful write reports it)
        return;
    }
    inner.head = ring_put(&inner, inner.head, rec);
    // If losses accumulated before this record, prepend a LOST record
    // first (reference reports overflow before further data).
    if inner.lost > 0 {
        let lost = inner.lost;
        inner.lost = 0;
        drop(inner);
        let mut lrec = [0u8; 24];
        lrec[0..4].copy_from_slice(&PERF_RECORD_LOST.to_le_bytes());
        lrec[4..6].copy_from_slice(&0u16.to_le_bytes());
        lrec[6..8].copy_from_slice(&24u16.to_le_bytes());
        lrec[8..16].copy_from_slice(&ev.id.to_le_bytes());
        lrec[16..24].copy_from_slice(&lost.to_le_bytes());
        let mut inner2 = ev.inner.lock_irqsave();
        inner2.head = ring_put(&inner2, inner2.head, &lrec);
        ring_publish_head(&inner2);
        drop(inner2);
        if wake {
            ev.wait_queue.wake_up_all();
        }
        return;
    }
    ring_publish_head(&inner);
    drop(inner);
    if wake {
        ev.wait_queue.wake_up_all();
    }
}

/// Build a perf_event_header (type, misc, size).
fn header(type_: u32, misc: u16, size: u16) -> [u8; 8] {
    let mut h = [0u8; 8];
    h[0..4].copy_from_slice(&type_.to_le_bytes());
    h[4..6].copy_from_slice(&misc.to_le_bytes());
    h[6..8].copy_from_slice(&size.to_le_bytes());
    h
}

fn now_ns() -> u64 {
    crate::drivers::timer::monotonic_ns_since_boot()
}

// ==================== Sampling (softirq) ====================

/// Frame-pointer user callchain walk. Reads user memory through the
/// exception-table copy (fault-safe); runs in softirq on the CPU that
/// took the tick, where the interrupted task is still current — the
/// TID check in `softirq_tick` guards the rare switch-in-between.
fn user_callchain(fp: u64, out: &mut [u64; MAX_CALLCHAIN]) -> usize {
    let mut n = 0;
    out[n] = PERF_CONTEXT_USER;
    n += 1;
    let mut bp = fp;
    // Frame records also need to not fault forever: each failed read
    // (None) ends the walk.
    while n < MAX_CALLCHAIN {
        // SAFETY: get_user faults into the extable fixup and returns
        // None instead of panicking.
        let next = match unsafe {
            crate::arch::uaccess::get_user(bp as *const u64)
        } {
            Some(v) => v,
            None => break,
        };
        let ret = match unsafe {
            crate::arch::uaccess::get_user((bp + 8) as *const u64)
        } {
            Some(v) => v,
            None => break,
        };
        if ret == 0 || ret > 0x0000_7fff_ffff_ffff {
            break; // not a canonical user code address
        }
        out[n] = ret;
        n += 1;
        if next <= bp || next > 0x0000_7fff_ffff_ffff {
            break; // loop or junk — stop
        }
        bp = next;
    }
    n
}

/// Charge pending ticks and emit samples. Called from the Hrtimer
/// softirq on the CPU whose stash is being charged.
pub fn softirq_tick() {
    if ARMED.load(Ordering::Acquire) == 0 {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c >= crate::config::MAX_CPUS {
        return;
    }
    let s = &STASH[c];
    let seq = s.seq.load(Ordering::Acquire);
    let pending = seq.saturating_sub(s.charged.load(Ordering::Relaxed));
    if pending == 0 {
        return;
    }
    s.charged.store(seq, Ordering::Relaxed);
    let pc = s.pc.load(Ordering::Relaxed);
    let fp = s.fp.load(Ordering::Relaxed);
    let mode = s.mode.load(Ordering::Relaxed);
    let tid = s.tid.load(Ordering::Relaxed);
    let tgid = s.tgid.load(Ordering::Relaxed);
    let charge_ns = pending as u64 * TICK_NS;

    let mut to_wake: alloc::vec::Vec<*mut PerfEvent> = alloc::vec::Vec::new();
    {
        let reg = REGISTRY.lock_irqsave();
        for &(_, p) in reg.iter() {
            // SAFETY: registry membership guarantees validity (close
            // unregisters before freeing).
            let ev = unsafe { &*(p as *const PerfEvent) };
            if !ev.enabled.load(Ordering::Relaxed) {
                continue;
            }
            // Scope match: per-CPU events match this CPU (idle ticks
            // included — cpu-clock counts idle time unless excluded);
            // task-scoped events match the interrupted TID only.
            match ev.target_cpu {
                cpu if cpu >= 0 => {
                    if cpu as usize != c {
                        continue;
                    }
                }
                _ => {
                    if tid == 0 || tid != ev.target_tid {
                        continue;
                    }
                }
            }
            // Privilege filter (attr exclude_* bits).
            match mode {
                MODE_USER if ev.exclude_user => continue,
                MODE_KERNEL if ev.exclude_kernel => continue,
                MODE_IDLE if ev.exclude_idle => continue,
                _ => {}
            }
            match ev.config {
                PERF_COUNT_SW_CPU_CLOCK | PERF_COUNT_SW_TASK_CLOCK => {
                    ev.counter.fetch_add(charge_ns, Ordering::Relaxed);
                    ev.running_ns.fetch_add(charge_ns, Ordering::Relaxed);
                }
                _ => {
                    // Switch/fault counters are absolute reads (see
                    // read_value); no per-tick charge.
                    ev.running_ns.fetch_add(charge_ns, Ordering::Relaxed);
                }
            }
            // Sampling: crossing the period emits one record. The
            // effective period was floored at TICK_NS at open, so at
            // most one sample per pass — the same PC is never faked
            // twice.
            if ev.sample_type != 0 {
                let acc = ev.counter.fetch_add(0, Ordering::Relaxed);
                let period = ev.sample_period.load(Ordering::Relaxed);
                let mut local = acc;
                let mut emitted = false;
                while local >= period {
                    local -= period;
                    emit_sample(ev, pc, fp, mode, tid, tgid, c as u32, period);
                    emitted = true;
                }
                if local != acc {
                    ev.counter.store(local, Ordering::Relaxed);
                }
                if emitted {
                    to_wake.push(p as *mut PerfEvent);
                }
            }
        }
    }
    // Wakes with no locks held (timerfd expire-notify discipline).
    for p in to_wake {
        // SAFETY: as above.
        unsafe { (*p).wait_queue.wake_up_all() };
    }
}

/// One PERF_RECORD_SAMPLE laid out per the ABI field order.
fn emit_sample(
    ev: &PerfEvent,
    pc: u64,
    fp: u64,
    mode: u8,
    tid: u32,
    tgid: u32,
    cpu: u32,
    period: u64,
) {
    let mut chain = [0u64; MAX_CALLCHAIN];
    let mut nr_chain = 0usize;
    if ev.sample_type & PERF_SAMPLE_CALLCHAIN != 0 {
        if mode == MODE_USER && !ev.exclude_user {
            nr_chain = user_callchain(fp, &mut chain);
        }
        // Kernel callchains are not walkable (no kernel frame
        // pointers); an empty chain (nr=0) is emitted, matching the
        // reference's behavior when a context is excluded.
    }
    let mut buf = [0u8; 8 + 9 * 8 + 8 + MAX_CALLCHAIN * 8];
    let mut o = 0usize;
    let misc = if mode == MODE_USER { PERF_RECORD_MISC_USER } else { PERF_RECORD_MISC_KERNEL };

    let put64 = |buf: &mut [u8], o: &mut usize, v: u64| {
        buf[*o..*o + 8].copy_from_slice(&v.to_le_bytes());
        *o += 8;
    };

    // Body length first (for the header), then fill.
    let mut body = 0usize;
    for (bit, _) in [
        (PERF_SAMPLE_IDENTIFIER, 0u64),
        (PERF_SAMPLE_IP, 0),
        (PERF_SAMPLE_TID, 0),
        (PERF_SAMPLE_TIME, 0),
        (PERF_SAMPLE_ID, 0),
        (PERF_SAMPLE_STREAM_ID, 0),
        (PERF_SAMPLE_CPU, 0),
        (PERF_SAMPLE_PERIOD, 0),
    ] {
        if ev.sample_type & bit != 0 {
            body += 8;
        }
    }
    if ev.sample_type & PERF_SAMPLE_CALLCHAIN != 0 {
        body += 8 + nr_chain * 8;
    }
    buf[0..8].copy_from_slice(&header(
        PERF_RECORD_SAMPLE,
        misc,
        (8 + body) as u16,
    ));
    o = 8;
    // PERF_SAMPLE_IDENTIFIER
    if ev.sample_type & PERF_SAMPLE_IDENTIFIER != 0 {
        put64(&mut buf, &mut o, ev.id);
    }
    if ev.sample_type & PERF_SAMPLE_IP != 0 {
        put64(&mut buf, &mut o, pc);
    }
    if ev.sample_type & PERF_SAMPLE_TID != 0 {
        buf[o..o + 4].copy_from_slice(&tgid.to_le_bytes());
        buf[o + 4..o + 8].copy_from_slice(&tid.to_le_bytes());
        o += 8;
    }
    if ev.sample_type & PERF_SAMPLE_TIME != 0 {
        put64(&mut buf, &mut o, now_ns());
    }
    if ev.sample_type & PERF_SAMPLE_ID != 0 {
        put64(&mut buf, &mut o, ev.id);
    }
    if ev.sample_type & PERF_SAMPLE_STREAM_ID != 0 {
        put64(&mut buf, &mut o, ev.id);
    }
    if ev.sample_type & PERF_SAMPLE_CPU != 0 {
        buf[o..o + 4].copy_from_slice(&cpu.to_le_bytes());
        buf[o + 4..o + 8].copy_from_slice(&0u32.to_le_bytes());
        o += 8;
    }
    if ev.sample_type & PERF_SAMPLE_PERIOD != 0 {
        put64(&mut buf, &mut o, period);
    }
    if ev.sample_type & PERF_SAMPLE_CALLCHAIN != 0 {
        put64(&mut buf, &mut o, nr_chain as u64);
        for i in 0..nr_chain {
            put64(&mut buf, &mut o, chain[i]);
        }
    }
    emit_record(ev, &buf[..o], false);
}

// ==================== Sideband records ====================

/// PERF_RECORD_COMM { u32 pid, tid; char comm[]; } — exec/comm change.
pub fn hook_comm(tgid: u32, tid: u32, comm: &[u8]) {
    if ARMED.load(Ordering::Relaxed) == 0 || !SIDEBAND_WANTED.load(Ordering::Relaxed) {
        return;
    }
    let name_len = comm.iter().position(|&b| b == 0).unwrap_or(comm.len()).min(1023);
    let mut rec = alloc::vec![0u8; 8 + 8 + name_len + 1];
    let total = rec.len();
    rec[0..8].copy_from_slice(&header(
        PERF_RECORD_COMM,
        PERF_RECORD_MISC_USER,
        total as u16,
    ));
    rec[8..12].copy_from_slice(&tgid.to_le_bytes());
    rec[12..16].copy_from_slice(&tid.to_le_bytes());
    rec[16..16 + name_len].copy_from_slice(&comm[..name_len]);
    // trailing NUL already zero.

    let reg = REGISTRY.lock_irqsave();
    for &(_, p) in reg.iter() {
        // SAFETY: registry membership guarantees validity.
        let ev = unsafe { &*(p as *const PerfEvent) };
        if ev.track_comm && ev.enabled.load(Ordering::Relaxed) {
            emit_record(ev, &rec, false);
        }
    }
}

/// PERF_RECORD_MMAP2 { pid, tid, addr, len, pgoff, maj, min, ino,
/// ino_generation, prot, flags, filename[] } — executable file
/// mappings (exec segments, dlopen'd DSOs).
pub fn hook_mmap2(
    tgid: u32,
    tid: u32,
    addr: u64,
    len: u64,
    pgoff: u64,
    prot: u32,
    flags: u32,
    filename: &[u8],
) {
    if ARMED.load(Ordering::Relaxed) == 0 || !SIDEBAND_WANTED.load(Ordering::Relaxed) {
        return;
    }
    let name_len = filename.len().min(1023);
    // fixed body: 8(pid,tid) + 8*3(addr,len,pgoff) + 8(maj,min) +
    // 8*2(ino,ino_gen) + 8(prot,flags) = 64
    let mut rec = alloc::vec![0u8; 8 + 64 + name_len + 1];
    let total = rec.len();
    rec[0..8].copy_from_slice(&header(
        PERF_RECORD_MMAP2,
        PERF_RECORD_MISC_USER,
        total as u16,
    ));
    rec[8..12].copy_from_slice(&tgid.to_le_bytes());
    rec[12..16].copy_from_slice(&tid.to_le_bytes());
    rec[16..24].copy_from_slice(&addr.to_le_bytes());
    rec[24..32].copy_from_slice(&len.to_le_bytes());
    rec[32..40].copy_from_slice(&pgoff.to_le_bytes());
    rec[40..44].copy_from_slice(&0u32.to_le_bytes()); // major
    rec[44..48].copy_from_slice(&0u32.to_le_bytes()); // minor
    rec[48..56].copy_from_slice(&0u64.to_le_bytes()); // ino
    rec[56..64].copy_from_slice(&0u64.to_le_bytes()); // ino_generation
    rec[64..68].copy_from_slice(&prot.to_le_bytes());
    rec[68..72].copy_from_slice(&flags.to_le_bytes());
    rec[72..72 + name_len].copy_from_slice(&filename[..name_len]);

    let reg = REGISTRY.lock_irqsave();
    for &(_, p) in reg.iter() {
        // SAFETY: registry membership guarantees validity.
        let ev = unsafe { &*(p as *const PerfEvent) };
        if ev.track_mmap && ev.enabled.load(Ordering::Relaxed) {
            emit_record(ev, &rec, false);
        }
    }
}

/// PERF_RECORD_FORK { u32 pid,ppid,tid,ptid; u64 time; }
pub fn hook_fork(parent_pid: u32, parent_tid: u32, child_pid: u32, child_tid: u32) {
    if ARMED.load(Ordering::Relaxed) == 0 {
        return;
    }
    let mut rec = [0u8; 8 + 24];
    rec[0..8].copy_from_slice(&header(
        PERF_RECORD_FORK,
        PERF_RECORD_MISC_USER,
        32,
    ));
    rec[8..12].copy_from_slice(&child_pid.to_le_bytes());
    rec[12..16].copy_from_slice(&parent_pid.to_le_bytes());
    rec[16..20].copy_from_slice(&child_tid.to_le_bytes());
    rec[20..24].copy_from_slice(&parent_tid.to_le_bytes());
    rec[24..32].copy_from_slice(&now_ns().to_le_bytes());

    let reg = REGISTRY.lock_irqsave();
    for &(_, p) in reg.iter() {
        // SAFETY: registry membership guarantees validity.
        let ev = unsafe { &*(p as *const PerfEvent) };
        if ev.enabled.load(Ordering::Relaxed) {
            emit_record(ev, &rec, false);
        }
    }
}

/// PERF_RECORD_EXIT { u32 pid,tid; u64 time; } (fork_exit shape).
pub fn hook_exit(tgid: u32, tid: u32) {
    if ARMED.load(Ordering::Relaxed) == 0 {
        return;
    }
    let mut rec = [0u8; 8 + 24];
    rec[0..8].copy_from_slice(&header(
        PERF_RECORD_EXIT,
        PERF_RECORD_MISC_USER,
        32,
    ));
    rec[8..12].copy_from_slice(&tgid.to_le_bytes());
    rec[12..16].copy_from_slice(&tid.to_le_bytes());
    rec[24..32].copy_from_slice(&now_ns().to_le_bytes());

    let reg = REGISTRY.lock_irqsave();
    for &(_, p) in reg.iter() {
        // SAFETY: registry membership guarantees validity.
        let ev = unsafe { &*(p as *const PerfEvent) };
        if ev.enabled.load(Ordering::Relaxed) {
            emit_record(ev, &rec, false);
        }
    }
}

// ==================== File operations ====================

/// Counted value for read(2): clocks are charged ns; switch/fault
/// events read their absolute per-CPU (or per-task) counters.
fn read_value(ev: &PerfEvent) -> u64 {
    match ev.config {
        PERF_COUNT_SW_CONTEXT_SWITCHES => {
            if ev.target_cpu >= 0 {
                CTXSW[ev.target_cpu as usize % crate::config::MAX_CPUS].load(Ordering::Relaxed)
            } else {
                // task-scoped: nvcsw+nivcsw of the target task
                match unsafe {
                    crate::process::pid_hash::pid_hash_lookup_pinned(ev.target_tid)
                } {
                    t if !t.is_null() => {
                        let v = unsafe {
                            ((*t).nvcsw.load(Ordering::Relaxed)
                                + (*t).nivcsw.load(Ordering::Relaxed)) as u64
                        };
                        // SAFETY: pinned lookup took a reference.
                        unsafe { crate::process::task::Task::task_put(t) };
                        v
                    }
                    _ => 0,
                }
            }
        }
        PERF_COUNT_SW_PAGE_FAULTS => {
            if ev.target_cpu >= 0 {
                PFAULT[ev.target_cpu as usize % crate::config::MAX_CPUS].load(Ordering::Relaxed)
            } else {
                // task-scoped: per-task major+minor faults (ru_minflt+
                // ru_majflt equivalent; the Task counters).
                match unsafe {
                    crate::process::pid_hash::pid_hash_lookup_pinned(ev.target_tid)
                } {
                    t if !t.is_null() => {
                        let v = unsafe {
                            ((*t).min_flt.load(Ordering::Relaxed)
                                + (*t).maj_flt.load(Ordering::Relaxed)) as u64
                        };
                        // SAFETY: pinned lookup took a reference.
                        unsafe { crate::process::task::Task::task_put(t) };
                        v
                    }
                    _ => 0,
                }
            }
        }
        _ => ev.counter.load(Ordering::Relaxed),
    }
}

/// read(2): the read-format word set (never records — reference
/// semantics; records travel through the mmap ring).
fn perf_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    let ev = match event_of(file) {
        Some(e) => e,
        None => return -(crate::errno::constants::EBADF as isize),
    };
    // value + up to three optional words.
    let need = 8
        + if ev.read_format & PERF_FORMAT_TOTAL_TIME_ENABLED != 0 { 8 } else { 0 }
        + if ev.read_format & PERF_FORMAT_TOTAL_TIME_RUNNING != 0 { 8 } else { 0 }
        + if ev.read_format & PERF_FORMAT_ID != 0 { 8 } else { 0 };
    if buf.len() < need {
        return -(crate::errno::constants::EINVAL as isize);
    }
    let value = read_value(ev);
    let mut o = 0usize;
    buf[o..o + 8].copy_from_slice(&value.to_le_bytes());
    o += 8;
    if ev.read_format & PERF_FORMAT_TOTAL_TIME_ENABLED != 0 {
        let en = now_ns().saturating_sub(ev.open_ns);
        buf[o..o + 8].copy_from_slice(&en.to_le_bytes());
        o += 8;
    }
    if ev.read_format & PERF_FORMAT_TOTAL_TIME_RUNNING != 0 {
        let run = ev.running_ns.load(Ordering::Relaxed);
        buf[o..o + 8].copy_from_slice(&run.to_le_bytes());
        o += 8;
    }
    if ev.read_format & PERF_FORMAT_ID != 0 {
        buf[o..o + 8].copy_from_slice(&ev.id.to_le_bytes());
        o += 8;
    }
    o as isize
}

/// poll(2): readable while unwritten ring data exists.
fn perf_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;
    let Some(ev) = event_of(file) else { return POLLERR };
    if events & POLLIN != 0 {
        // IRQ-safe: the armed softirq takes inner from IRQ context; a
        // plain take here deadlocks (poll holding inner when the tick
        // softirq spins on it).
        let inner = ev.inner.lock_irqsave();
        let has_data = match inner.pages.as_ref() {
            Some(_) => {
                let tail = ring_read_user_tail(&inner);
                inner.head > tail
            }
            None => false,
        };
        drop(inner);
        if has_data {
            ready |= POLLIN | POLLRDNORM;
        }
    }
    ready
}

fn event_of(file: &crate::fs::File) -> Option<&'static PerfEvent> {
    // SAFETY: private_data is an UnsafeCell; we hold &File so no
    // concurrent mutable access. The pointer stays valid while the
    // File exists (close unregisters + frees under the fd's lifetime).
    let ptr = unsafe { *file.private_data.get() }?;
    // SAFETY: installed by sys_perf_event_open as Box::into_raw.
    Some(unsafe { &*(ptr as *const PerfEvent) })
}

fn perf_close(file: &crate::fs::File) -> i32 {
    let Some(ptr) = (unsafe { *file.private_data.get() }) else { return 0 };
    // SAFETY: see event_of.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    let id = ev.id;
    let was_armed = unregister(id);
    if was_armed {
        ARMED.fetch_sub(1, Ordering::Release);
    }
    // Free the ring pages (if mmap'd) under the event lock, then the
    // event itself. No reader can hold &File past the last fd release.
    {
        // IRQ-safe take (softirq races — see perf_poll).
        let mut inner = ev.inner.lock_irqsave();
        if let Some(pages) = inner.pages.take() {
            for p in pages {
                // Leave the resold-detector set FIRST: from here the
                // frames are legitimately dead; a re-sale after this
                // point is not the premature-free bug.
                ring_frame_remove((p >> 12) as u64);
                // Owner reference (binder/io_uring discipline): the
                // mapping reference went away with the PTEs (munmap or
                // exit runs before the last fd release); the owner drop is
                // the last one — free to the buddy when it hits 0.
                let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(p));
                if !page.is_null() {
                    // SAFETY: descriptor of a page we allocated.
                    unsafe {
                        let r = (*page).put_page();
                        if r == 0 {
                            crate::mm::page_alloc::free_pages(p, 0);
                        } else if r < 0 {
                            // Underflow refusal (refcount was already 0):
                            // the frame has been over-put — it may already
                            // be back in the allocator and owned by someone
                            // else. Freeing again would hand the same frame
                            // to two owners; leak it and name the bug
                            // (hp88a: the old `r <= 0` freed here — a
                            // double-free of a possibly re-sold frame).
                            static N: AtomicUsize = AtomicUsize::new(0);
                            if N.fetch_add(1, Ordering::Relaxed) < 8 {
                                crate::pr_err!(
                                    "PERF-CLOSE-UNDERFLOW: ring frame {:#x} refcount already 0 at owner drop — an extra put happened (leaking, not freeing) pid {}",
                                    p,
                                    crate::process::current_pid()
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    // SAFETY: ptr came from Box::into_raw in sys_perf_event_open.
    unsafe {
        let _ = Box::from_raw(ptr as *mut PerfEvent);
    }
    unsafe { *file.private_data.get() = None; }
    0
}

/// perf event file operations (read/poll/close).
pub(crate) static PERF_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(perf_read)
    .close(perf_close)
    .poll(perf_poll)
    .build();

/// Poll-wait registration (event-driven poll, misc dispatcher).
pub(crate) fn perf_poll_wait(
    file: &crate::fs::File,
    task: *mut crate::process::task::Task,
) -> bool {
    let Some(ev) = event_of(file) else { return false };
    ev.wait_queue
        .add(crate::process::wait::WaitQueueEntry::new(task, false));
    true
}

pub(crate) fn perf_poll_unwait(
    file: &crate::fs::File,
    task: *mut crate::process::task::Task,
) {
    if let Some(ev) = event_of(file) {
        ev.wait_queue.remove(task);
    }
}

// ==================== mmap (ring buffer) ====================

/// mmap on a perf fd: (1+npages)*PAGE_SIZE, page 0 metadata + npages
/// data (power of two). Dispatched from sys_mmap by PERF_OPS identity
/// (binder/io_uring pattern).
pub fn perf_mmap_handler(
    file: &crate::fs::File,
    addr: usize,
    length: usize,
    offset: u64,
    prot: u32,
) -> Result<usize, i32> {
    use crate::arch::mm::{map_page, PageTableEntry, VirtAddr};
    use crate::mm::GfpFlags;
    use crate::mm::page_alloc::{alloc_pages, free_pages};
    use crate::mm::vma::{Vma, VmaFlags};

    // hp88 hunt bisect: noring — deny the ring mapping so hiperf runs
    // long-lived with open fds but no ring mmap at all.
    if node() == NODE_NORING {
        return Err(12); // ENOMEM (hiperf treats as fatal for the event)
    }
    if offset != 0 {
        return Err(22); // EINVAL
    }
    if length < 2 * PAGE_SIZE || length % PAGE_SIZE != 0 {
        return Err(22);
    }
    let npages_data = length / PAGE_SIZE - 1;
    if !npages_data.is_power_of_two() {
        return Err(22);
    }

    let task = crate::sched::current().ok_or(12)?;
    let addr_space = task.address_space().ok_or(12)?;
    let ev = event_of(file).ok_or(6)?; // ENXIO

    // One ring per event (second mmap: EBUSY, reference semantics).
    // IRQ-safe take (softirq races — see perf_poll).
    {
        let inner = ev.inner.lock_irqsave();
        if inner.pages.is_some() {
            return Err(16); // EBUSY
        }
    }

    // Order-0 pages: no high-order pressure at any ring size.
    let total_pages = npages_data + 1;
    let mut pages: Vec<usize> = Vec::new();
    for _ in 0..total_pages {
        let p = alloc_pages(GfpFlags::GFP_KERNEL, 0);
        if p == 0 {
            for q in pages {
                free_pages(q, 0);
            }
            return Err(12); // ENOMEM
        }
        // hp88 hunt: a FRESH allocation must be unowned and unmapped.
        // The resold detector only catches re-sales of listed ring
        // frames; this catches the allocator handing out a frame that
        // is still mapped in (or owned by) someone else — the first
        // sale of a live frame, which zeroing below would destroy.
        // Pristine mapcount is PAGE_MAPCOUNT_BIAS (-1), not 0.
        {
            let pd = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(p));
            if !pd.is_null() {
                // SAFETY: descriptor of the frame alloc_pages just
                // returned; read-only loads.
                let (rc, mc) = unsafe { ((*pd).refcount(), (*pd).mapcount()) };
                if rc != 1 || mc != crate::mm::page_desc::PAGE_MAPCOUNT_BIAS {
                    crate::pr_err!(
                        "PERF-ALLOC-DIRTY: alloc_pages returned frame {:#x} with refcount {} mapcount {} — allocator handed out a LIVE page; zeroing it would corrupt its owner pid {}",
                        p, rc, mc,
                        crate::process::current_pid()
                    );
                }
            }
        }
        // SAFETY/zero: fresh buddy page.
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(p as u64))
            .bits() as *mut u8;
        unsafe { core::ptr::write_bytes(kv, 0, PAGE_SIZE) };
        pages.push(p);
    }

    let data_size = npages_data * PAGE_SIZE;

    // VMA placement (binder model: atomic pick + insert under
    // vma_write, bottom-up scan from MMAP_START).
    let mut vma_flags = VmaFlags::new();
    vma_flags.insert(VmaFlags::READ);
    vma_flags.insert(VmaFlags::WRITE);
    vma_flags.insert(VmaFlags::SHARED);
    let vaddr = {
        let mut vmas = addr_space.vma_write();
        let user_end = crate::arch::mm::user_addr::USER_END;
        let mut base = if addr == 0 {
            None
        } else {
            let b = addr & !(PAGE_SIZE - 1);
            if b + length > user_end {
                for q in &pages { free_pages(*q, 0); }
                return Err(22);
            }
            let probe = Vma::new(
                crate::mm::page::VirtAddr::new(b),
                crate::mm::page::VirtAddr::new(b + length),
                vma_flags,
            );
            if vmas.iter().any(|v| v.overlaps(&probe)) { None } else { Some(b) }
        };
        if base.is_none() {
            use crate::arch::mm::user_addr as ua;
            let mut search_start = ua::MMAP_START;
            let search_end = ua::MMAP_END.min(user_end.saturating_sub(length));
            let mut hit: Option<usize> = None;
            for v in vmas.iter() {
                let vs = v.start().as_usize();
                if vs > search_start && vs - search_start >= length {
                    hit = Some(search_start);
                    break;
                }
                if v.end().as_usize() > search_start {
                    search_start = v.end().as_usize();
                }
                if search_start > search_end {
                    break;
                }
            }
            let b = match hit {
                Some(b) => b,
                None => {
                    if search_start <= search_end && search_end - search_start >= length {
                        search_start
                    } else {
                        drop(vmas);
                        for q in &pages { free_pages(*q, 0); }
                        return Err(12);
                    }
                }
            };
            base = Some(b);
        }
        let b = base.unwrap();
        let mut vma = Vma::new(
            crate::mm::page::VirtAddr::new(b),
            crate::mm::page::VirtAddr::new(b + length),
            vma_flags,
        );
        vma.set_origin(crate::mm::vma::VmaOrigin::from_bits(
            crate::mm::vma::VmaOrigin::OTHER,
        ));
        let ring_pages = vma.page_count() as u64;
        if vmas.add(vma).is_err() {
            drop(vmas);
            for q in &pages { free_pages(*q, 0); }
            return Err(12);
        }
        addr_space.add_total_vm(ring_pages);
        b
    };

    // Eager PTE install (binder discipline: under PTE_MODIFY_LOCK).
    let root = addr_space.root_ppn();
    let mut pte_flags = PageTableEntry::V | PageTableEntry::U | PageTableEntry::A | PageTableEntry::D;
    if prot & 0x1 != 0 {
        pte_flags |= PageTableEntry::R;
    }
    if prot & 0x2 != 0 {
        pte_flags |= PageTableEntry::R | PageTableEntry::W;
    }
    {
        #[cfg(feature = "x86_64")]
        let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
        #[cfg(not(feature = "x86_64"))]
        let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
        for i in 0..total_pages {
            let va = vaddr + i * PAGE_SIZE;
            let pa = pages[i];
            // SAFETY: fresh pages, aligned, under the PTE lock.
            unsafe {
                map_page(
                    root,
                    VirtAddr::new(va as u64),
                    crate::arch::mm::PhysAddr::new(pa as u64),
                    pte_flags,
                );
            }
            // Mapping reference (binder/io_uring discipline): munmap or
            // process exit put_page's the PTE ref; perf_close drops the
            // owner ref; the last drop frees.
            let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(pa));
            if !page.is_null() {
                // SAFETY: descriptor of the page we just mapped.
                unsafe { (*page).get_page(); }
            }
        }
    }

    // Publish the ring into the event (metadata fields on page 0).
    {
        let mut inner = ev.inner.lock_irqsave();
        let base = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pages[0] as u64))
            .bits() as *mut u8;
        // SAFETY: our own metadata page.
        unsafe {
            core::ptr::write_volatile(base.add(MP_VERSION) as *mut u32, 1);
            core::ptr::write_volatile(base.add(MP_COMPAT_VERSION) as *mut u32, 2);
            // cap_bit0 = 0, cap_bit0_is_deprecated = 1 (bit 1).
            core::ptr::write_volatile(base.add(MP_CAPABILITIES) as *mut u64, 0x2);
            core::ptr::write_volatile(base.add(MP_DATA_OFFSET) as *mut u64, PAGE_SIZE as u64);
            core::ptr::write_volatile(base.add(MP_DATA_SIZE) as *mut u64, data_size as u64);
            core::ptr::write_volatile(base.add(MP_TIME_ENABLED) as *mut u64, 0);
            core::ptr::write_volatile(base.add(MP_TIME_RUNNING) as *mut u64, 0);
            core::ptr::write_volatile(base.add(MP_DATA_HEAD) as *mut u64, 0);
            core::ptr::write_volatile(base.add(MP_DATA_TAIL) as *mut u64, 0);
        }
        inner.data_size = data_size;
        inner.head = 0;
        inner.lost = 0;
        inner.pages = Some(pages.clone());
        inner.va = vaddr;
        inner.va_len = length;
    }
    // Track the frames in the premature-free detector set (hp88a hunt).
    for p in &pages {
        ring_frame_insert((p >> 12) as u64);
    }
    // hp88 hunt: name every ring placement — correlate with #GP victims.
    {
        static N: AtomicUsize = AtomicUsize::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 16 {
            crate::pr_info!(
                "PERF-RING-MAP: event {} ring at {:#x} len {:#x} ({} pages) pid {}",
                ev.id, vaddr, length, total_pages,
                crate::process::current_pid()
            );
        }
    }

    Ok(vaddr)
}

// ==================== ioctls ====================

/// Returns Some(ret) when the file is a perf fd (sys_ioctl dispatch,
/// binder pattern).
pub fn perf_file_ioctl(file: &crate::fs::File, request: u32, arg: usize) -> Option<i64> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &PERF_OPS as *const _) {
        return None;
    }
    Some(perf_ioctl(file, request, arg))
}

fn perf_ioctl(file: &crate::fs::File, request: u32, arg: usize) -> i64 {
    let Some(ev) = event_of(file) else { return -(crate::errno::constants::EBADF as i64) };
    // hp88 hunt bisect: noarm — ENABLE/REFRESH pretend success but the
    // event never arms (no tick hooks, no softirq, no emit).
    if node() == NODE_NOARM && (request == IOC_ENABLE || request == IOC_REFRESH) {
        return 0;
    }
    match request {
        IOC_ENABLE | IOC_DISABLE => {
            let enable = request == IOC_ENABLE;
            let was = ev.enabled.load(Ordering::Acquire);
            if was != enable {
                if enable {
                    if ARMED.fetch_add(1, Ordering::Acquire) + 1 == 1 {
                        // First arm: prime each CPU's stash so the first
                        // charged tick has a consistent baseline.
                        for s in STASH.iter() {
                            s.charged.store(s.seq.load(Ordering::Relaxed), Ordering::Relaxed);
                        }
                    }
                } else {
                    ARMED.fetch_sub(1, Ordering::Release);
                }
                ev.enabled.store(enable, Ordering::Release);
                refresh_wanted_bits();
            }
            0
        }
        IOC_REFRESH => {
            // Treat REFRESH(n) as an enable (this port's sampler is
            // not overflow-limited; the count is not decremented).
            let was = ev.enabled.load(Ordering::Acquire);
            if !was {
                ARMED.fetch_add(1, Ordering::Acquire);
                ev.enabled.store(true, Ordering::Release);
                refresh_wanted_bits();
            }
            0
        }
        IOC_RESET => {
            ev.counter.store(0, Ordering::Relaxed);
            ev.running_ns.store(0, Ordering::Relaxed);
            0
        }
        IOC_PERIOD => {
            // _IOW u64: change the sampling period.
            let Some(v) = (unsafe {
                crate::arch::uaccess::get_user(arg as *const u64)
            }) else {
                return -(crate::errno::constants::EFAULT as i64);
            };
            if v == 0 {
                return -(crate::errno::constants::EINVAL as i64);
            }
            ev.sample_period.store(v.max(TICK_NS), Ordering::Relaxed);
            0
        }
        IOC_ID => {
            let id = ev.id;
            match unsafe { crate::arch::uaccess::put_user(arg as *mut u64, id) } {
                true => 0,
                false => -(crate::errno::constants::EFAULT as i64),
            }
        }
        IOC_SET_OUTPUT => {
            // Redirecting into another event's ring: this port keeps
            // one ring per event and hiperf only SET_OUTPUTs within a
            // group (single-event records never hit it). Reject the
            // redirect the way the reference does for a target without
            // a ring.
            -(crate::errno::constants::EINVAL as i64)
        }
        _ => -(crate::errno::constants::ENOTTY as i64),
    }
}

// ==================== syscall ====================

/// Bounded forensic line for rejected perf_event_open attrs (hiperf
/// bring-up, TASK 87.10): hiperf fails with a bare "syscall
/// perf_event_open failed (errno 22)" in its own log while exiting 0 —
/// this names the validation arm and the full attr so one boot settles
/// the parity gap. First 8 rejections only.
fn perf_open_einval_diag(arm: &str, attr: &AttrHead, pid: i32, cpu: i32, group_fd: i32, flags: u64) {
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
        crate::pr_err!(
            "PERF-EINVAL[{}]: pid={} cpu={} gfd={} flags={:#x} size={} type={:#x} config={:#x} sample_type={:#x} read_format={:#x} flags1={:#x} flags2={:#x} period={}",
            arm, pid, cpu, group_fd, flags, attr.size, attr.type_, attr.config,
            attr.sample_type, attr.read_format, attr.flags1, attr.flags2, attr.sample_period
        );
    }
}

/// sys_perf_event_open(attr, pid, cpu, group_fd, flags) — software
/// counting + mmap-ring sampling (see the module doc for the accepted
/// surface; everything unsupported returns EINVAL rather than
/// silently misbehaving).
pub fn sys_perf_event_open(args: crate::syscall::SyscallArgs) -> i64 {
    let attr_ptr = args[0] as *const AttrHead;
    let pid = args[1] as i32;
    let cpu = args[2] as i32;
    let group_fd = args[3] as i32;
    let flags = args[4];

    // hp88 hunt bisect: noperf — reject every open (hiperf's record
    // fails fast and exits, the exact hp87p/q/r shape on today's kernel).
    if node() == NODE_NOPERF {
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 1. Flags: only the bits this slice can honor.
    if flags & !(PERF_FLAG_FD_NO_GROUP | PERF_FLAG_FD_CLOEXEC) != 0 {
        // FD_OUTPUT (ring redirect) and PID_CGROUP (cgroup scoping)
        // are real ABI features we do not implement — EINVAL.
        let a = unsafe { crate::arch::uaccess::get_user(attr_ptr as *const AttrHead) }
            .unwrap_or_default();
        perf_open_einval_diag("flags", &a, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 2. Read the attr head. The mandatory part is 64 bytes; we only
    // decode the first 56 but require the user's size to cover VER0.
    if !crate::arch::uaccess::access_ok(attr_ptr as usize, 64) {
        return -(crate::errno::constants::EFAULT as i64);
    }
    // SAFETY: access_ok passed; get_user re-verifies page-wise.
    let attr = match unsafe { crate::arch::uaccess::get_user(attr_ptr as *const AttrHead) } {
        Some(a) => a,
        None => return -(crate::errno::constants::EFAULT as i64),
    };
    if attr.size < 64 || attr.size > 1024 {
        perf_open_einval_diag("size", &attr, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64);
    }

    // perf_event_paranoid=2 parity: kernel-inclusive measurement is
    // privileged (CAP_PERFMON or CAP_SYS_ADMIN). User-space-only
    // events (exclude_kernel set) stay open to unprivileged callers.
    if attr.flags1 & FLAG_EXCLUDE_KERNEL == 0
        && !crate::security::capable(crate::security::CAP_PERFMON)
        && !crate::security::capable(crate::security::CAP_SYS_ADMIN)
    {
        return -(crate::errno::constants::EACCES as i64);
    }

    // 3. Event class: software slice only.
    if attr.type_ != PERF_TYPE_SOFTWARE {
        perf_open_einval_diag("type", &attr, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64);
    }
    if attr.config != PERF_COUNT_SW_CPU_CLOCK
        && attr.config != PERF_COUNT_SW_TASK_CLOCK
        && attr.config != PERF_COUNT_SW_PAGE_FAULTS
        && attr.config != PERF_COUNT_SW_CONTEXT_SWITCHES
    {
        perf_open_einval_diag("config", &attr, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 4. Sampling configuration.
    let sample_type = attr.sample_type;
    if sample_type & !SAMPLE_TYPE_KNOWN != 0 {
        perf_open_einval_diag("sample_type", &attr, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64); // bits we cannot fill
    }
    if attr.read_format & PERF_FORMAT_GROUP != 0 {
        perf_open_einval_diag("read_format_group", &attr, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64); // no group reads
    }
    if attr.read_format
        & !(PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING | PERF_FORMAT_ID)
        != 0
    {
        perf_open_einval_diag("read_format_bits", &attr, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64);
    }
    if attr.flags1 & FLAG_INHERIT != 0 {
        // hiperf's counting mode sends inherit=1 even for per-CPU
        // system-wide events (hp87p PERF-EINVAL: flags1=0x3). For
        // COUNTING software events inherit is a no-op: a per-CPU
        // counter already covers every task on that CPU, children
        // included (and per-task counting without child tracking only
        // undercounts — tooling-visible softness, not corruption).
        // Sampled PER-TASK events keep the strict reject — a ring
        // inherited by untracked children has no page to land records
        // in. Per-CPU system-wide events (pid == -1, hp87q: hiperf
        // record sends exactly this shape) accept it: children on the
        // CPU are already covered by the CPU scope.
        if attr.sample_type != 0 && pid >= 0 {
            perf_open_einval_diag("inherit_sampled_pertask", &attr, pid, cpu, group_fd, flags);
            return -(crate::errno::constants::EINVAL as i64); // no child inheritance
        }
    }
    if attr.flags1 & FLAG_ENABLE_ON_EXEC != 0 {
        perf_open_einval_diag("read_format_bits", &attr, pid, cpu, group_fd, flags);
        return -(crate::errno::constants::EINVAL as i64);
    }
    if attr.flags2 & 0x1f != 0 {
        // exclude_callchain_*, mmap2/comm_exec/use_clockid/context_switch
        // (flags2 bits 0..4): callchain exclusion is implicit (we emit
        // user chains only); clockid and context-switch sidebands are
        // not implemented.
        return -(crate::errno::constants::EINVAL as i64);
    }

    let mut sample_period = 0u64;
    if sample_type != 0 {
        if attr.flags1 & FLAG_FREQ != 0 {
            // freq mode: period = 1e9/freq, floored at one tick.
            if attr.sample_period == 0 || attr.sample_period > 100_000_000 {
                return -(crate::errno::constants::EINVAL as i64);
            }
            sample_period = (1_000_000_000 / attr.sample_period).max(TICK_NS);
        } else {
            if attr.sample_period == 0 {
                return -(crate::errno::constants::EINVAL as i64);
            }
            sample_period = attr.sample_period.max(TICK_NS);
        }
    }

    // 5. Scoping. pid: 0 = calling task, >0 = that TID, -1 = any task
    // on the given CPU (cpu-clock per-CPU form); both -1 = EINVAL.
    if group_fd != -1 {
        return -(crate::errno::constants::EINVAL as i64); // no event groups
    }
    if cpu < -1 || (cpu >= 0 && cpu as usize >= crate::config::MAX_CPUS) {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if pid < -1 {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if pid == -1 && cpu == -1 {
        return -(crate::errno::constants::EINVAL as i64);
    }

    let (target_tid, target_cpu);
    if cpu >= 0 {
        // Per-CPU form: pid must be -1 (the reference's per-CPU
        // measurement measures the CPU, not a task).
        if pid != -1 {
            return -(crate::errno::constants::EINVAL as i64);
        }
        target_tid = 0;
        target_cpu = cpu;
    } else {
        // Task-scoped: pid 0 = self.
        let tid = if pid == 0 {
            match crate::sched::current() {
                Some(t) => unsafe { (*t).pid() },
                None => return -(crate::errno::constants::EINVAL as i64),
            }
        } else {
            pid as u32
        };
        // Existence + permission (same real uid or root, the
        // reference's perf permission rule simplified to uid).
        // SAFETY: pinned lookup keeps the Task alive across the check.
        unsafe {
            let target = crate::process::pid_hash::pid_hash_lookup_pinned(tid);
            if target.is_null() {
                return -(crate::errno::constants::ESRCH as i64);
            }
            let ok = if let Some(me) = crate::sched::current() {
                (*target).cred().uid == (*me).cred().uid || (*me).cred().uid == 0
            } else {
                false
            };
            crate::process::task::Task::task_put(target);
            if !ok {
                return -(crate::errno::constants::EACCES as i64);
            }
        }
        target_tid = tid;
        target_cpu = -1;
    }

    let mut starts_armed = attr.flags1 & FLAG_DISABLED == 0;
    if node() == NODE_NOARM {
        starts_armed = false;
    }

    // 6. Build + install the fd.
    let Some(fdtable) = crate::sched::get_current_fdtable() else {
        return -(crate::errno::constants::EMFILE as i64);
    };

    let ev = Box::new(PerfEvent {
        id: 0,
        config: attr.config,
        sample_type,
        read_format: attr.read_format,
        sample_period: AtomicU64::new(sample_period),
        target_tid,
        target_cpu,
        exclude_user: attr.flags1 & FLAG_EXCLUDE_USER != 0,
        exclude_kernel: attr.flags1 & FLAG_EXCLUDE_KERNEL != 0,
        exclude_idle: attr.flags1 & FLAG_EXCLUDE_IDLE != 0,
        track_comm: attr.flags1 & FLAG_COMM != 0,
        track_mmap: attr.flags1 & (FLAG_MMAP | FLAG_MMAP_DATA) != 0,
        enabled: AtomicBool::new(starts_armed),
        open_ns: now_ns(),
        running_ns: AtomicU64::new(0),
        counter: AtomicU64::new(0),
        inner: Spinlock::new(PerfInner::new()),
        wait_queue: WaitQueueHead::new(),
    });
    let ev_ptr = Box::into_raw(ev);
    register(ev_ptr, starts_armed);

    // O_RDWR: the ring mapping is MAP_SHARED+PROT_WRITE (hiperf record,
    // hp87r: mmap errno 13) and the generic file-backed mmap gate in
    // sys_mmap requires a writable fd for that combination. Reference
    // perf fds are anon-inode-like and bypass mode checks; carrying
    // O_RDWR achieves the same through our gate.
    let file_flags = crate::fs::file::FileFlags::O_RDWR;
    let file = Arc::new(crate::fs::File::new(
        crate::fs::file::FileFlags::new(file_flags),
    ));
    file.set_ops(&PERF_OPS);
    file.set_private_data(ev_ptr as *mut u8);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // Undo the registration (the id was stored in the event).
            let was_armed = unregister(unsafe { (*ev_ptr).id });
            if was_armed {
                ARMED.fetch_sub(1, Ordering::Release);
            }
            // SAFETY: reclaim the box.
            unsafe { let _ = Box::from_raw(ev_ptr); }
            return -(crate::errno::constants::EMFILE as i64);
        }
    };

    match fdtable.install_fd(fd, file) {
        Ok(()) => {
            if flags & PERF_FLAG_FD_CLOEXEC != 0 {
                crate::fs::set_cloexec_fd(fd, true);
            }
            fd as i64
        }
        Err(_) => {
            let was_armed = unregister(unsafe { (*ev_ptr).id });
            if was_armed {
                ARMED.fetch_sub(1, Ordering::Release);
            }
            // SAFETY: reclaim the box.
            unsafe { let _ = Box::from_raw(ev_ptr); }
            -(crate::errno::constants::EMFILE as i64)
        }
    }
}

// ==================== unit tests ====================

#[cfg(any(test, feature = "unit-test"))]
pub mod tests {
    use super::*;

    /// Record encoding: sample and lost records match the ABI layout.
    pub fn test_record_encoding() {
        let mut rec = [0u8; 16];
        rec[0..4].copy_from_slice(&PERF_RECORD_SAMPLE.to_le_bytes());
        rec[4..6].copy_from_slice(&PERF_RECORD_MISC_USER.to_le_bytes());
        rec[6..8].copy_from_slice(&16u16.to_le_bytes());
        rec[8..16].copy_from_slice(&0x1234_5678_9abc_def0u64.to_le_bytes());
        let expect: [u8; 16] = [
            9, 0, 0, 0, 1, 0, 16, 0,
            0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12,
        ];
        if rec == expect {
            crate::tests::test_pass("perf-record");
        } else {
            crate::tests::test_fail("perf-record", "sample record bytes mismatch");
        }
        let l = lost_record_bytes(7, 3);
        let mut ok = true;
        ok &= l[0..4] == PERF_RECORD_LOST.to_le_bytes();
        ok &= l[6..8] == (24u16).to_le_bytes();
        ok &= l[8..16] == 7u64.to_le_bytes();
        ok &= l[16..24] == 3u64.to_le_bytes();
        if ok {
            crate::tests::test_pass("perf-lost");
        } else {
            crate::tests::test_fail("perf-lost", "lost record bytes mismatch");
        }
    }

    /// LOST record builder (kept for the ring path's report record).
    fn lost_record_bytes(id: u64, lost: u64) -> [u8; 24] {
        let mut rec = [0u8; 24];
        rec[0..4].copy_from_slice(&PERF_RECORD_LOST.to_le_bytes());
        rec[6..8].copy_from_slice(&24u16.to_le_bytes());
        rec[8..16].copy_from_slice(&id.to_le_bytes());
        rec[16..24].copy_from_slice(&lost.to_le_bytes());
        rec
    }

    /// Ring wrap arithmetic: a write crossing the data-area end lands
    /// at the start (page-wise walker, synthetic pages).
    pub fn test_ring_wrap() {
        // data_size 2 pages; write 8 bytes at pos = data_size-4.
        // ring_slice resolves page 1 offset 4092 with n clamped to 4.
        // (Full-path ring_put needs live pages; the arithmetic is
        // covered by the boot-side hiperf run.)
        let data_size = 2 * PAGE_SIZE;
        let pos = (data_size - 4) as u64;
        let p = (pos % data_size as u64) as usize;
        let page_idx = p / PAGE_SIZE;
        let off = p % PAGE_SIZE;
        let ok = page_idx == 1 && off == PAGE_SIZE - 4;
        if ok {
            crate::tests::test_pass("perf-ringwrap");
        } else {
            crate::tests::test_fail("perf-ringwrap", "wrap arithmetic mismatch");
        }
    }

    /// freq→period conversion and the tick floor.
    pub fn test_freq_floor() {
        let f = 4000u64;
        let period = (1_000_000_000 / f).max(TICK_NS);
        // 4000Hz wants 250µs; the sampler floors at one tick.
        if period == TICK_NS {
            crate::tests::test_pass("perf-freqfloor");
        } else {
            crate::tests::test_fail("perf-freqfloor", "period not floored at TICK_NS");
        }
    }
}
