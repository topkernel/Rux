//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Whole-syscall cost table + tick-mode accounting + kernel-rip page
//! histogram for the systematic performance-audit line
//! (`feature/perf-audit`, docs/development/perf-audit.md).
//!
//! Everything here is armed by the kernel cmdline `syscost=1` and costs
//! nothing when off (one cached atomic load per syscall exit; the tick
//! hooks are two relaxed adds). No `dfx/switches.rs` bit is consumed —
//! that file belongs to the wild-write line; this probe owns its own
//! cmdline key so the branches can merge without coordination.
//!
//! What it measures and why:
//!
//! 1. `enter`/`exit` around `syscall::dispatch::syscall_handler` —
//!    per-native-nr { count, wall cycles, max } plus a small ring of
//!    >4 ms outliers with the caller comm. Wall cycles include blocking
//!    (futex sleep, pipe waits) — deliberately: the ranking question is
//!    "where does the 16-23 min OH boot spend wall time", and CPU-vs-
//!    block is separated by the tick-mode counters below. Diverging
//!    paths (do_exit from exit_group / fatal signals) skip the exit
//!    hook, so those calls are undercounted; noted in the report.
//!
//! 2. Tick-mode counters per CPU — user / kernel-task / idle — from the
//!    same timer-tick site the stime accounting uses. `idle` means the
//!    tick landed with no user address space current (idle task or the
//!    boot kernel context). This decomposes the boot into "CPU-saturated
//!    in kernel", "CPU-saturated in user" and "idle-blocked" — the
//!    first split of the systematic diagnosis.
//!
//! 3. A 256-slot kernel-text page histogram sampled on kernel-mode
//!    ticks (same sample site as sc105's STIME_HIST, but global and
//!    comm-free so no task starves the table; per-task attribution
//!    comes from taskdump's utime/stime columns). Pages are symbolized
//!    offline against the built ELF's nm(1) output.
//!
//! Dump: `tick()` is called from the timer softirq; every
//! SYSCOST_DUMP_SECS it prints the tables (sorted, compact, one line
//! each) plus the current uptime so host-side serial timestamps can be
//! correlated with guest time.

use core::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::alloc::string::ToString;

/// Syscall-number slots (native x86_64 numbers go up to ~335; generic
/// table numbers stay below; 512 leaves headroom).
const NR_SLOTS: usize = 512;

/// Kernel-text page histogram slots.
const RIP_SLOTS: usize = 256;

/// Slow-op ring size (>4 ms wall).
const SLOW_RING: usize = 64;

/// Slow-op threshold in nominal ticks (100 ns each): 4 ms. A normal
/// fast syscall is 1-30 us; only genuine outliers cross this.
const SLOW_CYC: u64 = 40_000;

/// Dump cadence in jiffies (100 Hz -> 60 s).
const SYSCOST_DUMP_SECS: u64 = 60;

struct SysStat {
    count: AtomicU64,
    cyc: AtomicU64,
    max: AtomicU64,
}

const EMPTY_SYS: SysStat = SysStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

static TABLE: [SysStat; NR_SLOTS] = [EMPTY_SYS; NR_SLOTS];

struct RipSlot {
    page: AtomicU64,
    count: AtomicU64,
}

const EMPTY_RIP: RipSlot = RipSlot {
    page: AtomicU64::new(0),
    count: AtomicU64::new(0),
};

static RIP_HIST: [RipSlot; RIP_SLOTS] = [EMPTY_RIP; RIP_SLOTS];

struct SlowEnt {
    nr: AtomicU64,
    cyc: AtomicU64,
    comm: [AtomicU8; 16],
}

const EMPTY_SLOW: SlowEnt = SlowEnt {
    nr: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
};

static SLOW: [SlowEnt; SLOW_RING] = [EMPTY_SLOW; SLOW_RING];
static SLOW_IDX: AtomicUsize = AtomicUsize::new(0);

/// Per-CPU tick modes: user / kernel-task / idle.
static TICK_USER: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
static TICK_KERN: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
static TICK_IDLE: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

static LAST_DUMP: AtomicU64 = AtomicU64::new(0);

/// Armed cache: 0 unknown, 1 off, 2 on (cmdline `syscost=1`).
static ARMED: AtomicU8 = AtomicU8::new(0);


struct EofSpin {
    comm: [AtomicU8; 16],
    path: [AtomicU8; 96],
    n: AtomicU64,
}

const EMPTY_EOF: EofSpin = EofSpin {
    comm: [const { AtomicU8::new(0) }; 16],
    path: [const { AtomicU8::new(0) }; 96],
    n: AtomicU64::new(0),
};

static EOF_SPIN: [EofSpin; 12] = [const { EMPTY_EOF }; 12];

static EAGAIN_SPIN: [EofSpin; 12] = [const { EMPTY_EOF }; 12];

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
struct RdDataSlot {
    /// First 8 comm bytes as one u64 — fast scan pre-filter (comm
    /// compare is the per-read hot path; "hilogd.s*" vs "hilogd.r*"
    /// differ here, and a filter miss is one load).
    comm0: AtomicU64,
    comm: [AtomicU8; 16],
    file_id: AtomicU64,
    fd: AtomicU64,
    ops: AtomicU64,
    n_cur: AtomicU64,
    n_total: AtomicU64,
    bytes: AtomicU64,
    path: [AtomicU8; 96],
}

const EMPTY_RD: RdDataSlot = RdDataSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    file_id: AtomicU64::new(0),
    fd: AtomicU64::new(0),
    ops: AtomicU64::new(0),
    n_cur: AtomicU64::new(0),
    n_total: AtomicU64::new(0),
    bytes: AtomicU64::new(0),
    path: [const { AtomicU8::new(0) }; 96],
};

static RD_DATA: [RdDataSlot; 16] = [const { EMPTY_RD }; 16];

/// pa10 failing-read census: (comm, errno, fd-resolution) of reads that
/// return a negative errno other than EAGAIN. pa9 closed the remaining
/// gap: hilogd.rd_kmsg issues 15.4M reads/boot that return neither data
/// (positive-read census: 195 reads total) nor EOF/EAGAIN (censuses
/// fire and list other comms) — the only shape left is a failing errno,
/// and WHICH errno (EBADF from a closed/write-only fd? EFAULT?) decides
/// kernel-fix vs OH-behavior routing. Once per comm.
struct RdFailSlot {
    comm0: AtomicU64,
    comm: [AtomicU8; 16],
    ret: AtomicI64,
    n: AtomicU64,
    /// 1 = get_file_fd(fd) resolved at census time (fd open), 0 = it
    /// did not (fd closed), 2 = resolved and file is O_WRONLY (read
    /// mode check would EBADF before any file-layer read).
    fd_state: AtomicU8,
    fd: AtomicU64,
    ops: AtomicU64,
    path: [AtomicU8; 96],
}

const EMPTY_RDFAIL: RdFailSlot = RdFailSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    ret: AtomicI64::new(0),
    n: AtomicU64::new(0),
    fd_state: AtomicU8::new(0),
    fd: AtomicU64::new(0),
    ops: AtomicU64::new(0),
    path: [const { AtomicU8::new(0) }; 96],
};

static RD_FAIL: [RdFailSlot; 16] = [const { EMPTY_RDFAIL }; 16];

// ============================================================================
// pa15: F1 irq-exit dispatch quantification — IF=0 time decomposition,
// UART per-byte cli cost, per-comm printk volume, recv-family fd census.
//
// The pa13/pa14 finding: ~26% of kernel ticks land on the page holding
// enable_irq/restore_irq + inb/outb + FileBuffer, but that share is a
// SAMPLING ARTIFACT (queued ticks attribute to the re-enable site), so
// it does not say how much IF=0 time each producer actually costs. The
// launcher line needs real numbers before fixing the irq-exit dispatch
// discipline or making the console async: how much wall time is spent
// with IF=0, split by zone (hardirq frame / softirq dispatch loop /
// UART TX / other irqsave sections), plus the per-byte UART wait
// distribution and who is printing.
//
// Zone model (x86_64 measurement hooks, activated by a scratch patch
// over cpu.rs/preempt.rs/softirq.rs/console.rs/printk.rs — those files
// are other lanes' territory, so the hooks ride the pa14 pattern: build,
// save bzImage, revert; see run/pa15-probe.patch):
//   - cpu.rs enable/disable/restore stamp IF 0->1 / 1->0 transitions.
//   - irq_enter/irq_exit push/pop a HARDIRQ zone (delivery->iret frame;
//     an interrupt gate frame always starts and ends with IF=1, so
//     pop-to-depth-0 closes any open segment).
//   - __do_softirq push/pop SOFTIRQ (nested under HARDIRQ).
//   - putc push/pop UART per byte (nested under whatever is running —
//     printk from softirq, from task context, from the direct-console
//     probe printer).
// Segments are stamped with the RAW TSC (read_counter), not read_time():
// a segment opens and closes on the same CPU, so cross-CPU TSC offsets
// never enter a delta, and rdtsc costs ~one instruction against
// read_time()'s correction loads — the hooks must not inflate the very
// windows they measure. Cumulatives are converted at dump time via
// get_counter_freq().
// ============================================================================

/// IF=0 zones (index into per-CPU CUM rows and the zone stack).
pub const ZONE_REST: u8 = 0;
pub const ZONE_HARDIRQ: u8 = 1;
pub const ZONE_SOFTIRQ: u8 = 2;
pub const ZONE_UART: u8 = 3;

const ZSTK_DEPTH: usize = 4;

const EMPTY_ZROW: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

struct If0Zone {
    /// Our model of IF while the probe hooks are live (false = enabled).
    if0: [AtomicU8; crate::config::MAX_CPUS],
    /// Raw-TSC stamp of the currently open IF=0 segment (valid if if0).
    t0: [AtomicU64; crate::config::MAX_CPUS],
    /// Zone stack (nesting), ZD = current depth.
    zstk: [[AtomicU8; ZSTK_DEPTH]; crate::config::MAX_CPUS],
    zd: [AtomicU8; crate::config::MAX_CPUS],
    /// Cumulative raw TSC per zone per CPU.
    cum: [[AtomicU64; 4]; crate::config::MAX_CPUS],
    /// Number of IF=0 segments closed per zone (window-count shape).
    nseg: [[AtomicU64; 4]; crate::config::MAX_CPUS],
    /// Longest closed segment per zone (raw TSC).
    seg_max: [[AtomicU64; 4]; crate::config::MAX_CPUS],
}

static IFZ: If0Zone = If0Zone {
    if0: [const { AtomicU8::new(0) }; crate::config::MAX_CPUS],
    t0: [const { AtomicU64::new(0) }; crate::config::MAX_CPUS],
    zstk: [const { [const { AtomicU8::new(0) }; ZSTK_DEPTH] }; crate::config::MAX_CPUS],
    zd: [const { AtomicU8::new(0) }; crate::config::MAX_CPUS],
    cum: [const { EMPTY_ZROW }; crate::config::MAX_CPUS],
    nseg: [const { EMPTY_ZROW }; crate::config::MAX_CPUS],
    seg_max: [const { EMPTY_ZROW }; crate::config::MAX_CPUS],
};

/// Current zone (stack top, or REST when empty). Caller-CPU only.
#[inline]
fn zone_cur(cpu: usize) -> u8 {
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    if d == 0 {
        ZONE_REST
    } else {
        IFZ.zstk[cpu][d - 1].load(Ordering::Relaxed)
    }
}

/// Close the currently open segment into `z` (kept separate from
/// `irq_note_on` only for the pop-to-zero path, which must NOT reopen).
#[inline]
fn seg_close(cpu: usize, z: u8, now: u64) {
    let dt = now.saturating_sub(IFZ.t0[cpu].load(Ordering::Relaxed));
    IFZ.cum[cpu][z as usize].fetch_add(dt, Ordering::Relaxed);
    IFZ.nseg[cpu][z as usize].fetch_add(1, Ordering::Relaxed);
    let mut m = IFZ.seg_max[cpu][z as usize].load(Ordering::Relaxed);
    while dt > m {
        match IFZ.seg_max[cpu][z as usize].compare_exchange_weak(
            m,
            dt,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(cur) => m = cur,
        }
    }
}

/// Hook for cpu.rs `disable_irq` / `save_and_disable_irq` (after cli):
/// opens a REST-zone segment if none is open.
#[inline]
pub fn irq_note_off() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) == 0 {
        IFZ.t0[cpu].store(crate::arch::cpu::read_counter(), Ordering::Relaxed);
        IFZ.if0[cpu].store(1, Ordering::Relaxed);
    }
}

/// Hook for cpu.rs `enable_irq` / `restore_irq(true)`: closes the open
/// segment into the current zone.
#[inline]
pub fn irq_note_on() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) != 0 {
        seg_close(cpu, zone_cur(cpu), crate::arch::cpu::read_counter());
        IFZ.if0[cpu].store(0, Ordering::Relaxed);
    }
}

/// Zone push (irq_enter / __do_softirq / putc). If an IF=0 segment is
/// open it is split (credited to the old zone, a new one starts under
/// the incoming zone). `open` is true ONLY for the irq_enter hook:
/// interrupt delivery itself clears IF before any cpu.rs hook can
/// observe it, so that push opens the delivery->iret segment when none
/// is running. Softirq/UART pushes pass false — ksoftirqd dispatches
/// softirqs with IF=1, and those must not manufacture IF=0 time.
#[inline]
pub fn zone_push(z: u8, open: bool) {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    if IFZ.if0[cpu].load(Ordering::Relaxed) != 0 {
        seg_close(cpu, zone_cur(cpu), now);
    } else if open {
        IFZ.t0[cpu].store(now, Ordering::Relaxed);
        IFZ.if0[cpu].store(1, Ordering::Relaxed);
    }
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    if d < ZSTK_DEPTH {
        IFZ.zstk[cpu][d].store(z, Ordering::Relaxed);
        IFZ.zd[cpu].store((d + 1) as u8, Ordering::Relaxed);
    }
}

/// Zone pop (irq_exit / __do_softirq exit / putc exit). At pop-to-zero
/// (irq_exit, just before iret) the segment always closes: an interrupt
/// gate frame was entered with IF=1 and iret restores IF=1, so any IF=0
/// inside the frame ends at the frame boundary.
#[inline]
pub fn zone_pop() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    if d == 0 {
        return;
    }
    let z = IFZ.zstk[cpu][d - 1].load(Ordering::Relaxed);
    IFZ.zd[cpu].store((d - 1) as u8, Ordering::Relaxed);
    if IFZ.if0[cpu].load(Ordering::Relaxed) != 0 {
        seg_close(cpu, z, now);
        if d - 1 == 0 {
            // Frame boundary: IF returns to its delivery-time value (1).
            IFZ.if0[cpu].store(0, Ordering::Relaxed);
        } else {
            // Nested zone ended; the enclosing window continues.
            IFZ.t0[cpu].store(now, Ordering::Relaxed);
        }
    }
}

/// Is the current CPU modeled IF=0 right now (for the putc if0 split)?
#[inline]
pub fn if0_now() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    cpu < crate::config::MAX_CPUS && IFZ.if0[cpu].load(Ordering::Relaxed) != 0
}

// ---------------------------------------------------------------------------
// pa15 UART per-byte cost: every console.rs putc reports its wall time
// (raw TSC, whole call incl. THRE poll + outb), its LSR poll count (one
// inb = one KVM exit), and whether it ran modeled IF=0. Two independent
// accumulator sets (if0 / if1) + log2 histograms give the launcher line
// the per-byte cli-cost distribution for the console-async decision.
// ---------------------------------------------------------------------------

const UART_HIST: usize = 40;

struct UartStat {
    n: AtomicU64,
    cyc: AtomicU64,
    max: AtomicU64,
    polls: AtomicU64,
    polls_gt1: AtomicU64,
    hist: [AtomicU64; UART_HIST],
}

const EMPTY_UART: UartStat = UartStat {
    n: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
    polls: AtomicU64::new(0),
    polls_gt1: AtomicU64::new(0),
    hist: [const { AtomicU64::new(0) }; UART_HIST],
};

static UART_IF0: UartStat = EMPTY_UART;
static UART_IF1: UartStat = EMPTY_UART;

/// Hook for console.rs putc (x86_64): `dt` raw TSC for the whole call,
/// `polls` = LSR reads issued, `if0` = modeled interrupts-off.
#[inline]
pub fn uart_putc_note(dt: u64, polls: u64, if0: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let s = if if0 { &UART_IF0 } else { &UART_IF1 };
    s.n.fetch_add(1, Relaxed);
    s.cyc.fetch_add(dt, Relaxed);
    let mut m = s.max.load(Relaxed);
    while dt > m {
        match s.max.compare_exchange_weak(m, dt, Relaxed, Relaxed) {
            Ok(_) => break,
            Err(cur) => m = cur,
        }
    }
    s.polls.fetch_add(polls, Relaxed);
    if polls > 1 {
        s.polls_gt1.fetch_add(1, Relaxed);
    }
    let b = (64 - dt.leading_zeros() as usize).min(UART_HIST - 1);
    s.hist[b].fetch_add(1, Relaxed);
}

// ---------------------------------------------------------------------------
// pa15 per-comm printk volume: who is printing. One 16-slot table keyed
// by comm (records reaching write_to_ring_buffer = everything that
// passed the loglevel filter, kernel prints AND /dev/kmsg writes; IRQ-
// context records go to a "<irq>" pseudo-slot so scheduler-interrupt
// prints do not smear random interrupted tasks). Console-emitted bytes
// are counted separately, split by modeled IF=0 (console emission is
// the only printk part that costs cli time in task context).
// ---------------------------------------------------------------------------

struct PrSlot {
    comm0: AtomicU64,
    comm: [AtomicU8; 16],
    recs: AtomicU64,
    ring_bytes: AtomicU64,
    con_bytes: AtomicU64,
    con_if0_bytes: AtomicU64,
}

const EMPTY_PR: PrSlot = PrSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    recs: AtomicU64::new(0),
    ring_bytes: AtomicU64::new(0),
    con_bytes: AtomicU64::new(0),
    con_if0_bytes: AtomicU64::new(0),
};

static PRNT: [PrSlot; 16] = [const { EMPTY_PR }; 16];

/// printk() calls dropped by the console-loglevel pre-filter (they cost
/// only the filter check — the number sizes the filter's value).
static PRNT_FILTERED: AtomicU64 = AtomicU64::new(0);

#[inline]
fn pr_slot() -> Option<&'static PrSlot> {
    use core::sync::atomic::Ordering::Relaxed;
    let irq_ctx = crate::interrupt::preempt::in_interrupt();
    let comm: [u8; 16] = if irq_ctx {
        *b"<irq>\0\0\0\0\0\0\0\0\0\0\0"
    } else {
        match crate::sched::current() {
            Some(t) => unsafe { *(*t).comm() },
            None => *b"<nointask>\0\0\0\0\0\0",
        }
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    for i in 0..PRNT.len() {
        if PRNT[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        if (0..16).all(|j| PRNT[i].comm[j].load(Relaxed) == comm[j]) {
            return Some(&PRNT[i]);
        }
    }
    for i in 0..PRNT.len() {
        if PRNT[i].comm0.load(Relaxed) != 0 {
            continue;
        }
        for j in 0..16 {
            PRNT[i].comm[j].store(comm[j], Relaxed);
        }
        PRNT[i].comm0.store(comm0, Relaxed);
        return Some(&PRNT[i]);
    }
    None
}

/// Hook for printk() early return when level > console_loglevel.
#[inline]
pub fn note_printk_filtered() {
    if on() {
        PRNT_FILTERED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Hook at write_to_ring_buffer entry (every post-filter record).
#[inline]
pub fn note_printk_ring(len: usize) {
    if !on() {
        return;
    }
    if let Some(s) = pr_slot() {
        s.recs.fetch_add(1, Ordering::Relaxed);
        s.ring_bytes.fetch_add(len as u64, Ordering::Relaxed);
    }
}

/// Hook at emit_to_console tail (console-emitted bytes; `len` counts the
/// bytes actually pushed, CR-for-LF expansion included by the caller).
#[inline]
pub fn note_printk_con(len: usize) {
    if !on() {
        return;
    }
    let if0 = if0_now();
    if let Some(s) = pr_slot() {
        s.con_bytes.fetch_add(len as u64, Ordering::Relaxed);
        if if0 {
            s.con_if0_bytes.fetch_add(len as u64, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// pa15 recv-family fd census (hilogd.server dual-drain evidence for the
// launcher line's ipc work — measurement only, no semantics change).
// Keyed by (comm, nr); the slot FOLLOWS the current fd: on fd/file_id
// change it retargets and resets the per-fd shape counters (RD_DATA
// pattern). Shape per current fd: data reads + bytes, EAGAINs, zeros,
// first-seen errno; plus flags/O_NONBLOCK/ops/desc resolved at retarget
// (unix sockets: kind + bound name + peer; symbolize ops host-side).
// ---------------------------------------------------------------------------

struct RxSlot {
    comm0: AtomicU64,
    comm: [AtomicU8; 16],
    nr: AtomicU64,
    fd: AtomicU64,
    file_id: AtomicU64,
    flags: AtomicI64,
    nonblock: AtomicU8,
    n_total: AtomicU64,
    n_cur: AtomicU64,
    n_data: AtomicU64,
    data_bytes: AtomicU64,
    n_eagain: AtomicU64,
    n_zero: AtomicU64,
    err: AtomicI64,
    n_err: AtomicU64,
    ops: AtomicU64,
    desc: [AtomicU8; 96],
}

const EMPTY_RX: RxSlot = RxSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    nr: AtomicU64::new(0),
    fd: AtomicU64::new(0),
    file_id: AtomicU64::new(0),
    flags: AtomicI64::new(0),
    nonblock: AtomicU8::new(0),
    n_total: AtomicU64::new(0),
    n_cur: AtomicU64::new(0),
    n_data: AtomicU64::new(0),
    data_bytes: AtomicU64::new(0),
    n_eagain: AtomicU64::new(0),
    n_zero: AtomicU64::new(0),
    err: AtomicI64::new(0),
    n_err: AtomicU64::new(0),
    ops: AtomicU64::new(0),
    desc: [const { AtomicU8::new(0) }; 96],
};

static RECV: [RxSlot; 16] = [const { EMPTY_RX }; 16];

/// Describe a recv-family fd for the census table (resolved once per
/// retarget, never on the hot path). Unix sockets get kind/bound/peer
/// (peer pid via SO_PEERCRED semantics for streams, the connected dgram
/// destination for dgrams); everything else records the ops-table
/// pointer for host-side nm symbolization.
fn recv_describe(fd: usize, out: &[AtomicU8; 96]) {
    let mut s = alloc::string::String::new();
    if let Some((usock, nonblock)) = crate::net::unix::unix_file_of(fd) {
        let kind = match usock.kind {
            crate::net::unix::UnixKind::Stream => "stream",
            _ => "dgram",
        };
        let bound = usock.bound_name.lock().clone().unwrap_or_default();
        let peer_name = match usock.peer_cred() {
            Some(c) => alloc::format!("pid {}", c.pid),
            None => usock
                .dgram_peer
                .lock()
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        };
        s = alloc::format!(
            "unix:{} bound=[{}] peer=[{}]{}",
            kind,
            bound,
            peer_name,
            if nonblock { " O_NONBLOCK" } else { "" }
        );
    } else if let Some(file) = unsafe { crate::fs::get_file_fd(fd) } {
        let ops = unsafe { (*file.ops.get()).map(|p| p as *const _ as u64).unwrap_or(0) };
        let path = unsafe {
            match &*file.dentry.get() {
                Some(dentry) => crate::fs::vfs::dentry_user_path(dentry),
                None => alloc::string::String::from("<no-dentry>"),
            }
        };
        s = alloc::format!("ops={:#x} path={}", ops, path);
    } else {
        s = alloc::format!("<closed fd {}>", fd);
    }
    for (k, b) in s.as_bytes().iter().take(96).enumerate() {
        out[k].store(*b, Ordering::Relaxed);
    }
}

/// pa15 recv census entry: called from the syscall wrapper for native
/// nr 47 (recvmsg) / 45 (recvfrom) with the caller's fd, flags and the
/// syscall's return value. Zero cost when syscost is off.
pub fn recv_census(nr: usize, fd: usize, flags: i32, ret: i64) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::current() {
        Some(t) => unsafe { *(*t).comm() },
        None => return,
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    let mut slot: Option<&RxSlot> = None;
    for i in 0..RECV.len() {
        if RECV[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        if (0..16).all(|j| RECV[i].comm[j].load(Relaxed) == comm[j])
            && RECV[i].nr.load(Relaxed) == nr as u64
        {
            slot = Some(&RECV[i]);
            break;
        }
    }
    if slot.is_none() {
        for i in 0..RECV.len() {
            if RECV[i].comm0.load(Relaxed) != 0 {
                continue;
            }
            for j in 0..16 {
                RECV[i].comm[j].store(comm[j], Relaxed);
            }
            RECV[i].comm0.store(comm0, Relaxed);
            RECV[i].nr.store(nr as u64, Relaxed);
            slot = Some(&RECV[i]);
            break;
        }
    }
    let s = match slot {
        Some(s) => s,
        None => return,
    };
    s.n_total.fetch_add(1, Relaxed);
    // Retarget on fd change (follow the current description, RD_DATA
    // pattern; the shape counters restart so the dump shows ONE fd's
    // loop shape at a time).
    let cur_file = unsafe { crate::fs::get_file_fd(fd) };
    let fid = cur_file.as_ref().map(|f| f.file_id).unwrap_or(0);
    if s.file_id.load(Relaxed) != fid || s.fd.load(Relaxed) != fd as u64 {
        s.file_id.store(fid, Relaxed);
        s.fd.store(fd as u64, Relaxed);
        s.flags.store(flags as i64, Relaxed);
        let nb = cur_file
            .as_ref()
            .map(|f| {
                (f.flags.load(Relaxed)
                    & crate::fs::file::FileFlags::O_NONBLOCK)
                    != 0
            })
            .unwrap_or(false);
        s.nonblock.store(if nb { 1 } else { 0 }, Relaxed);
        s.ops.store(
            cur_file
                .as_ref()
                .and_then(|f| unsafe { (*f.ops.get()).map(|p| p as *const _ as u64) })
                .unwrap_or(0),
            Relaxed,
        );
        for j in 0..96 {
            s.desc[j].store(0, Relaxed);
        }
        recv_describe(fd, &s.desc);
        s.n_cur.store(0, Relaxed);
        s.n_data.store(0, Relaxed);
        s.data_bytes.store(0, Relaxed);
        s.n_eagain.store(0, Relaxed);
        s.n_zero.store(0, Relaxed);
        s.n_err.store(0, Relaxed);
        s.err.store(0, Relaxed);
    }
    s.n_cur.fetch_add(1, Relaxed);
    if ret > 0 {
        s.n_data.fetch_add(1, Relaxed);
        s.data_bytes.fetch_add(ret as u64, Relaxed);
    } else if ret == 0 {
        s.n_zero.fetch_add(1, Relaxed);
    } else if ret == -(crate::errno::constants::EAGAIN as i64) {
        s.n_eagain.fetch_add(1, Relaxed);
    } else {
        if s.n_err.load(Relaxed) == 0 {
            s.err.store(ret, Relaxed);
        }
        s.n_err.fetch_add(1, Relaxed);
    }
}

/// pa10 failing-read census: call from the read wrapper when ret < 0
/// and ret != -EAGAIN. Records the errno, whether the fd still
/// resolves, and the path. Zero cost when syscost is off.
pub fn note_read_fail(fd: usize, ret: i64) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::current() {
        Some(t) => unsafe { *(*t).comm() },
        None => return,
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    // First-seen errno per comm is the story (a spin loops one errno).
    for i in 0..RD_FAIL.len() {
        if RD_FAIL[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        let same = (0..16).all(|j| RD_FAIL[i].comm[j].load(Relaxed) == comm[j]);
        if same {
            RD_FAIL[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
    for i in 0..RD_FAIL.len() {
        if RD_FAIL[i].comm0.load(Relaxed) != 0 {
            continue;
        }
        for j in 0..16 {
            RD_FAIL[i].comm[j].store(comm[j], Relaxed);
        }
        RD_FAIL[i].comm0.store(comm0, Relaxed);
        RD_FAIL[i].ret.store(ret, Relaxed);
        RD_FAIL[i].fd.store(fd as u64, Relaxed);
        RD_FAIL[i].n.store(1, Relaxed);
        match unsafe { crate::fs::get_file_fd(fd) } {
            Some(file) => {
                let mode = file.flags.load(Relaxed) & crate::fs::file::FileFlags::O_ACCMODE;
                RD_FAIL[i].fd_state.store(
                    if mode == crate::fs::file::FileFlags::O_WRONLY { 2 } else { 1 },
                    Relaxed,
                );
                RD_FAIL[i].ops.store(
                    unsafe { (*file.ops.get()).map(|p| p as *const _ as u64).unwrap_or(0) },
                    Relaxed,
                );
                let path = unsafe {
                    match &*file.dentry.get() {
                        Some(dentry) => crate::fs::vfs::dentry_user_path(dentry),
                        None => alloc::string::String::from("<no-dentry>"),
                    }
                };
                let mut k = 0;
                for &b in path.as_bytes() {
                    if k >= 96 {
                        break;
                    }
                    RD_FAIL[i].path[k].store(b, Relaxed);
                    k += 1;
                }
            }
            None => {
                RD_FAIL[i].fd_state.store(0, Relaxed);
            }
        }
        return;
    }
}

/// pa9 positive-read census: call on every read() that returned data.
/// See `RdDataSlot` for the design; zero cost when syscost is off.
pub fn note_read_data(file: &crate::fs::File, n: usize, fd: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => unsafe { *(*t).comm() },
        None => return,
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    let mut slot: Option<&RdDataSlot> = None;
    for i in 0..RD_DATA.len() {
        if RD_DATA[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        let same = (0..16).all(|j| RD_DATA[i].comm[j].load(Relaxed) == comm[j]);
        if same {
            slot = Some(&RD_DATA[i]);
            break;
        }
    }
    if slot.is_none() {
        for i in 0..RD_DATA.len() {
            if RD_DATA[i].comm0.load(Relaxed) != 0 {
                continue;
            }
            for j in 0..16 {
                RD_DATA[i].comm[j].store(comm[j], Relaxed);
            }
            RD_DATA[i].comm0.store(comm0, Relaxed);
            slot = Some(&RD_DATA[i]);
            break;
        }
    }
    let s = match slot {
        Some(s) => s,
        // Table full of foreign comms — this comm goes untracked.
        None => return,
    };
    s.n_total.fetch_add(1, Relaxed);
    if s.file_id.load(Relaxed) == file.file_id {
        s.n_cur.fetch_add(1, Relaxed);
        s.bytes.fetch_add(n as u64, Relaxed);
        return;
    }
    // Retarget: follow this description from now on.
    s.file_id.store(file.file_id, Relaxed);
    s.fd.store(fd as u64, Relaxed);
    s.ops.store(
        unsafe { (*file.ops.get()).map(|p| p as *const _ as u64).unwrap_or(0) },
        Relaxed,
    );
    let path = unsafe {
        match &*file.dentry.get() {
            Some(dentry) => crate::fs::vfs::dentry_user_path(dentry),
            None => alloc::string::String::from("<no-dentry>"),
        }
    };
    for j in 0..96 {
        s.path[j].store(0, Relaxed);
    }
    let mut k = 0;
    for &b in path.as_bytes() {
        if k >= 96 {
            break;
        }
        s.path[k].store(b, Relaxed);
        k += 1;
    }
    s.n_cur.store(1, Relaxed);
    s.bytes.store(n as u64, Relaxed);
}


#[inline]
pub fn on() -> bool {
    match ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            // pa15: do NOT cache a verdict before the cmdline exists.
            // The first caller used to be the first syscall (long after
            // cmdline::init); the pa15 IF=0 zone hooks fire from the very
            // first boot-time irqsave spinlock — mm/heap init, well before
            // cmdline::init_from — and a cached "off" from that call kept
            // syscost dead for the whole boot (pa15smoke: zero dumps).
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => {
                    if crate::cmdline::get_param("syscost").as_deref() == Some("1") {
                        2
                    } else {
                        1
                    }
                }
            };
            ARMED.store(v, Ordering::Relaxed);
            v == 2
        }
    }
}


/// pa4 EOF-spin census: (comm, path) of readers whose fd returned EOF,
/// once per comm (a spin candidate is a reader looping read()==0; pa3
/// proved hilogd.rd_kmsg's 92K/s never reaches /dev/kmsg's counters).
pub fn note_read_eof(file: &crate::fs::File) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => unsafe { *(*t).comm() },
        None => return,
    };
    // Keyed by COMM ONLY (pa7 lesson: init alone opened 8 different
    // EOF files and starved the 12 slots before the interesting
    // spinners started). First EOF path per comm is recorded.
    for i in 0..EOF_SPIN.len() {
        let same = (0..16).all(|j| EOF_SPIN[i].comm[j].load(Relaxed) == comm[j]);
        if same {
            EOF_SPIN[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
    for i in 0..EOF_SPIN.len() {
        let mut empty = true;
        for j in 0..16 {
            if EOF_SPIN[i].comm[j].load(Relaxed) != 0 {
                empty = false;
                break;
            }
        }
        if empty {
            for j in 0..16 {
                EOF_SPIN[i].comm[j].store(comm[j], Relaxed);
            }
            // Reader-relative path (same shape as /proc/<pid>/fd/N).
            let path = unsafe {
                match &*file.dentry.get() {
                    Some(dentry) => crate::fs::vfs::dentry_user_path(dentry),
                    None => alloc::string::String::from("<no-dentry>"),
                }
            };
            let mut k = 0;
            for &b in path.as_bytes() {
                if k >= 96 {
                    break;
                }
                EOF_SPIN[i].path[k].store(b, Relaxed);
                k += 1;
            }
            EOF_SPIN[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
}


/// pa5 EAGAIN-spin census: (comm, path) of nonblocking-fd reads that
/// returned EAGAIN, once per comm — the fast-path twin of
/// note_read_eof (pipe/sockets never reach the generic EOF census).
pub fn note_read_eagain(file: &crate::fs::File) {
    use core::sync::atomic::Ordering::Relaxed;
    let comm: [u8; 16] = match crate::sched::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => unsafe { *(*t).comm() },
        None => return,
    };
    for i in 0..EAGAIN_SPIN.len() {
        let mut empty = true;
        for j in 0..16 {
            if EAGAIN_SPIN[i].comm[j].load(Relaxed) != 0 {
                empty = false;
                break;
            }
        }
        if empty {
            for j in 0..16 {
                EAGAIN_SPIN[i].comm[j].store(comm[j], Relaxed);
            }
            let path = unsafe {
                match &*file.dentry.get() {
                    Some(dentry) => crate::fs::vfs::dentry_user_path(dentry),
                    None => alloc::string::String::from("<no-dentry>"),
                }
            };
            let mut k = 0;
            for &b in path.as_bytes() {
                if k >= 96 {
                    break;
                }
                EAGAIN_SPIN[i].path[k].store(b, Relaxed);
                k += 1;
            }
            EAGAIN_SPIN[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
}

/// Syscall entry stamp. `None` when the probe is off. Stamps are in the
/// NOMINAL time domain (`read_time()`, 10 ticks/us), NOT raw RDTSC:
/// under KVM each vCPU carries its own TSC offset (the sc100/sc114
/// lesson — offsets differ, rates match), so a raw delta across a CPU
/// migration is garbage (pa1 first attempt: nanosleep "avg 1.5 s");
/// `read_time()` applies the per-CPU correction and the monotonic
/// ratchet, at ~2 atomic loads of overhead.
#[inline]
pub fn enter() -> Option<u64> {
    if on() {
        Some(crate::drivers::timer::read_time())
    } else {
        None
    }
}

/// Flood-nr per-comm attribution: which process drives the 10^5/s calls.
/// Only the known flood nrs are tracked (16 comm slots each), so the
/// hot path pays one array scan only for those nrs.
const FLOOD_NRS: [usize; 6] = [0 /*read*/, 228 /*clock_gettime*/, 47 /*recvmsg*/, 45 /*recvfrom*/, 20 /*writev*/, 202 /*futex*/];


struct CommCount {
    comm: [AtomicU8; 16],
    n: AtomicU64,
}

const EMPTY_CC: CommCount = CommCount {
    comm: [const { AtomicU8::new(0) }; 16],
    n: AtomicU64::new(0),
};

static FLOOD: [[CommCount; 16]; 6] = [const { [const { EMPTY_CC }; 16] }; 6];

#[inline]
fn flood_account(nr: usize) {
    let f = match FLOOD_NRS.iter().position(|&x| x == nr) {
        Some(i) => i,
        None => return,
    };
    let comm: &[u8; 16] = match crate::sched::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => unsafe { (*t).comm() },
        None => return,
    };
    let tab = &FLOOD[f];
    for i in 0..16 {
        let mut same = true;
        let mut empty = true;
        for j in 0..16 {
            let c = tab[i].comm[j].load(Ordering::Relaxed);
            if c != 0 {
                empty = false;
            }
            if c != comm[j] {
                same = false;
            }
        }
        if same {
            tab[i].n.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if empty {
            // Claim the slot (comm first, count follows).
            for j in 0..16 {
                tab[i].comm[j].store(comm[j], Ordering::Relaxed);
            }
            tab[i].n.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
}

/// Syscall exit accounting (wall cycles include blocking by design).
/// `dt` in nominal ticks (100 ns each).
#[inline]
pub fn exit(nr: usize, t0: Option<u64>) {
    let t0 = match (t0, on()) {
        (Some(t), true) => t,
        _ => return,
    };
    if nr >= NR_SLOTS {
        return;
    }
    flood_account(nr);
    let dt = crate::drivers::timer::read_time().saturating_sub(t0);
    let e = &TABLE[nr];
    e.count.fetch_add(1, Ordering::Relaxed);
    e.cyc.fetch_add(dt, Ordering::Relaxed);
    // Monotonic-ish max: CAS grow; a lost race keeps the older max.
    let mut m = e.max.load(Ordering::Relaxed);
    while dt > m {
        match e
            .max
            .compare_exchange_weak(m, dt, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(cur) => m = cur,
        }
    }
    if dt > SLOW_CYC {
        let i = SLOW_IDX.fetch_add(1, Ordering::Relaxed) % SLOW_RING;
        SLOW[i].nr.store(nr as u64, Ordering::Relaxed);
        SLOW[i].cyc.store(dt, Ordering::Relaxed);
        if let Some(t) = crate::sched::current() {
            // SAFETY: current task; comm is a fixed-size field.
            let c = unsafe { (*t).comm() };
            for j in 0..16 {
                SLOW[i].comm[j].store(c[j], Ordering::Relaxed);
            }
        }
    }
}

/// Tick-mode accounting hook — call once per timer tick that landed in
/// a context with a user address space (`user_mode` per the trap
/// frame), plus the idle variant for ticks without one.
#[inline]
pub fn tick_user() {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_USER[c].fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub fn tick_kern(rip: u64) {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_KERN[c].fetch_add(1, Ordering::Relaxed);
    }
    rip_sample(rip);
}

#[inline]
pub fn tick_idle() {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_IDLE[c].fetch_add(1, Ordering::Relaxed);
    }
}

/// Kernel-text page sample (mirrors sc105 STIME_HIST, comm-free so the
/// whole boot crowd shares one table).
#[inline]
fn rip_sample(rip: u64) {
    let (lo, hi) = crate::sched::sched::kernel_text_window();
    if rip < lo || rip >= hi {
        return;
    }
    let page = rip & !0xFFFu64;
    // Fast path: find an existing slot.
    let mut min_i = 0usize;
    let mut min_c = u64::MAX;
    for i in 0..RIP_SLOTS {
        let p = RIP_HIST[i].page.load(Ordering::Relaxed);
        if p == page {
            RIP_HIST[i].count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if p == 0 {
            // Claim empty slot.
            if RIP_HIST[i]
                .page
                .compare_exchange(0, page, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                RIP_HIST[i].count.store(1, Ordering::Relaxed);
                return;
            }
            continue;
        }
        let c = RIP_HIST[i].count.load(Ordering::Relaxed);
        if c < min_c {
            min_c = c;
            min_i = i;
        }
    }
    // Steal the least-sampled slot.
    if RIP_HIST[min_i]
        .count
        .compare_exchange_weak(min_c, 1, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        RIP_HIST[min_i].page.store(page, Ordering::Relaxed);
    }
}

/// Periodic dump from the timer softirq. Cheap when off.
pub fn tick(now_jiffies: u64) {
    if !on() {
        return;
    }
    if now_jiffies.saturating_sub(LAST_DUMP.load(Ordering::Relaxed))
        < SYSCOST_DUMP_SECS * crate::config::KERNEL_HZ as u64
    {
        return;
    }
    LAST_DUMP.store(now_jiffies, Ordering::Relaxed);
    dump();
}

/// Print the full report (also usable from a debug trigger).
pub fn dump() {
    // Nominal domain: 10 ticks per microsecond by definition.
    const US: u64 = 10;
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;

    // Tick modes: one line per dump.
    let mut line = alloc::string::String::from("SYSCOST t=");
    line.push_str(&secs.to_string());
    line.push_str("s ticks(u/k/i):");
    for c in 0..crate::config::MAX_CPUS {
        let u = TICK_USER[c].load(Ordering::Relaxed);
        let k = TICK_KERN[c].load(Ordering::Relaxed);
        let i = TICK_IDLE[c].load(Ordering::Relaxed);
        if u | k | i == 0 {
            continue;
        }
        line.push_str(&alloc::format!(" c{}={}/{}/{}", c, u, k, i));
    }
    crate::pr_warn!("{}", line);

    // Softirq vector dispatch counts (the existing WW_VEC_RUN / run /
    // busy-tick counters, printed cumulatively): names the vector behind
    // the __do_softirq page burn seen in the rip histogram.
    {
        use crate::interrupt::softirq::{WW_SOFTIRQ_RUNS, WW_SOFTIRQ_TICKS, WW_VEC_RUN};
        let mut line = alloc::string::String::new();
        for v in 0..crate::interrupt::softirq::NR_SOFTIRQS {
            let n = WW_VEC_RUN[v].load(Ordering::Relaxed) as u64;
            if n > 0 {
                line.push_str(&alloc::format!(" v{}={}", v, n));
            }
        }
        let busy_ms = WW_SOFTIRQ_TICKS.load(Ordering::Relaxed)
            / (crate::config::TIMER_CLOCK_FREQ_HZ / 1000);
        crate::pr_warn!(
            "SYSCOST t={}s softirq: runs={} busy={}ms vecs:{}",
            secs,
            WW_SOFTIRQ_RUNS.load(Ordering::Relaxed),
            busy_ms,
            line
        );
    }

    // Flood-nr per-comm attribution (which process drives the 10^5/s
    // calls; v0=Hi v1=Timer v2=NetTx v3=NetRx v4=Block v5=IrqPoll
    // v6=Tasklet v7=Sched v8=Hrtimer v9=Rcu for the vec line above).
    {
        use crate::printk::{KMSG_N_EAGAIN, KMSG_N_RECORD, KMSG_N_SLEEP, KMSG_FIRST_FLAGS};
        crate::pr_warn!(
            "SYSCOST t={}s kmsg: rec={} eagain={} sleep={} first_flags={:#x}",
            secs,
            KMSG_N_RECORD.load(Ordering::Relaxed),
            KMSG_N_EAGAIN.load(Ordering::Relaxed),
            KMSG_N_SLEEP.load(Ordering::Relaxed),
            KMSG_FIRST_FLAGS.load(Ordering::Relaxed)
        );
    }
    for f in 0..FLOOD_NRS.len() {
        let mut line = alloc::string::String::new();
        for i in 0..16 {
            let n = FLOOD[f][i].n.load(Ordering::Relaxed);
            if n == 0 {
                continue;
            }
            let mut comm = [0u8; 16];
            for j in 0..16 {
                comm[j] = FLOOD[f][i].comm[j].load(Ordering::Relaxed);
            }
            let name = core::str::from_utf8(&comm)
                .unwrap_or("?")
                .split('\0')
                .next()
                .unwrap_or("?");
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&alloc::format!("{}={}", name, n));
        }
        if !line.is_empty() {
            crate::pr_warn!(
                "SYSCOST t={}s flood nr={}: {}",
                secs,
                FLOOD_NRS[f],
                line
            );
        }
    }

    // pa15 IF=0 zone decomposition (F1 irq-exit dispatch numbers; raw
    // TSC -> ms). Zones: rest / hardirq-frame / softirq-dispatch / uart.
    {
        let tsc_hz = crate::arch::cpu::get_counter_freq().max(1);
        let msd = tsc_hz / 1000;
        for c in 0..crate::config::MAX_CPUS {
            let v = [
                IFZ.cum[c][0].load(Ordering::Relaxed),
                IFZ.cum[c][1].load(Ordering::Relaxed),
                IFZ.cum[c][2].load(Ordering::Relaxed),
                IFZ.cum[c][3].load(Ordering::Relaxed),
            ];
            if v.iter().all(|&x| x == 0) {
                continue;
            }
            let tot: u64 = v.iter().sum();
            let nseg = [
                IFZ.nseg[c][0].load(Ordering::Relaxed),
                IFZ.nseg[c][1].load(Ordering::Relaxed),
                IFZ.nseg[c][2].load(Ordering::Relaxed),
                IFZ.nseg[c][3].load(Ordering::Relaxed),
            ];
            let mx = [
                IFZ.seg_max[c][0].load(Ordering::Relaxed),
                IFZ.seg_max[c][1].load(Ordering::Relaxed),
                IFZ.seg_max[c][2].load(Ordering::Relaxed),
                IFZ.seg_max[c][3].load(Ordering::Relaxed),
            ];
            crate::pr_warn!(
                "SYSCOST t={}s if0 cpu={}: tot={}ms rest={}ms hard={}ms soft={}ms uart={}ms nseg={}/{}/{}/{} max={}ms/{}ms/{}ms/{}ms",
                secs,
                c,
                tot / msd,
                v[0] / msd,
                v[1] / msd,
                v[2] / msd,
                v[3] / msd,
                nseg[0], nseg[1], nseg[2], nseg[3],
                mx[0] / msd, mx[1] / msd, mx[2] / msd, mx[3] / msd
            );
        }
    }

    // pa15 UART per-byte cost distribution (console-async decision
    // input). Buckets are log2 of raw TSC cycles per putc call.
    {
        let tsc_hz = crate::arch::cpu::get_counter_freq().max(1);
        let msd = tsc_hz / 1000;
        for (tag, s) in [("if0", &UART_IF0), ("if1", &UART_IF1)] {
            let n = s.n.load(Ordering::Relaxed);
            if n == 0 {
                continue;
            }
            crate::pr_warn!(
                "SYSCOST t={}s uart {} : n={} cyc={}ms avg={}ns max={}ms polls={} multi={} ({:.0}%)",
                secs,
                tag,
                n,
                s.cyc.load(Ordering::Relaxed) / msd,
                // avg ns/byte: divide first (cyc * 1e9 overflows u64 once
                // the table passes ~7 s of accumulated TSC).
                s.cyc.load(Ordering::Relaxed) / n.max(1) * 1000
                    / (tsc_hz / 1_000_000).max(1),
                s.max.load(Ordering::Relaxed) / msd,
                s.polls.load(Ordering::Relaxed),
                s.polls_gt1.load(Ordering::Relaxed),
                s.polls_gt1.load(Ordering::Relaxed) as u64 * 100 / n
            );
            let mut line = alloc::string::String::new();
            for b in 0..UART_HIST {
                let c = s.hist[b].load(Ordering::Relaxed);
                if c == 0 {
                    continue;
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&alloc::format!("2^{}={}", b, c));
            }
            if !line.is_empty() {
                crate::pr_warn!("SYSCOST t={}s uart-hist {}: {}", secs, tag, line);
            }
        }
    }

    // pa15 per-comm printk volume (console producers; "<irq>" collects
    // interrupt-context prints so they do not smear interrupted tasks).
    {
        crate::pr_warn!(
            "SYSCOST t={}s printk: filtered_by_loglevel={}",
            secs,
            PRNT_FILTERED.load(Ordering::Relaxed)
        );
        for i in 0..PRNT.len() {
            let recs = PRNT[i].recs.load(Ordering::Relaxed);
            if recs == 0 {
                continue;
            }
            let mut comm = [0u8; 16];
            for j in 0..16 {
                comm[j] = PRNT[i].comm[j].load(Ordering::Relaxed);
            }
            crate::pr_warn!(
                "SYSCOST t={}s printk comm={} recs={} ringB={} conB={} con_if0B={}",
                secs,
                core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
                recs,
                PRNT[i].ring_bytes.load(Ordering::Relaxed),
                PRNT[i].con_bytes.load(Ordering::Relaxed),
                PRNT[i].con_if0_bytes.load(Ordering::Relaxed)
            );
        }
    }

    // pa15 recv-family fd census (hilogd.server dual-drain evidence).
    for i in 0..RECV.len() {
        let n_total = RECV[i].n_total.load(Ordering::Relaxed);
        if n_total == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = RECV[i].comm[j].load(Ordering::Relaxed);
        }
        let mut desc = [0u8; 96];
        let mut dlen = 0;
        for j in 0..96 {
            desc[j] = RECV[i].desc[j].load(Ordering::Relaxed);
            if desc[j] == 0 {
                dlen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s recv comm={} nr={} tot={} fd={} cur={} data={}({}B) eagain={} zero={} err={}({}n) nb={} flags={:#x} ops={:#x} desc={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            RECV[i].nr.load(Ordering::Relaxed),
            n_total,
            RECV[i].fd.load(Ordering::Relaxed),
            RECV[i].n_cur.load(Ordering::Relaxed),
            RECV[i].n_data.load(Ordering::Relaxed),
            RECV[i].data_bytes.load(Ordering::Relaxed),
            RECV[i].n_eagain.load(Ordering::Relaxed),
            RECV[i].n_zero.load(Ordering::Relaxed),
            RECV[i].err.load(Ordering::Relaxed),
            RECV[i].n_err.load(Ordering::Relaxed),
            RECV[i].nonblock.load(Ordering::Relaxed),
            RECV[i].flags.load(Ordering::Relaxed),
            RECV[i].ops.load(Ordering::Relaxed),
            core::str::from_utf8(&desc[..dlen]).unwrap_or("?")
        );
    }

    // Syscall table: collect (nr, count, cyc, max), sort by cyc desc.
    let mut rows: alloc::vec::Vec<(usize, u64, u64, u64)> = alloc::vec::Vec::new();
    for i in 0..NR_SLOTS {
        let n = TABLE[i].count.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        rows.push((
            i,
            n,
            TABLE[i].cyc.load(Ordering::Relaxed),
            TABLE[i].max.load(Ordering::Relaxed),
        ));
    }
    rows.sort_by(|a, b| b.2.cmp(&a.2));
    let total_cyc: u64 = rows.iter().map(|r| r.2).sum();
    crate::pr_warn!(
        "SYSCOST t={}s syscalls: {} kinds {} calls tot={} ms",
        secs,
        rows.len(),
        rows.iter().map(|r| r.1).sum::<u64>(),
        total_cyc / US / 1000
    );
    for r in rows.iter().take(48) {
        crate::pr_warn!(
            "SYSCOST t={}s nr={} n={} avg={}us max={}ms tot={}ms",
            secs,
            r.0,
            r.1,
            r.2 / US / r.1.max(1), // us
            r.3 / US / 1000,
            r.2 / US / 1000
        );
    }

    for i in 0..EAGAIN_SPIN.len() {
        let n = EAGAIN_SPIN[i].n.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = EAGAIN_SPIN[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = EAGAIN_SPIN[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s eagain comm={} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // pa4 EOF-spin census table.
    for i in 0..EOF_SPIN.len() {
        let n = EOF_SPIN[i].n.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = EOF_SPIN[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = EOF_SPIN[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s eofspin comm={} n={} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            n,
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // pa9 positive-read census table (which fd returns the data).
    for i in 0..RD_DATA.len() {
        let n_total = RD_DATA[i].n_total.load(Ordering::Relaxed);
        if n_total == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = RD_DATA[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = RD_DATA[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s rd_data comm={} n_cur={} n_tot={} bytes={} fd={} file_id={} ops={:#x} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            RD_DATA[i].n_cur.load(Ordering::Relaxed),
            n_total,
            RD_DATA[i].bytes.load(Ordering::Relaxed),
            RD_DATA[i].fd.load(Ordering::Relaxed),
            RD_DATA[i].file_id.load(Ordering::Relaxed),
            RD_DATA[i].ops.load(Ordering::Relaxed),
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // pa10 failing-read census table (errno + fd resolution state).
    for i in 0..RD_FAIL.len() {
        let n = RD_FAIL[i].n.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = RD_FAIL[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = RD_FAIL[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s rd_fail comm={} ret={} n={} fd={} fd_state={} ops={:#x} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            RD_FAIL[i].ret.load(Ordering::Relaxed),
            n,
            RD_FAIL[i].fd.load(Ordering::Relaxed),
            RD_FAIL[i].fd_state.load(Ordering::Relaxed),
            RD_FAIL[i].ops.load(Ordering::Relaxed),
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // Slow ring (last SLOW_RING outliers, ring order).
    crate::pr_warn!("SYSCOST t={}s slow-ring (>4ms wall):", secs);
    let base = SLOW_IDX.load(Ordering::Relaxed);
    for k in 0..SLOW_RING {
        let i = (base + SLOW_RING - 1 - k) % SLOW_RING; // newest first
        let cyc = SLOW[i].cyc.load(Ordering::Relaxed);
        if cyc == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = SLOW[i].comm[j].load(Ordering::Relaxed);
        }
        let name = core::str::from_utf8(&comm)
            .unwrap_or("?")
            .split('\0')
            .next()
            .unwrap_or("?");
        crate::pr_warn!(
            "SYSCOST t={}s slow nr={} cyc={}us comm={}",
            secs,
            SLOW[i].nr.load(Ordering::Relaxed),
            cyc / US,
            name
        );
    }

    // Kernel-rip page histogram: top 32 by count.
    let mut pages: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    for i in 0..RIP_SLOTS {
        let p = RIP_HIST[i].page.load(Ordering::Relaxed);
        if p == 0 {
            continue;
        }
        pages.push((p, RIP_HIST[i].count.load(Ordering::Relaxed)));
    }
    pages.sort_by(|a, b| b.1.cmp(&a.1));
    let tot: u64 = pages.iter().map(|p| p.1).sum();
    crate::pr_warn!(
        "SYSCOST t={}s rip-pages: {} pages {} kern-ticks",
        secs,
        pages.len(),
        tot
    );
    for p in pages.iter().take(32) {
        crate::pr_warn!(
            "SYSCOST t={}s rip page={:#x} n={} ({}%)",
            secs,
            p.0,
            p.1,
            p.1 * 100 / tot.max(1)
        );
    }
}
