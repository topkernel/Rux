//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! perf events — software-clock sampling (`perf_event_open(2)` ABI).
//!
//! Scope of this first slice (the plan doc's perf tranche, part 1):
//! `PERF_TYPE_SOFTWARE` counters `PERF_COUNT_SW_CPU_CLOCK` (0) and
//! `PERF_COUNT_SW_TASK_CLOCK` (1) with `sample_period` (ns of counted
//! time per sample) and `PERF_SAMPLE_IP` / `PERF_SAMPLE_CALLCHAIN`
//! records. Everything else keeps returning `EINVAL`/`ENOSYS` as
//! before — no invented interfaces: the attr layout, the open flag
//! set, the pid/cpu scoping rules, the record binary layout
//! (`perf_event_header` + u64 ip) and the `PERF_RECORD_LOST` overflow
//! report are perf_event_open(2)'s.
//!
//! ## read(2): the reference's counter read (never blocks)
//!
//! read(2) on the event fd returns the counter per the man page's
//! reading section — u64 count, then (per attr.read_format) u64
//! total_time_enabled, u64 total_time_running, u64 id. Sample records
//! are NOT read(2) output: the reference delivers samples through the
//! mmap ring (unimplemented here). The stopgap record channel is the
//! per-event debugfs drain file `/sys/kernel/debug/perf/<id>/records`
//! (each open drains up to 8 KiB of pending records — LOST report
//! first, then samples). A blocking read was the stress fleet's
//! session-13 self-deadlock: a task-scoped event read from its own
//! task slept forever, because its samples could only be charged
//! while that very task ran.
//!
//! ## How samples are produced (internal design)
//!
//! The hard timer IRQ (the arch trap paths, next to the syscost tick
//! hooks) calls [`note_tick`] with the interrupted PC, the privilege
//! mode and the interrupted task's TID. That hook is LOCK-FREE: one
//! relaxed load to bail when nothing is armed, otherwise three
//! per-CPU atomic stores + a sequence bump — hard-IRQ context takes
//! no locks here.
//!
//! The Hrtimer softirq (raised every tick while any event is armed;
//! see `timer_softirq_work_pending`) calls [`softirq_tick`] on the
//! same CPU, which charges `pending_ticks * TICK_NS` to every
//! matching armed event and emits a sample whenever the accumulated
//! counter crosses `sample_period` (catch-up loop, so a burst of
//! ticks between softirq passes yields the right sample count).
//! Charging and waking readers happen in softirq context where locks
//! and wait-queue wakes are normal (the timerfd expire notify runs in
//! this same context).
//!
//! Granularity note: samples can only be produced at tick (or
//! softirq-pass) boundaries, so a `sample_period` below one tick
//! (10 ms at KERNEL_HZ=100) coalesces to tick-rate samples rather
//! than being lost. The counter arithmetic itself is exact ns.
//!
//! ## Lock order and the no-allocation rule (INV: registry → event.inner)
//!
//! `REGISTRY` (irqsave) is the outer lock; an event's `inner` is
//! taken under it by the charging path. `read`/`poll` take `inner`
//! alone. All REGISTRY takes are `lock_irqsave()` uniformly, so no
//! holder can ever be interrupted into a path that wants it.
//!
//! **No allocation under any spinlock reachable from softirq/irq
//! context** (the stress fleet's session-13 both-CPU deadlock: the
//! timer softirq's first `to_wake.push` allocated under REGISTRY and
//! the record ring grew under `inner`; under heap pressure the
//! allocator slowpath inside those sections stalled both CPUs — 67/67
//! DEADLOCK rows). Enforced structurally now: REGISTRY is a
//! fixed-capacity array (never grows), the wake list is a per-CPU
//! fixed array, and each event's record ring reserves its full budget
//! at construction (process context, no locks) so the charging path's
//! `push_back` can never allocate. The debugfs drain allocates its
//! output buffer BEFORE taking REGISTRY.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
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
/// sample_type bit — record the interrupted instruction pointer.
pub const PERF_SAMPLE_IP: u64 = 1 << 0;
/// sample_type bit — record the kernel call chain of the sample
/// (frame-pointer walk of the interrupted kernel context).
pub const PERF_SAMPLE_CALLCHAIN: u64 = 1 << 5;
/// The callchain's kernel-context marker (uapi's PERF_CONTEXT_KERNEL):
/// the chain opens with it, then kernel addresses follow.
const PERF_CONTEXT_KERNEL: u64 = u64::MAX - 255;

/// attr bitfield (u64 at offset 40) bits this slice reads.
const FLAG_DISABLED: u64 = 1 << 0;
const FLAG_INHERIT: u64 = 1 << 1;
const FLAG_EXCLUDE_USER: u64 = 1 << 4;
const FLAG_EXCLUDE_KERNEL: u64 = 1 << 5;
const FLAG_EXCLUDE_IDLE: u64 = 1 << 7;
const FLAG_FREQ: u64 = 1 << 10;

/// perf_event_open flags argument — the uapi bit values verbatim
/// (include/uapi/linux/perf_event.h: FD_NO_GROUP 1<<0, FD_OUTPUT 1<<1,
/// PID_CGROUP 1<<2, FD_CLOEXEC 1<<3). The stress lane's fsfuzz arm
/// caught this family shifted one off the reference (NO_GROUP at 1<<1,
/// PID_CGROUP at 1<<4): callers passing the reference's FD_NO_GROUP
/// got spurious EINVAL.
const PERF_FLAG_FD_NO_GROUP: u64 = 1 << 0;
const PERF_FLAG_FD_OUTPUT: u64 = 1 << 1;
const PERF_FLAG_PID_CGROUP: u64 = 1 << 2;
const PERF_FLAG_FD_CLOEXEC: u64 = 1 << 3;

/// attr.read_format bits (uapi): what read(2) lays out after the
/// 8-byte counter. ENABLED/RUNNING/ID are implemented; GROUP (1<<3)
/// and anything else is refused at open.
const PERF_FORMAT_TOTAL_TIME_ENABLED: u64 = 1 << 0;
const PERF_FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;
const PERF_FORMAT_ID: u64 = 1 << 2;

/// Record types (perf_event_header.type).
const PERF_RECORD_LOST: u32 = 2;
const PERF_RECORD_SAMPLE: u32 = 9;
/// perf_event_header.misc cpumode bits.
const PERF_RECORD_MISC_USER: u16 = 1;
const PERF_RECORD_MISC_KERNEL: u16 = 2;

/// The mandatory head of `struct perf_event_attr` — everything this
/// slice reads lives in the first 48 bytes. `size` (the user's own
/// attr->size) gates how much of the struct the user filled in; the
/// ABI minimum (PERF_ATTR_SIZE_VER0) is 64.
#[repr(C)]
#[derive(Clone, Copy)]
struct AttrHead {
    type_: u32,   // offset 0
    size: u32,    // offset 4
    config: u64,  // offset 8
    sample_period: u64, // offset 16 (sample_freq when FLAG_FREQ)
    sample_type: u64,   // offset 24
    read_format: u64,   // offset 32
    flags1: u64,        // offset 40 (bitfield: disabled..)
}

/// ns per tick — the accounting quantum.
const TICK_NS: u64 = 1_000_000_000 / (crate::config::KERNEL_HZ as u64);

/// Pending-record byte budget per event. A sample is 16 bytes, so
/// this holds 4096 samples; beyond it new samples bump `lost` and are
/// reported via PERF_RECORD_LOST on the next read.
const RING_CAP: usize = 64 * 1024;

// ==================== Per-CPU tick stash (hard IRQ → softirq) ====================

/// Tick privilege modes for the stash.
pub const MODE_KERNEL: u8 = 0;
pub const MODE_USER: u8 = 1;
pub const MODE_IDLE: u8 = 2;

struct TickStash {
    pc: AtomicU64,
    mode: AtomicU8,
    tid: AtomicU32,
    /// The interrupted kernel frame pointer (for the callchain walk).
    /// Only meaningful with MODE_KERNEL.
    fp: AtomicU64,
    /// The walked kernel chain (return addresses, newest first) and
    /// its length; walked in the hard IRQ when any event wants
    /// callchains — softirq time is too late, the stack has moved on
    /// (the reference walks in the hrtimer overflow callback, the same
    /// context discipline).
    chain: [AtomicU64; CHAIN_FRAMES],
    chain_len: AtomicU8,
    /// Incremented by every armed tick; the softirq charges the
    /// delta since its last pass (normally 1).
    seq: AtomicU64,
    /// seq already charged by the softirq.
    charged: AtomicU64,
}

/// How many frames a sample chain carries at most (the reference's
/// default PERF_MAX_STACK_DEEP class; the frame-pointer walker's 32
/// frames trimmed to the wedge-hunt need).
pub const CHAIN_FRAMES: usize = 16;

const fn new_stash() -> TickStash {
    TickStash {
        pc: AtomicU64::new(0),
        mode: AtomicU8::new(MODE_IDLE),
        tid: AtomicU32::new(0),
        fp: AtomicU64::new(0),
        chain: [const { AtomicU64::new(0) }; CHAIN_FRAMES],
        chain_len: AtomicU8::new(0),
        seq: AtomicU64::new(0),
        charged: AtomicU64::new(0),
    }
}

static STASH: [TickStash; crate::config::MAX_CPUS] = [const { new_stash() }; crate::config::MAX_CPUS];

/// Armed-and-enabled event count: the hard-IRQ bail-out and the
/// softirq-raise gate (`timer_softirq_work_pending`) read this; open
/// and close maintain it. When 0 the subsystem costs one relaxed load
/// per tick.
static ARMED: AtomicUsize = AtomicUsize::new(0);

/// Events wanting PERF_SAMPLE_CALLCHAIN: the hard-IRQ hook walks the
/// interrupted stack only while some reader asked for chains.
static CALLCHAIN_ARMED: AtomicUsize = AtomicUsize::new(0);

/// Hard-IRQ tick hook (arch trap paths). `tid` 0 = idle/no task. `fp`
/// is the interrupted frame pointer (the trap frame's s0/rbp) — kept
/// for the callchain walk. Lock-free by contract — see the module doc;
/// the walk itself reads only kernel stack memory through the same
/// plausibility gates memwatch uses.
#[inline]
pub fn note_tick(pc: u64, mode: u8, tid: u32, fp: u64) {
    if ARMED.load(Ordering::Relaxed) == 0 {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c >= crate::config::MAX_CPUS {
        return;
    }
    let s = &STASH[c];
    // The chain is walked HERE, in hard-IRQ context, only for
    // kernel-mode ticks while some event wants it: softirq time is too
    // late (the stack may have unwound), and a user-mode tick's kernel
    // stack holds the previous syscall's stale trail — an honest
    // nr=0 chain answers that case (no user-stack walker exists).
    let chain_len = if mode == MODE_KERNEL && CALLCHAIN_ARMED.load(Ordering::Relaxed) > 0 {
        let mut frames = [0u64; CHAIN_FRAMES];
        // SAFETY: fp is the interrupted context's frame pointer from
        // the trap frame; the walk validates every hop (kernel-window
        // plausibility, alignment, monotonicity) before dereferencing.
        unsafe {
            crate::dfx::memwatch::walk_fp_chain(fp, &mut frames);
        }
        let mut n = 0usize;
        while n < CHAIN_FRAMES && frames[n] != 0 {
            s.chain[n].store(frames[n], Ordering::Relaxed);
            n += 1;
        }
        n as u8
    } else {
        0
    };
    s.chain_len.store(chain_len, Ordering::Relaxed);
    s.pc.store(pc, Ordering::Relaxed);
    s.mode.store(mode, Ordering::Relaxed);
    s.tid.store(tid, Ordering::Relaxed);
    s.fp.store(fp, Ordering::Relaxed);
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
    /// PERF_COUNT_SW_CPU_CLOCK / PERF_COUNT_SW_TASK_CLOCK.
    config: u64,
    sample_period: u64,
    /// PERF_SAMPLE_CALLCHAIN requested in sample_type.
    wants_callchain: bool,
    /// TID to match (task-scoped events); 0 in per-CPU mode.
    target_tid: u32,
    /// CPU to match; -1 = task-scoped, else per-CPU.
    target_cpu: i32,
    exclude_user: bool,
    exclude_kernel: bool,
    exclude_idle: bool,
    /// attr.read_format (the accepted subset): read(2)'s tail layout.
    read_format: u64,
    /// sched_clock() stamp at open while armed; 0 for a disabled open
    /// (time_enabled reads 0 until an ENABLE ioctl exists).
    enabled_at: u64,
    enabled: AtomicBool,
    inner: Spinlock<PerfInner>,
    /// Readers blocked in read() waiting for records.
    wait_queue: WaitQueueHead,
}

struct PerfInner {
    /// ns accumulated since the last sample.
    counter: u64,
    /// Total ns charged since enable — the counter value read(2)
    /// reports (never decremented by sampling).
    total: u64,
    /// Pending record bytes (whole records, little-endian). The
    /// capacity is RESERVED at event construction (RING_CAP): pushes
    /// are bounded by the same RING_CAP check, so push_back can never
    /// grow the buffer — no allocation ever happens under `inner`
    /// (the stress fleet's session-13 registry deadlock: an allocator
    /// slowpath inside a spinlocked section reached from the timer
    /// softirq stalled both CPUs under heap pressure).
    ring: VecDeque<u8>,
    /// Samples dropped because the ring was full (reported as
    /// PERF_RECORD_LOST on the next record drain).
    lost: u64,
}

/// The registry: a FIXED-CAPACITY array — no allocation under its
/// lock, ever (session-13: register()'s Vec growth under REGISTRY was
/// one leg of the both-CPU deadlock). 128 simultaneous events; a full
/// registry answers EMFILE at open (the fd-exhaustion errno class).
const MAX_EVENTS: usize = 128;

struct Registry {
    slots: [(u64, usize); MAX_EVENTS],
    used: usize,
}

const fn new_registry() -> Registry {
    Registry {
        slots: [(0, 0); MAX_EVENTS],
        used: 0,
    }
}

impl Registry {
    /// Insert (id, ptr). Returns false when full.
    fn insert(&mut self, id: u64, ptr: usize) -> bool {
        if self.used == MAX_EVENTS {
            return false;
        }
        self.slots[self.used] = (id, ptr);
        self.used += 1;
        true
    }

    /// Remove by id (swap with last). Returns the pointer, or None.
    fn remove(&mut self, id: u64) -> Option<usize> {
        let idx = self.slots[..self.used].iter().position(|&(i, _)| i == id)?;
        let entry = self.slots[idx];
        self.used -= 1;
        self.slots[idx] = self.slots[self.used];
        Some(entry.1)
    }
}

/// id → event. Every take is lock_irqsave (module lock-order doc);
/// with the fixed array, no code path allocates while holding it.
static REGISTRY: Spinlock<Registry> = Spinlock::new(new_registry());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn register(ev: *mut PerfEvent, starts_armed: bool) -> bool {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // SAFETY: caller built the box; the registry keeps the pointer
    // valid until unregister.
    unsafe {
        (*ev).id = id;
    }
    let mut reg = REGISTRY.lock_irqsave();
    let ok = reg.insert(id, ev as usize);
    drop(reg);
    if !ok {
        return false;
    }
    if starts_armed {
        ARMED.fetch_add(1, Ordering::Release);
    }
    true
}

/// Remove `id` from the registry. Returns the event pointer (the
/// caller reclaims the box), or None when absent.
fn unregister(id: u64) -> Option<usize> {
    let mut reg = REGISTRY.lock_irqsave();
    reg.remove(id)
}

/// The per-CPU wake list: a FIXED array, not a Vec — softirq_tick
/// used to push into a Vec while holding REGISTRY (irqsave), and the
/// first push's allocation was one leg of the session-13 deadlock.
/// Only softirq_tick on the owning CPU touches its buffer, and the
/// same softirq vector cannot nest on one CPU, so plain relaxed
/// atomics serialize it against nothing.
struct WakeBuf {
    n: AtomicUsize,
    ptrs: [AtomicUsize; MAX_EVENTS],
}

const fn new_wake_buf() -> WakeBuf {
    WakeBuf {
        n: AtomicUsize::new(0),
        ptrs: [const { AtomicUsize::new(0) }; MAX_EVENTS],
    }
}

static WAKE: [WakeBuf; crate::config::MAX_CPUS] =
    [const { new_wake_buf() }; crate::config::MAX_CPUS];

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
    let mode = s.mode.load(Ordering::Relaxed);
    let tid = s.tid.load(Ordering::Relaxed);

    // The wake list is this CPU's FIXED buffer — nothing in the
    // charging path allocates (no-alloc-under-lock, session 13).
    let wake = &WAKE[c];
    let mut nwake = 0usize;
    {
        let reg = REGISTRY.lock_irqsave();
        for idx in 0..reg.used {
            let (event_id, p) = reg.slots[idx];
            // SAFETY: registry membership guarantees validity (close
            // unregisters before freeing).
            let ev = unsafe { &*(p as *const PerfEvent) };
            let _ = event_id;
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
            let emitted = {
                let mut inner = ev.inner.lock();
                let charge = pending as u64 * TICK_NS;
                inner.counter = inner.counter.saturating_add(charge);
                inner.total = inner.total.saturating_add(charge);
                let mut emitted = 0u64;
                while inner.counter >= ev.sample_period && ev.sample_period > 0 {
                    inner.counter -= ev.sample_period;
                    let rec_len = if ev.wants_callchain {
                        // nr field always; frames only for kernel ticks
                        // (user ticks carry an honest nr=0).
                        SAMPLE_REC_LEN as usize
                            + 8
                            + if mode == MODE_USER {
                                0
                            } else {
                                8 * (1 + s.chain_len.load(Ordering::Relaxed) as usize)
                            }
                    } else {
                        SAMPLE_REC_LEN as usize
                    };
                    if inner.ring.len() + rec_len <= RING_CAP {
                        push_sample(
                            &mut inner.ring,
                            pc,
                            mode,
                            ev.wants_callchain,
                            &s.chain,
                            s.chain_len.load(Ordering::Relaxed) as usize,
                        );
                        emitted += 1;
                    } else {
                        inner.lost += 1;
                    }
                }
                emitted
            };
            if emitted > 0 && nwake < MAX_EVENTS {
                wake.ptrs[nwake].store(p, Ordering::Relaxed);
                nwake += 1;
            }
        }
    }
    wake.n.store(nwake, Ordering::Relaxed);
    // Wakes with no locks held (timerfd expire-notify discipline).
    for i in 0..nwake {
        // SAFETY: entries were registry members under the lock above;
        // close unregisters (and so skips) before freeing, and the
        // registry cannot have been re-locked between store and wake
        // on this CPU — the wake list is private to this softirq pass.
        let p = wake.ptrs[i].load(Ordering::Relaxed) as *mut PerfEvent;
        if !p.is_null() {
            unsafe { (*p).wait_queue.wake_up_all() };
        }
    }
}

/// One PERF_RECORD_SAMPLE with PERF_SAMPLE_IP: header (8 B) + ip (8 B)
/// — the minimum record this slice emits.
const SAMPLE_REC_LEN: usize = 16;
/// The longest record: IP-only head + nr + [CONTEXT_KERNEL + frames].
const SAMPLE_REC_MAX: usize = SAMPLE_REC_LEN + 8 + 8 * (1 + CHAIN_FRAMES);
/// PERF_RECORD_LOST: header (8 B) + u64 id + u64 lost.
const LOST_REC_LEN: usize = 24;

/// Append one sample record. With `callchain`, the ABI's layout after
/// the header: ip (u64), nr (u64), then `nr` u64 entries — the
/// PERF_CONTEXT_KERNEL marker followed by the walked kernel return
/// addresses (the interrupted PC rides in the ip field). Kernel-mode
/// ticks carry the walked chain; user-mode ticks carry nr=0 — Rux has
/// no user-stack walker, and an empty chain is the honest value (the
/// reference emits user chains there; that machinery stays open).
fn push_sample(
    ring: &mut VecDeque<u8>,
    pc: u64,
    mode: u8,
    callchain: bool,
    chain: &[AtomicU64; CHAIN_FRAMES],
    chain_len: usize,
) {
    let misc = if mode == MODE_USER {
        PERF_RECORD_MISC_USER
    } else {
        PERF_RECORD_MISC_KERNEL
    };
    let n_frames = if callchain && mode != MODE_USER {
        chain_len.min(CHAIN_FRAMES)
    } else {
        0
    };
    let nr = if callchain {
        if mode == MODE_USER {
            0
        } else {
            (1 + n_frames) as u64 // the kernel marker + the frames
        }
    } else {
        // IP-only records do not carry the nr field at all.
        0
    };
    let carries_nr = callchain;
    let total = SAMPLE_REC_LEN + if carries_nr { 8 + 8 * nr as usize } else { 0 };
    let mut rec = [0u8; SAMPLE_REC_MAX];
    rec[0..4].copy_from_slice(&PERF_RECORD_SAMPLE.to_le_bytes());
    rec[4..6].copy_from_slice(&misc.to_le_bytes());
    rec[6..8].copy_from_slice(&(total as u16).to_le_bytes());
    rec[8..16].copy_from_slice(&pc.to_le_bytes());
    if carries_nr {
        rec[16..24].copy_from_slice(&nr.to_le_bytes());
        if nr > 0 {
            rec[24..32].copy_from_slice(&PERF_CONTEXT_KERNEL.to_le_bytes());
        }
        for i in 0..n_frames {
            let v = chain[i].load(Ordering::Relaxed);
            rec[32 + i * 8..40 + i * 8].copy_from_slice(&v.to_le_bytes());
        }
    }
    for b in &rec[..total] {
        ring.push_back(*b);
    }
}

/// PERF_RECORD_LOST record bytes for (id, lost).
pub(crate) fn lost_record_bytes(id: u64, lost: u64) -> [u8; LOST_REC_LEN] {
    let mut rec = [0u8; LOST_REC_LEN];
    rec[0..4].copy_from_slice(&PERF_RECORD_LOST.to_le_bytes());
    rec[4..6].copy_from_slice(&0u16.to_le_bytes());
    rec[6..8].copy_from_slice(&(LOST_REC_LEN as u16).to_le_bytes());
    rec[8..16].copy_from_slice(&id.to_le_bytes());
    rec[16..24].copy_from_slice(&lost.to_le_bytes());
    rec
}

/// Drain whole records into `buf`. Emits the pending LOST record
/// (if any) first, then samples (variable length — the header's own
/// size field says how long each record is). Returns bytes copied;
/// Err(()) = a record is pending but does not fit `buf` at all (the
/// caller surfaces EINVAL).
fn drain_records(ev: &PerfEvent, buf: &mut [u8]) -> Result<usize, ()> {
    let mut inner = ev.inner.lock();
    let mut copied = 0usize;

    // Overflow report first, per the ABI's lost-record contract.
    if inner.lost > 0 {
        if buf.len() < LOST_REC_LEN {
            return Err(());
        }
        let rec = lost_record_bytes(ev.id, inner.lost);
        buf[..LOST_REC_LEN].copy_from_slice(&rec);
        copied += LOST_REC_LEN;
        inner.lost = 0;
    }

    while inner.ring.len() >= SAMPLE_REC_LEN && buf.len() - copied >= SAMPLE_REC_LEN {
        // The record's own length: perf_event_header.size at bytes 6..8
        // of the queued record.
        let mut hdr = [0u8; 8];
        for (i, h) in hdr.iter_mut().enumerate() {
            *h = *inner.ring.get(i).unwrap_or(&0);
        }
        let rec_len = u16::from_le_bytes([hdr[6], hdr[7]]) as usize;
        if !(SAMPLE_REC_LEN..=SAMPLE_REC_MAX).contains(&rec_len) {
            // A corrupted queue entry (should not happen): drop one
            // byte and rescan rather than trust it.
            inner.ring.pop_front();
            continue;
        }
        if buf.len() - copied < rec_len {
            break;
        }
        for b in buf[copied..copied + rec_len].iter_mut() {
            *b = inner.ring.pop_front().unwrap_or(0);
        }
        copied += rec_len;
    }

    if copied == 0 && !inner.ring.is_empty() {
        // A sample is pending but buf cannot hold one record.
        return Err(());
    }
    Ok(copied)
}

/// Are records pending (poll readiness)?
fn records_pending(ev: &PerfEvent) -> bool {
    let inner = ev.inner.lock();
    !inner.ring.is_empty() || inner.lost > 0
}

/// Drain one event's pending records into a fresh Vec — the per-event
/// debugfs file /sys/kernel/debug/perf/<id>/records is the stopgap
/// sample channel while the mmap ring is unimplemented (read(2) is the
/// reference's counter read; the reference delivers samples through
/// the ring only). The registry is held across the drain per the
/// module's lock order (REGISTRY → inner; close unregisters before it
/// frees, so the pointer cannot dangle under this lock). Budget: 8 KiB
/// per open — the drain is repeatable, and the bound keeps the
/// irqsave section short.
fn drain_records_by_id(id: u64) -> alloc::vec::Vec<u8> {
    const DRAIN_BUDGET: usize = 8 * 1024;
    // The output buffer is allocated BEFORE the registry is taken —
    // nothing allocates under REGISTRY (session 13's rule).
    let mut out = alloc::vec![0u8; DRAIN_BUDGET + SAMPLE_REC_MAX];
    let reg = REGISTRY.lock_irqsave();
    let ptr = match reg.slots[..reg.used].iter().find(|&&(i, _)| i == id) {
        Some(&(_, p)) => p as *const PerfEvent,
        None => {
            out.clear();
            return out;
        }
    };
    // SAFETY: registry membership guarantees validity (close
    // unregisters before freeing, and cannot while REGISTRY is held).
    let ev = unsafe { &*ptr };
    match drain_records(ev, &mut out) {
        Ok(n) => {
            out.truncate(n);
            out
        }
        Err(()) => {
            out.clear();
            out
        }
    }
}

// ==================== File operations ====================

/// read(2): the reference's counter read — NEVER blocks. The layout
/// is the man page's reading section verbatim: a u64 counter, then
/// (per attr.read_format) u64 total_time_enabled, u64 total_time_running,
/// u64 id. For these software clock events the counter is the ns
/// charged while the event was counting; running is the same charged
/// ns (the software clock only advances while charging); enabled is
/// the wall ns since open while armed. Sample records are NOT read(2)
/// output — the reference delivers samples through the mmap ring
/// (unimplemented here); the stopgap record channel is the per-event
/// debugfs drain file /sys/kernel/debug/perf/<id>/records.
///
/// The blocking-drain shape this replaces self-deadlocked by
/// construction on a task-scoped event read from the same task: the
/// reader sleeps, the task never runs, no ticks charge it, no sample
/// ever arrives (stress fleet session 13, fsfuzz perf arm).
fn perf_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data is an UnsafeCell; we hold &File so no
    // concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -(crate::errno::constants::EBADF as isize),
    };
    // SAFETY: installed by sys_perf_event_open as Box::into_raw; valid
    // while the File exists.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    match read_counter(ev, buf) {
        Some(n) => n as isize,
        None => -(crate::errno::constants::EINVAL as isize),
    }
}

/// The counter read's layout core (perf_read's body as a test seam):
/// u64 count, then (per attr.read_format) u64 total_time_enabled, u64
/// total_time_running, u64 id. None when `buf` cannot hold the layout.
fn read_counter(ev: &PerfEvent, buf: &mut [u8]) -> Option<usize> {
    let fmt = ev.read_format;
    let mut need = 8usize; // the counter
    if fmt & PERF_FORMAT_TOTAL_TIME_ENABLED != 0 {
        need += 8;
    }
    if fmt & PERF_FORMAT_TOTAL_TIME_RUNNING != 0 {
        need += 8;
    }
    if fmt & PERF_FORMAT_ID != 0 {
        need += 8;
    }
    if buf.len() < need {
        return None;
    }

    let (count, enabled, running, id) = {
        let inner = ev.inner.lock();
        let enabled = if ev.enabled_at != 0 {
            crate::sched::fair::sched_clock().saturating_sub(ev.enabled_at)
        } else {
            0
        };
        (inner.total, enabled, inner.total, ev.id)
    };
    let mut at = 0usize;
    let mut put = |v: u64| {
        buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
        at += 8;
    };
    put(count);
    if fmt & PERF_FORMAT_TOTAL_TIME_ENABLED != 0 {
        put(enabled);
    }
    if fmt & PERF_FORMAT_TOTAL_TIME_RUNNING != 0 {
        put(running);
    }
    if fmt & PERF_FORMAT_ID != 0 {
        put(id);
    }
    Some(at)
}

fn perf_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;
    // SAFETY: see perf_read.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: as above.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    if events & POLLIN != 0 && records_pending(ev) {
        ready |= POLLIN | POLLRDNORM;
    }
    ready
}

fn perf_close(file: &crate::fs::File) -> i32 {
    // SAFETY: see perf_read.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    // SAFETY: as above.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    let id = ev.id;
    let wants_chain = ev.wants_callchain;
    let was_armed = ev.enabled.load(Ordering::Acquire);
    let _ = unregister(id);
    if was_armed {
        ARMED.fetch_sub(1, Ordering::Release);
    }
    if wants_chain {
        CALLCHAIN_ARMED.fetch_sub(1, Ordering::Release);
    }
    // Drop the drain file first (after the unregister above, its
    // generator already answers empty; removing the name ends new
    // opens). Stale opens keep their snapshot — debugfs nodes are Arc'd.
    {
        let path = alloc::format!("/perf/{}", id);
        crate::fs::debugfs::remove_file(&path);
    }
    // Residual race (mirrors timerfd_close): a reader still inside
    // perf_read holds &File via its own reference; close runs on the
    // last fd release, and the same one-softirq-pass window applies.
    // The registry removal above already stopped all sampler access.
    // SAFETY: ptr came from Box::into_raw in sys_perf_event_open.
    unsafe {
        let _ = Box::from_raw(ptr as *mut PerfEvent);
    }
    unsafe { *file.private_data.get() = None; }
    0
}

/// perf event file operations: read = the reference counter read
/// (never blocks), poll = record readiness (the drain channel's
/// signal), close unregisters.
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
    // SAFETY: dispatcher identity-checked the File against PERF_OPS.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return false,
    };
    // SAFETY: as above.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    ev.wait_queue
        .add(crate::process::wait::WaitQueueEntry::new(task, false));
    true
}

pub(crate) fn perf_poll_unwait(
    file: &crate::fs::File,
    task: *mut crate::process::task::Task,
) {
    if let Some(p) = unsafe { *file.private_data.get() } {
        // SAFETY: as above.
        let ev = unsafe { &*(p as *const PerfEvent) };
        ev.wait_queue.remove(task);
    }
}

// ==================== syscall ====================

/// sys_perf_event_open(attr, pid, cpu, group_fd, flags) — the scoped
/// software-clock slice. See the module doc for what is accepted;
/// everything unsupported returns EINVAL (validated configs we cannot
/// honor) rather than silently misbehaving.
pub fn sys_perf_event_open(args: crate::syscall::SyscallArgs) -> i64 {
    let attr_ptr = args[0] as *const AttrHead;
    let pid = args[1] as i32;
    let cpu = args[2] as i32;
    let group_fd = args[3] as i32;
    let flags = args[4];

    // 1. Flags: only the bits this slice can honor. FD_NO_GROUP is
    //    accepted as a no-op (no event groups exist here, so every
    //    event already satisfies it); FD_CLOEXEC is honored on the fd.
    //    FD_OUTPUT (ring redirect) and PID_CGROUP (cgroup scoping)
    //    are real ABI features we do not implement — EINVAL, the same
    //    answer the reference gives PID_CGROUP without CONFIG_CGROUP_PERF.
    if flags & !(PERF_FLAG_FD_NO_GROUP | PERF_FLAG_FD_CLOEXEC) != 0 {
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 2. Read the attr head. The mandatory part is 64 bytes; we only
    // decode the first 48 but require the user's size to cover VER0.
    if !crate::arch::uaccess::access_ok(attr_ptr as usize, 64) {
        return -(crate::errno::constants::EFAULT as i64);
    }
    // SAFETY: access_ok passed; get_user re-verifies page-wise.
    let attr = match unsafe {
        crate::arch::uaccess::get_user(attr_ptr as *const AttrHead)
    } {
        Some(a) => a,
        None => return -(crate::errno::constants::EFAULT as i64),
    };
    if attr.size < 64 || attr.size > 1024 {
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 3. Event class.
    if attr.type_ != PERF_TYPE_SOFTWARE {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if attr.config != PERF_COUNT_SW_CPU_CLOCK && attr.config != PERF_COUNT_SW_TASK_CLOCK {
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 4. Sampling mode: period-based; sample_type is IP or
    // IP|CALLCHAIN (this slice fills exactly those).
    if attr.flags1 & FLAG_FREQ != 0 {
        return -(crate::errno::constants::EINVAL as i64); // freq mode not implemented
    }
    if attr.sample_type != PERF_SAMPLE_IP
        && attr.sample_type != (PERF_SAMPLE_IP | PERF_SAMPLE_CALLCHAIN)
    {
        return -(crate::errno::constants::EINVAL as i64); // this slice fills exactly IP (+CALLCHAIN)
    }
    let wants_callchain = attr.sample_type & PERF_SAMPLE_CALLCHAIN != 0;
    // read_format: the counter-read tail bits are implemented;
    // anything else (PERF_FORMAT_GROUP foremost) is refused — EINVAL.
    if attr.read_format
        & !(PERF_FORMAT_TOTAL_TIME_ENABLED
            | PERF_FORMAT_TOTAL_TIME_RUNNING
            | PERF_FORMAT_ID)
        != 0
    {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if attr.flags1 & FLAG_INHERIT != 0 {
        return -(crate::errno::constants::EINVAL as i64); // no child inheritance
    }
    if attr.sample_period == 0 {
        return -(crate::errno::constants::EINVAL as i64);
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

    let (target_tid, target_cpu, starts_armed);
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

    starts_armed = attr.flags1 & FLAG_DISABLED == 0;

    // 6. Build + install the fd.
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(crate::errno::constants::EMFILE as i64),
    };

    let ev = Box::new(PerfEvent {
        id: 0,
        config: attr.config,
        sample_period: attr.sample_period,
        wants_callchain,
        target_tid,
        target_cpu,
        exclude_user: attr.flags1 & FLAG_EXCLUDE_USER != 0,
        exclude_kernel: attr.flags1 & FLAG_EXCLUDE_KERNEL != 0,
        exclude_idle: attr.flags1 & FLAG_EXCLUDE_IDLE != 0,
        read_format: attr.read_format,
        enabled_at: if starts_armed {
            crate::sched::fair::sched_clock()
        } else {
            0
        },
        enabled: AtomicBool::new(starts_armed),
        inner: Spinlock::new(PerfInner {
            counter: 0,
            total: 0,
            // Capacity reserved at construction (process context, no
            // locks held): the charging path's push_back can then
            // never allocate under `inner` (session 13's rule).
            ring: VecDeque::with_capacity(RING_CAP),
            lost: 0,
        }),
        wait_queue: WaitQueueHead::new(),
    });
    let ev_ptr = Box::into_raw(ev);
    if !register(ev_ptr, starts_armed) {
        // Registry full — the fd-exhaustion errno class. No ARMED or
        // CALLCHAIN_ARMED adjustments: register refused before them.
        // SAFETY: reclaim the box.
        unsafe {
            let _ = Box::from_raw(ev_ptr);
        }
        return -(crate::errno::constants::EMFILE as i64);
    }
    if wants_callchain {
        // The hard-IRQ hook walks chains only while a live event asks.
        CALLCHAIN_ARMED.fetch_add(1, Ordering::Release);
    }

    let file_flags = crate::fs::file::FileFlags::O_RDONLY;
    let file = Arc::new(crate::fs::File::new(
        crate::fs::file::FileFlags::new(file_flags),
    ));
    file.set_ops(&PERF_OPS);
    file.set_private_data(ev_ptr as *mut u8);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            abort_event(ev_ptr, starts_armed, wants_callchain);
            return -(crate::errno::constants::EMFILE as i64);
        }
    };

    match fdtable.install_fd(fd, file) {
        Ok(()) => {
            if flags & PERF_FLAG_FD_CLOEXEC != 0 {
                crate::fs::set_cloexec_fd(fd, true);
            }
            // The stopgap sample channel: one drain file per event
            // under /sys/kernel/debug/perf/. Removed at close. A
            // creation failure (debugfs not mounted) leaves the event
            // fully functional — the drain channel is optional, the
            // ABI (read/poll) does not depend on it.
            {
                let id = unsafe { (*ev_ptr).id };
                if crate::fs::debugfs::create_dir("/perf") {
                    let path = alloc::format!("/perf/{}", id);
                    crate::fs::debugfs::create_file(
                        &path,
                        0o400,
                        move || drain_records_by_id(id),
                        None::<fn(&[u8]) -> i32>,
                    );
                }
            }
            fd as i64
        }
        Err(_) => {
            abort_event(ev_ptr, starts_armed, wants_callchain);
            -(crate::errno::constants::EMFILE as i64)
        }
    }
}

/// Undo a successful open that failed at the fd layer: unregister,
/// unwind the armed counters, reclaim the box.
fn abort_event(ev_ptr: *mut PerfEvent, was_armed: bool, wants_chain: bool) {
    // SAFETY: the caller built the box and owns it again.
    let id = unsafe { (*ev_ptr).id };
    let _ = unregister(id);
    if was_armed {
        ARMED.fetch_sub(1, Ordering::Release);
    }
    if wants_chain {
        CALLCHAIN_ARMED.fetch_sub(1, Ordering::Release);
    }
    // SAFETY: reclaim the box.
    unsafe {
        let _ = Box::from_raw(ev_ptr);
    }
}

// ==================== unit tests ====================

#[cfg(any(test, feature = "unit-test"))]
pub mod tests {
    use super::*;

    fn sample_record(pc: u64, user: bool) -> [u8; SAMPLE_REC_LEN] {
        let mut rec = [0u8; SAMPLE_REC_LEN];
        rec[0..4].copy_from_slice(&PERF_RECORD_SAMPLE.to_le_bytes());
        let misc = if user { PERF_RECORD_MISC_USER } else { PERF_RECORD_MISC_KERNEL };
        rec[4..6].copy_from_slice(&misc.to_le_bytes());
        rec[6..8].copy_from_slice(&(SAMPLE_REC_LEN as u16).to_le_bytes());
        rec[8..16].copy_from_slice(&pc.to_le_bytes());
        rec
    }

    fn test_event(id: u64, wants_callchain: bool, lost: u64) -> PerfEvent {
        PerfEvent {
            id,
            config: PERF_COUNT_SW_CPU_CLOCK,
            sample_period: TICK_NS,
            wants_callchain,
            target_tid: 0,
            target_cpu: 0,
            exclude_user: false,
            exclude_kernel: false,
            exclude_idle: false,
            read_format: 0,
            enabled_at: 0,
            enabled: AtomicBool::new(true),
            inner: Spinlock::new(PerfInner {
                counter: 0,
                total: 0,
                // Reserved like the real construction — the tests push
                // under `inner`, so the same no-alloc rule applies.
                ring: VecDeque::with_capacity(RING_CAP),
                lost,
            }),
            wait_queue: WaitQueueHead::new(),
        }
    }

    fn zero_chain() -> [AtomicU64; CHAIN_FRAMES] {
        [const { AtomicU64::new(0) }; CHAIN_FRAMES]
    }

    /// The PERF_FLAG_* family carries the uapi bit values verbatim
    /// (the stress lane's fsfuzz arm caught them shifted: NO_GROUP at
    /// 1<<1, PID_CGROUP at 1<<4 — reference callers got EINVAL).
    pub fn test_flag_values() {
        let ok = PERF_FLAG_FD_NO_GROUP == 1 << 0
            && PERF_FLAG_FD_OUTPUT == 1 << 1
            && PERF_FLAG_PID_CGROUP == 1 << 2
            && PERF_FLAG_FD_CLOEXEC == 1 << 3;
        if ok {
            crate::tests::test_pass("perf-flag-values");
        } else {
            crate::tests::test_fail("perf-flag-values", "flag bits diverge from the uapi");
        }
    }

    /// read(2)'s counter layout: 8/16/24/32 bytes per read_format, the
    /// count first, then enabled/running/id — and it is computed from
    /// PerfInner.total (never decremented by sampling), not the
    /// sample-phase residual.
    pub fn test_read_layout() {
        let mut ev = test_event(77, false, 0);
        {
            let mut inner = ev.inner.lock();
            inner.total = 1_500_000;
            inner.counter = 100_000; // residual since last sample
        }
        let mut buf = [0u8; 40];
        // Default (fmt 0): the count alone.
        ev.read_format = 0;
        let n = read_counter(&ev, &mut buf);
        let mut ok = n == 8 && u64::from_le_bytes(buf[0..8].try_into().unwrap()) == 1_500_000;
        // Both TIME bits: count, enabled (0 — never stamped), running
        // (== the charged count).
        ev.read_format =
            PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING | PERF_FORMAT_ID;
        let n = read_counter(&ev, &mut buf);
        ok &= n == 32;
        let count = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let enabled = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let running = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let id = u64::from_le_bytes(buf[24..32].try_into().unwrap());
        ok &= count == 1_500_000 && enabled == 0 && running == 1_500_000 && id == 77;
        if ok {
            crate::tests::test_pass("perf-read-layout");
        } else {
            crate::tests::test_fail("perf-read-layout", "read(2) bytes diverge from the layout");
        }
    }

    /// Record encoding: sample and lost records match the ABI layout.
    pub fn test_record_encoding() {
        let r = sample_record(0x1234_5678_9abc_def0, true);
        let expect: [u8; 16] = [
            9, 0, 0, 0, // type=PERF_RECORD_SAMPLE
            1, 0, // misc=USER
            16, 0, // size=16
            0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12, // ip LE
        ];
        if r != expect {
            crate::tests::test_fail("perf-record", "sample record bytes mismatch");
            return;
        }
        let l = lost_record_bytes(7, 3);
        let mut ok = true;
        ok &= l[0..4] == PERF_RECORD_LOST.to_le_bytes();
        ok &= l[6..8] == (LOST_REC_LEN as u16).to_le_bytes();
        ok &= l[8..16] == 7u64.to_le_bytes();
        ok &= l[16..24] == 3u64.to_le_bytes();
        if ok {
            crate::tests::test_pass("perf-record");
        } else {
            crate::tests::test_fail("perf-record", "lost record bytes mismatch");
        }
    }

    /// push_sample/drain round trip through the byte ring.
    pub fn test_ring_roundtrip() {
        let ev = test_event(42, false, 0);
        {
            let mut inner = ev.inner.lock();
            let chain = zero_chain();
            push_sample(&mut inner.ring, 0xaabb, MODE_KERNEL, false, &chain, 0);
            push_sample(&mut inner.ring, 0xccdd, MODE_USER, false, &chain, 0);
        }
        let mut buf = [0u8; 40];
        match drain_records(&ev, &mut buf) {
            Ok(n) => {
                let want0 = sample_record(0xaabb, false);
                let want1 = sample_record(0xccdd, true);
                if n == 32 && buf[..16] == want0 && buf[16..32] == want1 {
                    crate::tests::test_pass("perf-ring");
                } else {
                    crate::tests::test_fail("perf-ring", "drained bytes mismatch");
                }
            }
            Err(()) => {
                crate::tests::test_fail("perf-ring", "unexpected EINVAL");
            }
        }
        // Lost-first ordering: a lost count precedes ring samples.
        {
            let ev2 = test_event(43, false, 5);
            {
                let mut inner = ev2.inner.lock();
                let chain = zero_chain();
                push_sample(&mut inner.ring, 0x1, MODE_KERNEL, false, &chain, 0);
            }
            let mut b2 = [0u8; 64];
            match drain_records(&ev2, &mut b2) {
                Ok(n) => {
                    let lost = lost_record_bytes(43, 5);
                    if n == 40 && b2[..24] == lost {
                        crate::tests::test_pass("perf-lost");
                    } else {
                        crate::tests::test_fail("perf-lost", "lost record not first");
                    }
                }
                Err(()) => {
                    crate::tests::test_fail("perf-lost", "unexpected EINVAL");
                }
            }
        }
    }

    /// A callchain sample: variable-length record, ABI layout
    /// (ip, nr, PERF_CONTEXT_KERNEL marker, frames), drained whole.
    pub fn test_callchain_record() {
        let ev = test_event(45, true, 0);
        {
            let mut inner = ev.inner.lock();
            let mut chain = zero_chain();
            chain[0].store(0x8000_1000, Ordering::Relaxed);
            chain[1].store(0x8000_2000, Ordering::Relaxed);
            push_sample(&mut inner.ring, 0x5566, MODE_KERNEL, true, &chain, 2);
        }
        let mut buf = [0u8; 64];
        match drain_records(&ev, &mut buf) {
            Ok(n) => {
                // 8 hdr + 8 ip + 8 nr + 3*8 ips = 48 bytes.
                let ok = n == 48
                    && buf[0..4] == PERF_RECORD_SAMPLE.to_le_bytes()
                    && buf[6..8] == 48u16.to_le_bytes()
                    && buf[8..16] == 0x5566u64.to_le_bytes()
                    && buf[16..24] == 3u64.to_le_bytes()
                    && buf[24..32] == PERF_CONTEXT_KERNEL.to_le_bytes()
                    && buf[32..40] == 0x8000_1000u64.to_le_bytes()
                    && buf[40..48] == 0x8000_2000u64.to_le_bytes();
                if ok {
                    crate::tests::test_pass("perf-callchain");
                } else {
                    crate::tests::test_fail("perf-callchain", "chain layout mismatch");
                }
            }
            Err(()) => {
                crate::tests::test_fail("perf-callchain", "unexpected EINVAL");
            }
        }
        // Mixed queue: a 16-byte IP-only record then a chain record —
        // the drain must respect each record's own length.
        {
            let ev3 = test_event(46, false, 0);
            {
                let mut inner = ev3.inner.lock();
                let chain = zero_chain();
                chain[0].store(0xdead_beef, Ordering::Relaxed);
                push_sample(&mut inner.ring, 0x1, MODE_USER, false, &chain, 0);
                push_sample(&mut inner.ring, 0x2, MODE_KERNEL, true, &chain, 1);
            }
            let mut b3 = [0u8; 64];
            match drain_records(&ev3, &mut b3) {
                Ok(n) => {
                    let ok = n == 16 + 40
                        && b3[6..8] == 16u16.to_le_bytes()
                        && b3[16 + 6..16 + 8] == 40u16.to_le_bytes();
                    if ok {
                        crate::tests::test_pass("perf-mixedlen");
                    } else {
                        crate::tests::test_fail("perf-mixedlen", "mixed lengths mismatch");
                    }
                }
                Err(()) => {
                    crate::tests::test_fail("perf-mixedlen", "unexpected EINVAL");
                }
            }
        }
    }

    /// A buffer too small for one record is EINVAL.
    pub fn test_short_buffer() {
        let ev = test_event(44, false, 0);
        {
            let mut inner = ev.inner.lock();
            let chain = zero_chain();
            push_sample(&mut inner.ring, 0x99, MODE_USER, false, &chain, 0);
        }
        let mut buf = [0u8; 8];
        match drain_records(&ev, &mut buf) {
            Ok(_) => crate::tests::test_fail("perf-short", "short buffer accepted"),
            Err(()) => crate::tests::test_pass("perf-short"),
        }
    }
}
