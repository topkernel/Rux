//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! preempt_count implementation
//!
//! Bit layout:
//!   bits [0:7]   PREEMPT_MASK   — preemption disable count
//!   bits [8:15]  SOFTIRQ_MASK   — softirq nesting count
//!   bits [16:19] HARDIRQ_MASK   — hard IRQ nesting count
//!   bit  [20]    NMI_MASK       — NMI count
//!   bit  [26]    PREEMPT_ACTIVE — actively preempting

// NOTE (kernel TCB ledger): the forbid was pinned by the sweep, but
// the R-SC-TLB merge added the IRQ-PAIR-SPLIT diagnostic with legitimate
// SAFETY-commented raw derefs of just-pinned Task pointers; the deny is
// lifted until those reads move behind a framework primitive.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust. ENFORCED AS deny, not forbid, with
// exactly one explicit promotion (below) — the s14 IRQ-PAIR report
// reads stable Task fields through raw pointers and cannot be expressed
// safely; the forbid form 00521544 added shipped BROKEN (the module
// also carries 766d60a9's derefs, so every default riscv64 build
// hard-fails on it).
#![deny(unsafe_code)]

// ============================================================================
// Bit masks
// ============================================================================

/// Preemption disable mask (bits [0:7])
pub const PREEMPT_MASK: i32 = 0x0000_00FF;
/// Softirq mask (bits [8:15])
pub const SOFTIRQ_MASK: i32 = 0x0000_FF00;
/// Hard IRQ mask (bits [16:19])
pub const HARDIRQ_MASK: i32 = 0x000F_0000;
/// NMI mask (bits [20:23], matching Linux's 4-bit NMI field)
pub const NMI_MASK: i32 = 0x00F0_0000;
/// Preempt active flag (bit [26])
pub const PREEMPT_ACTIVE: i32 = 0x0400_0000;

// ============================================================================
// Offsets (add/sub these to preempt_count)
// ============================================================================

/// Preemption disable offset
pub const PREEMPT_OFFSET: i32 = 1;
/// Softirq offset
pub const SOFTIRQ_OFFSET: i32 = 1 << 8;
/// Hard IRQ offset
pub const HARDIRQ_OFFSET: i32 = 1 << 16;
/// NMI offset
pub const NMI_OFFSET: i32 = 1 << 20;

// ============================================================================
// Context query functions
// ============================================================================

/// Per-CPU snapshot of the interrupted task's own preempt_count, taken at
/// timer-trap ENTRY — before irq_enter() books the tick's own
/// HARDIRQ_OFFSET on the current task (fleet s2 session-6 discriminator).
/// The PREEMPT-LEAK probe prints it (pctick=) so an observed pc=0x10001
/// splits into its two producers: pctick=0x1 is a leaked PREEMPT bit
/// (the 0x10000 is the tick's own in-flight offset), while
/// pctick>=0x10001 is a stranded hardirq bit carried from an earlier
/// bracket. Written only from the timer trap path on the owning CPU;
/// read from the tick probe on the same CPU.
pub static TICK_ENTRY_PC: [core::sync::atomic::AtomicU32;
    crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU32::new(0) };
        crate::config::MAX_CPUS];

/// Per-CPU snapshot of the interrupted instruction pointer at timer-trap
/// entry (leg-33B incumbent attribution). For a preempt-off incumbent
/// monopolizing its CPU, the strand census names only pid/pc; the last
/// tick's interrupted RIP names the kernel function it was spinning in
/// (symbolize offline with addr2line). Written on the owning CPU from
/// the same sites as TICK_ENTRY_PC; read from the wakewatch strand
/// print under rux:leakhunt (retired dfx=leakhunt).
pub static TICK_ENTRY_RIP: [core::sync::atomic::AtomicU64;
    crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) };
        crate::config::MAX_CPUS];

/// Per-CPU snapshot of the interrupted code segment (CS) at timer-trap
/// entry, same sites as TICK_ENTRY_RIP (runq-strand lane). The ring
/// bits name USER (cs & 3 == 3) vs KERNEL mode of the interrupted RIP
/// — the discriminator between a userspace spinner (preemptable at
/// user return) and a kernel-loop monopolist (no preempt points).
pub static TICK_ENTRY_CS: [core::sync::atomic::AtomicU64;
    crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) };
        crate::config::MAX_CPUS];

/// Per-CPU snapshot of the interrupted task's pid at timer-trap entry
/// (same sites; lets the monopolist census attribute the stamp to the
/// incumbent it sampled even across a CPU switch).
pub static TICK_ENTRY_PID: [core::sync::atomic::AtomicU32;
    crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU32::new(0) };
        crate::config::MAX_CPUS];

/// Jiffies at the last timer-trap stamp write (stamp freshness: a
/// spinning monopolist refreshes it every tick; a halted or
/// tick-starved CPU leaves it stale).
pub static TICK_ENTRY_JIF: [core::sync::atomic::AtomicU64;
    crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) };
        crate::config::MAX_CPUS];

/// Read current task's preempt_count
#[inline]
pub fn preempt_count() -> i32 {
    match crate::sched::current() {
        Some(task) => task.preempt_count(),
        None => 0,
    }
}

/// Check if in any interrupt context (hardirq + softirq + NMI)
#[inline]
pub fn in_interrupt() -> bool {
    (preempt_count() & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) != 0
}

/// Check if in hard IRQ context
#[inline]
pub fn in_irq() -> bool {
    (preempt_count() & HARDIRQ_MASK) != 0
}

/// Check if in softirq context
#[inline]
pub fn in_softirq() -> bool {
    (preempt_count() & SOFTIRQ_MASK) != 0
}

/// Check if in process (task) context
#[inline]
pub fn in_task() -> bool {
    !in_interrupt()
}

/// The reference's might_sleep() discipline: a task about to sleep must
/// be in task context. Sleeping primitives (SleepMutex::lock's slowpath,
/// IoCompletion::wait, futex waits) call this at their sleeping point —
/// a hit names the SOFTIRQ-context sleeping call that the count-
/// migration leak (Session-6: +SOFTIRQ_OFFSET stranded on the
/// interrupted task when a handler parks) needs to stop producing.
/// The reference BUGs here; the hunting-phase equivalent is a
/// rate-capped splat naming the site, and schedule()'s own
/// __schedule_bug-parity check refuses the switch (see schedule()).
pub fn might_sleep_check(site: &'static str) {
    if !in_interrupt() {
        return;
    }
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(core::sync::atomic::Ordering::Acquire);
    if now.wrapping_sub(last) < 200
        || LAST
            .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
            .is_err()
    {
        return;
    }
    crate::pr_err!(
        "MIGHT-SLEEP-ATOMIC site={} pid={} pc={:#x} in_softirq={} in_irq={}",
        site,
        crate::sched::get_current_pid(),
        preempt_count(),
        in_softirq(),
        in_irq(),
    );
}

/// Check if preemption is allowed
#[inline]
pub fn preemptible() -> bool {
    let pc = preempt_count();
    pc == 0
}

// ============================================================================
// Manipulation functions
// ============================================================================

/// Add value to current task's preempt_count
#[inline]
#[track_caller]
pub fn preempt_count_add(val: i32) {
    if let Some(task) = crate::sched::current() {
        task.add_preempt_count(val);
    }
}

/// Add with an explicit captured acquire-site location — the parameter
/// is kept for the lock helpers' signatures (the acquire-site shadow it
/// fed was deleted with its module; the location is unused).
#[inline]
pub fn preempt_count_add_at(
    val: i32,
    _loc: &'static core::panic::Location<'static>,
) {
    if let Some(task) = crate::sched::current() {
        task.add_preempt_count(val);
    }
}

/// Subtract value from current task's preempt_count
///
/// #[track_caller]: on underflow the report names the guard's drop site
/// (file:line) instead of this generic helper — the leak hunt's missing
/// fingerprint (which lock family double-drops).
#[inline]
#[track_caller]
pub fn preempt_count_sub(val: i32) {
    let loc = core::panic::Location::caller();
    if let Some(task) = crate::sched::current() {
        task.sub_preempt_count_at(val, loc);
    }
}

/// `preempt_count_sub` with an explicit captured drop-site location —
/// used by #[track_caller] inline helpers (spinlock guards) so the
/// underflow report names the GUARD's line, not the helper's.
#[inline]
pub fn preempt_count_sub_at(
    val: i32,
    loc: &'static core::panic::Location<'static>,
) {
    if let Some(task) = crate::sched::current() {
        task.sub_preempt_count_at(val, loc);
    }
}

/// As `preempt_count_sub_at`, plus the guard's lock instance address for
/// the underflow report (leak-hunt round 3e: name WHICH lock).
#[inline]
pub fn preempt_count_sub2_at(
    val: i32,
    loc: &'static core::panic::Location<'static>,
    site: usize,
) {
    if let Some(task) = crate::sched::current() {
        task.sub_preempt_count2_at(val, loc, site);
    }
}

/// R74: reset a corrupted (negative) preempt_count to zero. A negative
/// count is never legal — it is the fingerprint of an unpaired decrement
/// (double-executed guard drop during a double-run). Called only from
/// the fault-path corruption self-heal, with a loud print alongside.
#[inline]
pub fn preempt_count_reset() {
    if let Some(task) = crate::sched::current() {
        task.ti_preempt_count_zero();
    }
}

// ============================================================================
// IRQ entry/exit helpers
// ============================================================================

/// Enter IRQ context — increment hardirq count
///
/// Called at the beginning of hardware interrupt handling.
/// IRQ entry path.
///
/// 119th (epoll-pwait conviction): the bracket's release must act on
/// the task that TOOK it. The +HARDIRQ_OFFSET base is charged to the
/// interrupted task, but the bracket can span a context switch (the
/// tick handler switches inside the bracket), and the release at
/// irq_exit used to land on whatever task was CURRENT at exit — so the
/// interrupted task kept its 0x10000 base FOREVER (the leaked-base
/// face: LEAK-ORIGIN pc=65536 on a parked init) and the task the
/// switch brought in was charged a decrement it never took (the
/// unpaired-decrement face: UNDERFLOW-ORIGIN). The per-CPU booked-task
/// stack below makes the bracket's owner explicit: irq_enter books the
/// current task, irq_exit releases on the BOOKED task.
#[inline]
#[track_caller]
pub fn irq_enter() {
    // 119th: book the task that takes the base (per-CPU depth stack).
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        let cur = crate::sched::current()
            .map(|t| t as *mut crate::process::task::Task as usize)
            .unwrap_or(0);
        let depth = IRQ_BOOK_DEPTH_CNT[cpu].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        if depth < IRQ_BOOK_DEPTH {
            IRQ_BOOKED[cpu][depth].store(cur, core::sync::atomic::Ordering::Release);
        }
    }
    preempt_count_add(HARDIRQ_OFFSET);
    crate::debug::syscost::zone_hardirq_enter(); // syscost probe (no-op when off)
}

/// Per-CPU stack of the tasks that booked +HARDIRQ_OFFSET, one slot per
/// nesting depth (119th). The depth stack (not a single token) is what
/// makes an overwrite visible: with one token per CPU a second booking
/// overwrote the first and the exit side could not see the split.
const IRQ_BOOK_DEPTH: usize = 8;

/// Booked-task stack (119th): [cpu][depth] holds the task pointer that
/// took the +HARDIRQ_OFFSET base at that depth (0 = no task).
static IRQ_BOOKED: [[core::sync::atomic::AtomicUsize; IRQ_BOOK_DEPTH];
    crate::config::MAX_CPUS] =
    [const {
        [const {
            core::sync::atomic::AtomicUsize::new(0)
        }; IRQ_BOOK_DEPTH]
    }; crate::config::MAX_CPUS];

/// Current booking depth per CPU (119th).
static IRQ_BOOK_DEPTH_CNT: [core::sync::atomic::AtomicUsize; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; crate::config::MAX_CPUS];

/// r-strand round 4 (lat_fs conviction, 2026-11-19): does THIS cpu have
/// live (booked, not yet exited) hardirq brackets? The __schedule
/// voluntary-sleep exemption consults it: IRQ-class bits on a voluntary
/// sleeper are zeroed and the park allowed — but while a REAL interrupt
/// bracket is in flight on this cpu those bits are LEGAL nesting, and
/// zeroing them breaks the bracket's paired sub (it underflow-clamps or
/// re-leaks on the sleeper) — the marks-drift/counter-residue family
/// behind the 1k-round livelock (worker parks from inside its write's
/// IO wait with a tick/device IRQ in flight; the exemption wipes the
/// live bracket; every later adjudication sees forged counts).
pub fn hardirq_depth_live(cpu: usize) -> bool {
    cpu < crate::config::MAX_CPUS
        && IRQ_BOOK_DEPTH_CNT[cpu].load(core::sync::atomic::Ordering::Acquire) != 0
}

/// Softirq starvation bound for the irq_exit deferral gate (phantom-bits):
/// 20 ms of the timer timebase. See irq_exit.
const SOFTIRQ_STARVE_TICKS: u64 =
    20 * crate::config::TIMER_CLOCK_FREQ_HZ / 1000;

/// Exit IRQ context — decrement hardirq count and process softirqs
///
/// Called at the end of hardware interrupt handling.
/// IRQ exit path with softirq invocation.
#[inline]
pub fn irq_exit() {
    // 119th (epoll-pwait conviction): release the base on the task that
    // BOOKED it. The bracket can span a context switch (the tick
    // handler switches inside it), so the release used to land on
    // whatever task was current at exit — stranding the 0x10000 base
    // on the interrupted task forever and underflowing the task the
    // switch brought in. The per-CPU booked-task stack names the owner.
    let cpu = crate::arch::cpu_id() as usize;
    let mut booked: usize = 0;
    if cpu < crate::config::MAX_CPUS {
        let depth = IRQ_BOOK_DEPTH_CNT[cpu].load(core::sync::atomic::Ordering::Acquire);
        if depth > 0 {
            IRQ_BOOK_DEPTH_CNT[cpu].store(depth - 1, core::sync::atomic::Ordering::Release);
            let idx = depth - 1;
            if idx < IRQ_BOOK_DEPTH {
                booked = IRQ_BOOKED[cpu][idx].load(core::sync::atomic::Ordering::Acquire);
                IRQ_BOOKED[cpu][idx].store(0, core::sync::atomic::Ordering::Release);
            }
        }
    }
    if booked != 0 {
        let cur = crate::sched::current()
            .map(|t| t as *mut crate::process::task::Task as usize)
            .unwrap_or(0);
        if cur == booked {
            preempt_count_sub(HARDIRQ_OFFSET);
        } else {
            // The bracket spanned a switch: release on the BOOKED task.
            // The task may have exited and been freed inside the
            // bracket (it ran on another CPU) — the safe accessor
            // keeps the write off a freed page; the count of a freed
            // task is meaningless anyway.
            let bpid = crate::process::task::Task::task_pid_of(booked as *mut crate::process::task::Task);
            let released = crate::process::task::Task::release_hardirq_base_on(
                booked as *mut crate::process::task::Task,
                HARDIRQ_OFFSET,
                core::panic::Location::caller(),
            );
            // 119th conviction print: the bracket spanned a switch — the
            // release landed on a task that never took the base. This
            // is the producer of the leaked 0x10000 face. Budgeted.
            {
                static SPLIT_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if SPLIT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                    use crate::console::putchar_no_lock as putchar;
                    const M: &[u8] = b"\nIRQ-BOOK-SPLIT cpu=";
                    for &b in M {
                        putchar(b);
                    }
                    fn pd(mut v: u64) {
                        use crate::console::putchar_no_lock as p;
                        let mut buf = [0u8; 20];
                        let mut n = 0;
                        while v > 0 {
                            buf[n] = b'0' + (v % 10) as u8;
                            v /= 10;
                            n += 1;
                        }
                        if n == 0 {
                            p(b'0');
                        }
                        while n > 0 {
                            n -= 1;
                            p(buf[n]);
                        }
                    }
                    pd(cpu as u64);
                    const M2: &[u8] = b" booked_pid=";
                    for &b in M2 {
                        putchar(b);
                    }
                    pd(bpid as u64);
                    const M3: &[u8] = b" cur_pid=";
                    for &b in M3 {
                        putchar(b);
                    }
                    let cpid = crate::sched::get_current_pid();
                    pd(cpid as u64);
                    const M4: &[u8] = b" released=";
                    for &b in M4 {
                        putchar(b);
                    }
                    pd(released as u64);
                    putchar(b'\n');
                }
            }
        }
    } else {
        // No booking on the stack (early boot, or a bracket whose
        // entry ran before the stack existed): release on the current
        // task — the legacy behaviour.
        preempt_count_sub(HARDIRQ_OFFSET);
    }

    // If we are no longer in hardirq context (outermost irq_exit),
    // check for and process pending softirqs.
    //
    // Skip softirq invocation when there is no current task (secondary
    // CPUs in the early WFI loop have tp=hart_id, not a task_struct).
    // Without a current task, preempt_count is always 0, so in_irq()
    // returns false even during hardware interrupt handling.  Running
    // do_softirq_own_stack() in this state would switch to the same
    // interrupt stack that already holds PtRegs, corrupting the trap
    // frame and crashing the CPU.
    // R14-6 (HIGH-1): also require !in_softirq() — a task holding a
    // lock_bh guard (SOFTIRQ_OFFSET set) that takes this IRQ must NOT run
    // softirq handlers inline at exit: the handler can spin on the very
    // lock the interrupted holder owns (lock_bh was a lie before). Deferring
    // to ksoftirqd is always safe.
    //
    // phantom-bits (2026-11-01): the same deferral now also applies when
    // the interrupted task holds ANY task-context base count (a live
    // spinlock guard books PREEMPT_OFFSET). Softirq handlers take the
    // same locks syscall code takes (net rx → socket queues → unix
    // registry); an inline dispatch over a spinning lock holder wedges
    // the CPU forever: the handler spins with preemption disabled inside
    // an IF=0 window — the tick cannot land (no heal, no preemption),
    // the holder cannot resume (its CPU never exits the window), and the
    // task shows up as the unpreemptible monopolist (preempt=0x100/0x101/
    // 0x102, CFS-undefeatable, the lb60 pid-552 / lb55 UNIX_TABLE
    // both-CPU DEADLOCK family). Deferring costs one ksoftirqd wake —
    // the reference kernel's backlog-to-ksoftirqd behavior under the
    // equivalent condition (its irq_exit runs softirqs inline only
    // outside interrupt context; its task-side users of softirq-shared
    // locks disable bh for exactly this reason).
    //
    // Dispatch black hole fix (UART perf gate round 2, 2026-10-20): the
    // timer IRQ raises its softirqs via raise_softirq_irqoff (no
    // ksoftirqd wake — it expects THIS path to run them). Every deferral
    // branch therefore wakes ksoftirqd: the pending bits stay set, and
    // the woken kthread drains them as soon as the scheduler picks it.
    //
    // phantom-bits starvation bound: deferral assumes ksoftirqd can
    // eventually run on this CPU. A task busy-polling I/O completion
    // with a guard held (bio/queue waiters hold lock_irqsave — base != 0)
    // never yields, ksoftirqd never runs, and the completions the poller
    // waits for ARE the deferred softirqs — a deadlock the make-test
    // boot caught live (VW-WEDGE disk=0 used=28 irqs=0 at sgid_mode).
    // Escape hatch: when this CPU has not run ANY softirq window for 20 ms
    // (SOFTIRQ_WIN_T0 stamps every dispatch, inline or ksoftirqd), force
    // the inline dispatch anyway. Strictly no worse than the old
    // unconditional inline dispatch: a holder still spinning after 20 ms
    // of protected deferral is exactly the case the old behavior wedged
    // on; every shorter hold — the overwhelming majority — keeps the
    // protection.
    if crate::sched::current().is_some() {
        let cpu = crate::arch::cpu_id() as usize;
        let mut starved = false;
        if cpu < crate::config::MAX_CPUS {
            let t0 = crate::interrupt::softirq::SOFTIRQ_WIN_T0[cpu]
                .load(core::sync::atomic::Ordering::Acquire);
            let now = crate::drivers::timer::read_time();
            if t0 == 0 || now.saturating_sub(t0) >= SOFTIRQ_STARVE_TICKS {
                starved = true;
            }
        } else {
            starved = true;
        }
        let base = preempt_count() & PREEMPT_MASK;
        if !in_irq() && !in_softirq() && (base == 0 || starved) {
            crate::interrupt::softirq::invoke_softirq();
        } else if !in_irq() {
            crate::interrupt::ksoftirqd::wakeup_ksoftirqd();
        }
    }
    crate::debug::syscost::zone_hardirq_exit(); // syscost probe (iret frame boundary; no-op when off)
}

// ============================================================================
// NMI entry/exit helpers
// ============================================================================

/// Enter NMI context — increment NMI count
///
/// Unlike `irq_enter()`, NMI entry is lock-free and never invokes softirqs.
/// Must be paired with `nmi_exit()`.
#[inline]
pub fn nmi_enter() {
    preempt_count_add(NMI_OFFSET);
}

/// Exit NMI context — decrement NMI count
///
/// Does NOT invoke softirqs (unlike `irq_exit`) because NMI context
/// must return as quickly as possible.
#[inline]
pub fn nmi_exit() {
    preempt_count_sub(NMI_OFFSET);
}

/// Check if currently in NMI context
#[inline]
pub fn in_nmi() -> bool {
    (preempt_count() & NMI_MASK) != 0
}

/// Full NMI entry path
#[inline]
pub fn irqentry_nmi_enter() {
    nmi_enter();
}

/// Full NMI exit path
#[inline]
pub fn irqentry_nmi_exit() {
    nmi_exit();
}

