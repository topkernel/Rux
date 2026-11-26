//! The SCHED POLICY HALF (crate-flip S1a, the in-crate carve):
//! pick/enqueue/wake-placement/balance/tick/throttle decisions, the
//! class dispatch helpers and the census/diagnostic walks, carved OUT
//! of sched.rs behind the SchedClass seam. sched.rs keeps the CORE
//! half (GRQ + guards, the switch machinery, NEED_RESCHED, per-CPU
//! state, task-slot lifecycle, the walkers). In-crate for S1a — the
//! S1b flip moves this file to the sched crate (the
//! rung-2 recipe); until then the forbid below already enforces the
//! boundary this rung exists to prove: ZERO unsafe in policy.

#![forbid(unsafe_code)]

// The core half's items (bare names stay valid through the glob; the
// visibility of private core items the policy needs is pub(crate) —
// the carve's fixer loop bumped exactly those).
use crate::sched::hotpath::*;
use crate::sched::types::W2RefusalCensus;
// Two definitions this file reached through the glob above while they lived
// in the core's sched module. A definition sits in types now, which the glob
// over hotpath does not cover, so they are named.
use crate::sched::types::{GlobalRunQueue, GrqPendingReport};
// Devirt lane: the shared observability state moved framework-side
// (sched/entities.rs); this crate's census readers keep the bare names.
use crate::sched::hotpath::{CLASS_NONEMPTY, SCHED_CALLS};
use rux::api::sched::{
    CLASS_DL_BIT,
    CLASS_RT_BIT,
    CLASS_CFS_BIT,
    SLOT_W_SCHED_PICK,
    SLOT_W_R56D,
    SLOT_W_R56C,
    SLOT_W_STEAL,
    SLOT_W_ABORT_REPUB,
    WW_EV_WAKE_OK,
    WW_EV_LINK,
    WW_EV_UNLINK_PICK,
    WW_EV_UNLINK_SCHED,
    WW_EV_UNLINK_DEQ,
    WW_EV_UNLINK_POISON,
    WW_EV_PICK_PUB,
    WW_EV_WAKE_REFUSED,
    WW_EV_AFFIRM
};
use crate::errno;
use crate::process::task::{Task, TaskState, SchedPolicy, Pid, PICK_LIVE, PICK_PICKED};
use crate::arch;
use crate::println;
use crate::config::MAX_CPUS;
use alloc::boxed::Box;
use crate::process::pid::alloc_pid;
use core::arch::asm;
use crate::sync::spinlock::RawSpinlock;
use crate::config::MAX_TASKS;
use crate::process;
use crate::debug;
use crate::sync;
use crate::trace;
use crate::sched::r#unsafe;
use crate::sched::hotpath;


/// Convert a nanosecond delta (sched_clock domain) to a jiffies count for
/// the kernel timer wheel, never rounding below one tick.
fn dl_ns_to_jiffies(delta_ns: u64) -> u64 {
    let hz = crate::config::KERNEL_HZ as u64;
    (delta_ns * hz / 1_000_000_000).max(1)
}

/// Arm (or re-arm) the replenish timer for a throttled DL task.
///
/// Safe to call with the GRQ lock held: lock order GRQ -> TIMERS is one-way
/// because the timer softirq delivers its wake_up_process calls only AFTER
/// releasing TIMERS (R12-3), so no path holds TIMERS while acquiring GRQ.
/// The single BTreeMap insert here is the same order of allocation the
/// class-queue inserts already perform under this lock.
fn arm_dl_replenish_timer(task: *mut Task) {
    // INV-SCE-2: callers pass a validated, poison-checked Task pointer.
    {
        let task = crate::sched::r#unsafe::task_shared(task);
        let dl = task.dl_entity();
        // Already armed and still pending (timer ids are one-shot)?
        let cur = dl.replenish_timer.load(core::sync::atomic::Ordering::Acquire);
        if cur != 0 && crate::timer::timer_pending(cur) {
            return;
        }
        let now = crate::sched::hotpath::sched_clock();
        let deadline = dl.deadline.load(core::sync::atomic::Ordering::Acquire);
        let delta_ns = deadline.saturating_sub(now);
        let expires = crate::timer::get_jiffies() + dl_ns_to_jiffies(delta_ns);
        let id = crate::timer::add_timer_wakeup(expires, task.pid());
        if id != 0 {
            dl.replenish_timer.store(id, core::sync::atomic::Ordering::Release);
        }
        // id == 0 (timer table full): leave the task throttled; the next
        // wake attempt retries the arm. Failing to arm must NOT fall back to
        // an early replenish — that re-opens the 100%-CPU hole CBS closes.
    }
}

/// Throttle a DL task whose CBS budget is exhausted: mark it, move it out of
/// the runnable set (INTERRUPTIBLE, so the upcoming __schedule treats it as
/// a blocker instead of demanding a requeue link), and arm the replenish
/// timer for its deadline. Idempotent.
pub(crate) fn dl_throttle(task: &mut Task) {
    let dl = task.dl_entity();
    if dl.dl_throttled.load(core::sync::atomic::Ordering::Acquire) {
        // A second exhaustion report for an already-throttled task can only
        // be a straggler tick — but its timer may have fired and failed to
        // re-arm, so refresh it.
        arm_dl_replenish_timer(core::ptr::from_mut(task));
        return;
    }
    dl.dl_throttled.store(true, core::sync::atomic::Ordering::Release);
    // Only RUNNING tasks are on a CPU; anything else is already blocking.
    if task.state() == TaskState::new(TaskState::RUNNING) {
        task.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    }
    arm_dl_replenish_timer(core::ptr::from_mut(task));
}

/// RT runtime budget per period in jiffies PER-CPU: 950ms.
const RT_RUNTIME_JIFFIES: u64 = 950 * crate::config::KERNEL_HZ as u64 / 1000;

/// RT accounting period in jiffies: 1s.
const RT_PERIOD_JIFFIES: u64 = 1000 * crate::config::KERNEL_HZ as u64 / 1000;

static RT_THROTTLED: [core::sync::atomic::AtomicBool; MAX_CPUS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; MAX_CPUS];

static RT_TIME_USED: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

static RT_PERIOD_START: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

/// Whether the RT class is throttled on `cpu` for pick purposes.
#[inline]
fn rt_throttled(cpu: usize) -> bool {
    cpu < MAX_CPUS && RT_THROTTLED[cpu].load(core::sync::atomic::Ordering::Acquire)
}

/// timer-gap lane: periodic pinned-RT wedge-state dump (rt2xdbg=1,
/// cpu-0's tick only, every 32 jiffies): the slot, idle bit,
/// need_resched, the global/class counters, the RT bitmap words, and the
/// per-prio linked pids with their ownership bits — decides between
/// "linked but never walked", "unlinked while runnable", and
/// "counter desync" in one read.
pub(crate) fn rt_wedge_dump_tick(cpu: usize) {
    if cpu != 0 || crate::cmdline::get_param("rt2xdbg").as_deref() != Some("1") {
        return;
    }
    use crate::debug::taskdump::putc_brief as putchar;
    fn ph(mut v: u64) {
        for _ in 0..16 {
            let n = (v >> 60) as u8;
            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
            v <<= 4;
        }
    }
    static LASTJ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LASTJ.load(core::sync::atomic::Ordering::Acquire);
    if now.wrapping_sub(last) < 32
        && LASTJ
            .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
            .is_err()
    {
        return;
    }
    // SAFETY: read-only diagnostics under the GRQ lock; all task
    // pointers come from the RT lists (linked tasks cannot be freed).
    {
        let g = grq().lock_irqsave();
        let slot = cpu_state(0).current;
        let slot_pid = if slot.is_null() {
                u32::MAX
            } else {
                crate::sched::r#unsafe::task_shared(slot).pid()
            };
        let idle_bit = (grq().idle_cpus.load(core::sync::atomic::Ordering::Acquire) & 1) != 0;
        let nr = g.nr_running.load(core::sync::atomic::Ordering::Acquire);
        let rt_nr = g.rt_rq.nr_running();
        let w0 = g.rt_rq.bitmap_word(0);
        let w1 = g.rt_rq.bitmap_word(1);
        for &b in b"WEDGE jif=" { putchar(b); }
        ph(now);
        for &b in b" slot_pid=" { putchar(b); }
        ph(slot_pid as u64);
        for &b in b" idlebit=" { putchar(b); }
        ph(idle_bit as u64);
        for &b in b" nrsched=" { putchar(b); }
        ph(need_resched() as u64);
        for &b in b" nr=" { putchar(b); }
        ph(nr as u64);
        for &b in b" rt_nr=" { putchar(b); }
        ph(rt_nr as u64);
        for &b in b" bm0=" { putchar(b); }
        ph(w0);
        for &b in b" bm1=" { putchar(b); }
        ph(w1);
        putchar(b'\n');
        // Walk every set prio's list (max 6 tasks printed).
        let mut printed = 0;
        for word_idx in 0..2usize {
            let word = if word_idx == 0 { w0 } else { w1 };
            let mut remaining = word;
            while remaining != 0 && printed < 6 {
                let bit = remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                let prio = 99 - (word_idx * 64 + bit);
                g.rt_rq.walk_prio(prio as usize, &mut |t: *mut Task| {
                    if printed >= 6 || t.is_null() {
                        return;
                    }
                    printed += 1;
                    for &b in b"  RTQ pid=0x" { putchar(b); }
                    ph(crate::sched::r#unsafe::task_shared(t).pid() as u64);
                    for &b in b" p=" { putchar(b); }
                    ph(prio as u64);
                    for &b in b" mask=" { putchar(b); }
                    ph(crate::sched::r#unsafe::task_shared(t).cpus_allowed() as u64);
                    for &b in b" st=0x" { putchar(b); }
                    ph(crate::sched::r#unsafe::task_shared(t).state().bits() as u64);
                    for &b in b" oncpu=" { putchar(b); }
                    ph(crate::sched::r#unsafe::task_shared(t).on_cpu() as u64);
                    for &b in b" claim=" { putchar(b); }
                    ph(crate::sched::r#unsafe::task_shared(t).running_on_cpu.load(core::sync::atomic::Ordering::Relaxed) as u64);
                    for &b in b" pk=" { putchar(b); }
                    ph(crate::sched::r#unsafe::task_shared(t).pick_state() as u64);
                    putchar(b'\n');
                });
            }
        }
    }
}

/// Per-tick RT bandwidth accounting. Called from scheduler_tick with the
/// ticking CPU's current task (no locks — plain atomics).
pub(crate) fn rt_bandwidth_tick(cpu: usize, current: *mut Task) {
    if cpu >= MAX_CPUS {
        return;
    }
    let now = crate::timer::get_jiffies();
    let period_start = RT_PERIOD_START[cpu].load(core::sync::atomic::Ordering::Acquire);
    if period_start == 0 {
        RT_PERIOD_START[cpu].store(now, core::sync::atomic::Ordering::Release);
        return;
    }
    if now.saturating_sub(period_start) >= RT_PERIOD_JIFFIES {
        // Period rollover: replenish the budget, lift the throttle, and let
        // a throttled-away RT task be picked again.
        RT_PERIOD_START[cpu].store(now, core::sync::atomic::Ordering::Release);
        RT_TIME_USED[cpu].store(0, core::sync::atomic::Ordering::Release);
        if RT_THROTTLED[cpu].swap(false, core::sync::atomic::Ordering::AcqRel) {
            // timer-gap lane: throttle LIFT (rt2xdbg=1, cpu-0, 32 lines)
            if cpu == 0 && crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
                static TLN: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if TLN.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 32 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    for &b in b"RT-THROTTLE-LIFT cpu=0\n" { putchar(b); }
                }
            }
            set_need_resched();
        }
        return;
    }

    // SAFETY: current is this CPU's current task, paused inside the tick.
    {
        let policy = crate::sched::r#unsafe::task_shared(current).policy();
        if matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
            let used = RT_TIME_USED[cpu].fetch_add(1, core::sync::atomic::Ordering::AcqRel) + 1;
            if used >= RT_RUNTIME_JIFFIES
                && !RT_THROTTLED[cpu].swap(true, core::sync::atomic::Ordering::AcqRel)
            {
                // timer-gap lane: throttle SET (rt2xdbg=1, cpu-0, 32 lines)
                if cpu == 0 && crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
                    static TSN: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    if TSN.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 32 {
                        use crate::debug::taskdump::putc_brief as putchar;
                        for &b in b"RT-THROTTLE-SET cpu=0\n" { putchar(b); }
                    }
                }
                set_need_resched();
            }
        }
    }
}

/// Emit reports captured under the GRQ. Call AFTER the guard is dropped.
pub fn emit_grq_reports(reports: &[GrqPendingReport]) {
    for r in reports {
        let cl = r.comm.iter().position(|&b| b == 0).unwrap_or(16);
        let comm = core::str::from_utf8(&r.comm[..cl]).unwrap_or("?");
        match r.kind {
            GRQ_RPT_COUNTER_RESYNC => {
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                crate::pr_warn!("STRAND-COUNTER-RESYNC cfs was={} now={}", r.a, r.b);
            }
            GRQ_RPT_SHOUT_RETIRED => {
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                crate::pr_warn!("SWOUT-STALE-RETIRED cpu={} pid={}", r.a, r.b);
            }
            GRQ_RPT_STRAND_ID => {
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                crate::pr_warn!(
                    "STRAND-ID pid={} comm={} st={:#x} qword={:#x} pk={} linked=true ago={}",
                    r.a,
                    comm,
                    r.b,
                    r.c,
                    r.d,
                    r.e,
                );
            }
            GRQ_RPT_STRAND_ID_NOTRACE => {
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                crate::pr_warn!(
                    "STRAND-ID pid={} comm={} st={:#x} qword={:#x} pk={} linked=true trace=none",
                    r.a,
                    comm,
                    r.b,
                    r.c,
                    r.d,
                );
            }
            _ => {}
        }
    }
}

/// Rate-limited one-line report for a poisoned runqueue entry (see
/// kernel_ptr_canonical). Called with the GRQ lock held — the R34
/// no-lock console discipline applies.
pub fn report_poisoned_rq_entry(cpu: usize, raw: usize) {
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST.load(core::sync::atomic::Ordering::Acquire);
    if now.wrapping_sub(last) < 100 {
        return;
    }
    if LAST
        .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::debug::taskdump::putc_brief as putchar;
    fn ph(mut v: u64) {
        let mut buf = [0u8; 16];
        let mut n = 0;
        while v > 0 && n < 16 {
            let d = (v & 0xF) as u8;
            buf[n] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
            v >>= 4;
            n += 1;
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    for &b in b"sched: POISONED rq entry raw=0x" {
        putchar(b);
    }
    ph(raw as u64);
    for &b in b" on cpu " {
        putchar(b);
    }
    let mut c = cpu;
    let mut cb = [0u8; 8];
    let mut n = 0;
    if c == 0 {
        cb[0] = b'0';
        n = 1;
    }
    while c > 0 {
        cb[n] = b'0' + (c % 10) as u8;
        c /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        putchar(cb[i]);
    }
    putchar(b'\n');
}

// grq-hold-slim Cut 2 (class-chain short-circuit): one word answers
// "which scheduling classes are non-empty" for the GRQ-held pick chain,
// so a single-class boot (the standard shape: CFS only) never walks
// DL -> RT -> CFS with a load per class. The bits are maintained ONLY
// at empty <-> nonempty transitions of each class's own nr_running
// counter (every mutation of those counters holds the GRQ, and the pick
// chain reads the mask under the GRQ, so the mask is exactly as
// consistent as the is_empty() loads it replaces — it is DERIVED from
// the same counters at the same serialization point). Relaxed ordering
// suffices: the GRQ lock provides the happens-before, no other
// inter-word ordering is consumed.



// CLASS_NONEMPTY + class_nonempty_set/clear moved FRAMEWORK-side
// (devirt lane): plain state, storage in sched/entities.rs; the
// framework's enqueue/dequeue transitions call it directly and this
// crate reads it through the crate::sched re-exports.

// WW_IDLE_PHASE and SCHED_CS_PHASE moved FRAMEWORK-side (devirt
// lane): per-CPU phase stamps, plain state — storage now in
// sched/entities.rs; this crate's census readers reach them through
// the crate::sched re-exports.

// Wakewatch (s15): per-CPU count of timer-tick-entry samples where the
// hardware tp register disagreed with cpu_state.current (the lying-slot
// shape the R56 steal / abort-republish slot rewrites can produce).
// Sampled by each CPU's own tick (trap.rs) — no cross-CPU register
// reads; printed as a per-second delta next to the phase stamp.
static WW_SLOT_MISMATCH: [core::sync::atomic::AtomicU64; MAX_CPUS] = [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

/// Wakewatch (s15): count one tp-vs-slot divergence observed at tick
/// entry on `cpu` (called from the riscv64 timer trap path).
pub fn ww_note_slot_mismatch(cpu: usize) {
    if cpu < MAX_CPUS {
        WW_SLOT_MISMATCH[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Wakewatch (s15): heartbeat read of the per-CPU mismatch counters
/// (absolute values; the heartbeat computes the delta itself).
pub fn ww_slot_mismatch_counters() -> [u64; MAX_CPUS] {
    let mut out = [0u64; MAX_CPUS];
    for c in 0..MAX_CPUS {
        out[c] = WW_SLOT_MISMATCH[c].load(core::sync::atomic::Ordering::Relaxed);
    }
    out
}

// R76 (slot-divergence producer hunt): tag + task id of the LAST writer of
// each per-CPU current slot. full1/full2's terminal shape (R74-SLOTLESS-TP:
// hardware tp executing a real task while its slot names idle or a third
// task) requires SOME writer to have rewritten the slot while the task kept
// executing — but no heal fired in either capture, and three of the five
// slot writers heal SILENTLY (R56c rewrites a foreign slot with no print;
// the steal and the abort republish print only on their own paths). This
// shadow names the writer site unconditionally, from the R74 print, with
// zero behavioral change. Tags: 1 = __schedule pick publish, 2 = R56d
// own-slot resync, 3 = R56c lying-idle-slot heal (wake path), 4 = harvest
// steal, 5 = switch-abort republish.





// pick_gen_publish moved FRAMEWORK-side (devirt lane): plain RMWs on
// the framework-owned PICK_GEN statics (sched.rs, next to them).


// pick_gen_retire moved FRAMEWORK-side (devirt lane).


/// r11: is `cpu`'s current pick generation a LOSING switch — published,
/// unretired, its slot still naming the published task, past the in-flight
/// floor (2 jiffies; a healthy pick→switch-in retires within the same
/// jiffy)? `task` must be what the slot names (the caller verified
/// cpu_state(cpu).current == task). tag-1 writer required: the heal-side
/// slot rewrites publish+retire in one breath, so an unretired generation
/// under any other writer tag means the ledger was superseded, not lost.
#[inline]
fn pick_gen_is_lost(cpu: usize, task: *mut Task, now_jiffies: u64) -> bool {
    if cpu >= MAX_CPUS {
        return false;
    }
    let tag = SLOT_WRITER[cpu].load(core::sync::atomic::Ordering::Relaxed);
    if tag != SLOT_W_SCHED_PICK {
        return false;
    }
    if PICK_GEN_TASK[cpu].load(core::sync::atomic::Ordering::Relaxed) != task as u64 {
        return false;
    }
    let gen = PICK_GEN[cpu].load(core::sync::atomic::Ordering::Relaxed);
    let done = PICK_GEN_DONE[cpu].load(core::sync::atomic::Ordering::Relaxed);
    if done >= gen {
        return false;
    }
    let published = PICK_GEN_JIF[cpu].load(core::sync::atomic::Ordering::Relaxed);
    now_jiffies.wrapping_sub(published) >= 2
}

/// r11: heal-side slot rewrite (R56c/R56d/steal/abort-repub/idle-heal/
/// the R65 garbage-next guard): whatever generation was in flight is
/// superseded — the pick it described will never materialize (or its
/// outcome is irrelevant: the slot now names something else). Publish
/// and retire the new slot content in one breath so the ledger keeps
/// describing the slot. All callers hold the GRQ (or run on the slot's
/// own CPU with IRQs off — the abort-repub), so the ledger stays
/// serialized against the tag-1 publisher.
// pick_gen_supersede moved FRAMEWORK-side (devirt lane).



/// bool sched_can_stop_tick(struct rq *rq) — reference kernel/sched/
/// core.c, decision tree preserved in order (DL → RR → FIFO → CFS):
/// Deadline tasks need the tick even when single; more than one RR
/// task needs the tick for RR behaviour; FIFO-only needs no tick (no
/// forced preemption between FIFO tasks); otherwise more than one CFS
/// task needs the tick for involuntary preemption. The HZ_BW cfs-
/// bandwidth check is statically false here (no CFS bandwidth). The rq
/// is the global run queue (D-1) — the counters are global; the early-
/// boot guard keeps pre-grq_init ticks periodic (grq() panics before
/// grq_init).
pub fn sched_can_stop_tick() -> bool {
    if !crate::sched::r#unsafe::grq_is_ready() {
        return false;
    }
    let g = grq();
    // Deadline tasks, even if single, need the tick.
    if g.dl_rq.dl_nr_running.load(core::sync::atomic::Ordering::Relaxed) != 0 {
        return false;
    }
    // If there are more than one RR tasks, we need the tick to affect
    // the actual RR behaviour.
    let rr = g.rt_rq.rr_nr_running.load(core::sync::atomic::Ordering::Relaxed);
    if rr != 0 {
        return rr == 1;
    }
    // If there's no RR tasks, but FIFO tasks, we can skip the tick —
    // no forced preemption between FIFO tasks.
    let rt = g.rt_rq.rt_nr_running.load(core::sync::atomic::Ordering::Relaxed);
    if rt.saturating_sub(rr) != 0 {
        return true;
    }
    // More than one CFS task: the tick drives involuntary preemption.
    // STRUCTURAL MAPPING: the reference's rq->nr_running INCLUDES the
    // currently-running task (a task stays on_rq while it runs; the
    // dequeue happens at sleep/exit). Rux's GRQ counter drops the
    // current at pick, so the running non-idle currents are added back
    // here to reproduce the reference's count.
    let mut load = g.nr_running.load(core::sync::atomic::Ordering::Relaxed);
    for c in 0..MAX_CPUS {
        let cur = cpu_state(c).current;
        if !cur.is_null() && cur != cpu_state(c).idle {
            load += 1;
        }
    }
    if load > 1 {
        return false;
    }
    true
}


/// Busy-CPU dispatch for a wake that found no idle CPU (perf-ipc): poke
/// a CPU only when the wakee actually wins the wakeup-preemption
/// comparison — the reference's check_preempt_curr semantics. Policy
/// dispatch mirrors enqueue_task's checks.
pub fn preempt_check_wake(task: *mut Task) {
    if task.is_null() {
        return;
    }
    // SAFETY: task is validated non-null by the caller (Task::wake_up);
    // the check fns only read per-CPU current slots and task fields.
    {
        let allowed = crate::sched::r#unsafe::task_shared(task).cpus_allowed();
        // R3 (rt-r3): the wake ladder steered ti_cpu to select_task_rq's
        // placement BEFORE the enqueue, so ti_cpu names the rq the wakee
        // was enqueued on — the reference's check_preempt_curr target.
        let prefer = crate::sched::r#unsafe::task_shared(task).ti_cpu();
        match crate::sched::r#unsafe::task_shared(task).policy() {
            SchedPolicy::Fifo | SchedPolicy::Rr => check_rt_preempt(task, allowed, prefer),
            SchedPolicy::Deadline => check_dl_preempt(task, allowed, prefer),
            _ => {
                // CFS: the vruntime-credit comparison is unusable as a
                // wakeup gate in this tree — the sleeper-credit clamp
                // (enqueue floor = min_vruntime − SCHED_LATENCY_NS) and
                // the place-on-wake adjustments hold a freshly-pinged
                // peer permanently a few ms of vruntime "behind" its
                // running producer, so a raw check_cfs_preempt passes on
                // EVERY wake of a ping-pong receiver and re-creates the
                // per-record preempt dance (measured: parent +~10 ms
                // vruntime jumps at requeue clamps, CHG probe 2026-10-31).
                // Gate on the MEASURED wait instead: a wakee that last ran
                // within one scheduling-latency window (its producer is
                // mid-batch — IPC drains, log readers, socket pairs) is
                // left to the slice/handoff machinery (the reference
                // batches exactly this way); a wakee that waited a full
                // latency window or more keeps its immediate preempt
                // (the starvation/latency case the wakeup preemption
                // exists for). exec_start holds the last PICK clock; at
                // wake, sched_clock − exec_start ≈ last run + sleep, and
                // the last run is µs-to-ms against a 6 ms threshold.
                let se = crate::sched::r#unsafe::task_shared(task).sched_entity();
                let exec_start = se.exec_start.load(core::sync::atomic::Ordering::Acquire);
                let waited = crate::sched::hotpath::sched_clock().saturating_sub(exec_start);
                // RT-plan R3 (docs/development/rt-kernel-plan.md): the D-14
                // batch window was one full scheduling-latency window
                // (6 ms), which quantized every fair wake that lost the
                // idle-CPU race to the incumbent's slice end — the p2f
                // khog canary read the full 10 ms tick with kpreempt=1
                // armed (perf-gate-baseline.md: the canary row for the
                // 977adb51 batch-peer gate + kpreempt re-land). The
                // reference's own batching scale is the WAKEUP
                // GRANULARITY, not the latency window: sub-granularity
                // waits batch (the D-14 ping-pong win stays — a mid-batch
                // producer's peer waits far below 0.7 ms), granularity-
                // and-above waits preempt immediately. With the R1
                // irq-exit hook armed (ABSENT now arms) the preempt lands
                // at the incumbent's next irq-exit. Measured (p2fcanary,
                // same-image same-binary A/B): khog futex wake->run
                // median 28,135 -> 150 us (187x); quiet 43 -> 31 us;
                // p1_null/p2_ctxsw/p5b_dgram flat.
                if exec_start != 0
                    && waited < crate::sched::hotpath::SCHED_MIN_GRANULARITY_NS
                {
                    return;
                }
                check_cfs_preempt(task, allowed);
            }
        }
    }
}

/// 63rd session (strand-wall hunt): gate for the persistent per-task
/// pick/wake history. The OH boots carry rux:fuxstuck (retired debug=fuxstuck) (the census burst)
/// without schedtrace, and the 61st's burst-sampled RUNQ-STRAND missed
/// the wheel thread's refusal input — so the history follows the census
/// gate too. One relaxed switch-table load per event.
// sched_hist_armed moved FRAMEWORK-side (devirt lane): two trace-flag
// loads — the old row made the pick walk pay a seam round trip to read
// its own kernel's flags.


/// Append one event to `task`'s persistent scheduler-history ring
/// (callers hold the GRQ; writes race nothing).
/// kind: 1=link(wake) 2=refuse 3=pick 4=heal-claim 5=heal-oncpu.
/// `why` carries the R76 skip reason for refuse/heal kinds.
/// `aux` (22 bits) is kind-specific context (cpu id, packed marks).
/// Entry layout: kind:4 | why:8 | jiffies:30 | aux:22.
pub fn sched_hist_ring(task: &Task, kind: u64, why: u64, aux: u64) {
    let jif = crate::timer::get_jiffies() & 0x3FFF_FFFF;
    let v = (kind << 60) | ((why & 0xFF) << 52) | (jif << 22) | (aux & 0x3F_FFFF);
    let i = task.pick_ring_seq.fetch_add(1, core::sync::atomic::Ordering::Relaxed) as usize & 15;
    task.pick_ring[i].store(v, core::sync::atomic::Ordering::Relaxed);
}

/// 63rd: record a pick-walk REFUSAL of `task` (decision time, GRQ held).
/// skip_reason uses the R76 codes 1..5 (see pick_next_cpu).
pub fn sched_hist_refuse(task: *mut Task, skip_reason: u32, cpu: usize) {
    // SAFETY: caller (pick_next_cpu) holds the GRQ and the pointer came
    // from the timeline; only this task's diagnostic fields are touched.
    let t = crate::sched::r#unsafe::task_shared(task);
    t.pick_visit_n.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if let Some(w) = t.pick_refuse_why.get((skip_reason as usize).saturating_sub(1)) {
        w.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    if t.pick_first_refuse_jif.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        t.pick_first_refuse_jif
            .store(crate::timer::get_jiffies(), core::sync::atomic::Ordering::Relaxed);
    }
    let claim = t.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
    let aux = (cpu as u64 & 0xF) | ((t.on_cpu() as u64) << 4) | (((claim as u64) & 0xF) << 5);
    sched_hist_ring(t, 2, skip_reason as u64, aux);
}

/// 63rd: record a successful run-queue LINK of `task` (wake, fork,
/// heal re-insert — GRQ held at every caller).
pub fn sched_hist_link(task: *mut Task) {
    // SAFETY: callers hold the GRQ; diagnostic fields only.
    let t = crate::sched::r#unsafe::task_shared(task);
    t.wake_n.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    t.wake_last_jif
        .store(crate::timer::get_jiffies(), core::sync::atomic::Ordering::Relaxed);
    t.wait_open_ns
        .store(crate::sched::hotpath::sched_clock(), core::sync::atomic::Ordering::Relaxed);
    sched_hist_ring(t, 1, 0, crate::arch::cpu_id() as u64 & 0xF);
}

/// 63rd: record a PICK of `task` (mark_picked_on_cpu, GRQ held) and
/// close the link→pick wait window.
pub fn sched_hist_pick(task: *mut Task, cpu: usize) {
    // SAFETY: callers hold the GRQ; diagnostic fields only.
    let t = crate::sched::r#unsafe::task_shared(task);
    t.pick_n.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    t.pick_last_jif
        .store(crate::timer::get_jiffies(), core::sync::atomic::Ordering::Relaxed);
    let open = t.wait_open_ns.swap(0, core::sync::atomic::Ordering::Relaxed);
    if open != 0 {
        let d = crate::sched::hotpath::sched_clock().saturating_sub(open);
        t.wait_last_ns.store(d, core::sync::atomic::Ordering::Relaxed);
        t.wait_sum_ns.fetch_add(d, core::sync::atomic::Ordering::Relaxed);
        t.wait_max_ns.fetch_max(d, core::sync::atomic::Ordering::Relaxed);
    }
    sched_hist_ring(t, 3, 0, cpu as u64 & 0xF);
}

/// Wakewatch heartbeat snapshot of the scheduler state (timer.rs prints
/// it 1 Hz): [grq_nr, cfs_nr, idle_mask, need_resched_mask, then per CPU
/// (pid, state word, preempt_count) for cpu0/cpu1]. Diagnostic reads
/// only; called from the timer softirq with no scheduler locks held.
pub fn ww_sched_snapshot() -> [u64; 10] {
    let mut out = [0u64; 10];
    out[0] = GlobalRunQueue::grq_nr_running() as u64;
    out[1] = grq().cfs_rq.nr_running() as u64;
    out[2] = grq().idle_cpus.load(core::sync::atomic::Ordering::Acquire) as u64;
    for c in 0..MAX_CPUS {
        // SAFETY: bounded NEED_RESCHED read; cpu_state(c) slots are
        // init-time valid pointers (may race a concurrent switch — the
        // values are diagnostic only).
        {
            if c < 2 {
                out[3] |= (NEED_RESCHED[c].load(core::sync::atomic::Ordering::Acquire) as u64) << c;
                let cur = cpu_state(c).current;
                let base = 4 + c * 3;
                if !cur.is_null() {
                    let cur_t = crate::sched::r#unsafe::task_shared(cur);
                    out[base] = cur_t.pid() as u64;
                    out[base + 1] = cur_t.state().bits() as u64;
                    out[base + 2] = cur_t.preempt_count() as u64;
                } else {
                    out[base] = u64::MAX;
                }
            }
        }
    }
    out
}

/// Wakewatch: CFS timeline geometry for the strand report — the victim's
/// vruntime, the queue's min_vruntime, and the leftmost queued vruntime
/// (all raw ns; caller computes deltas). Diagnostic reads under the GRQ
/// lock (check_picked holds it).
pub fn ww_cfs_geometry(task: *mut Task) -> (u64, u64, u64) {
    let g = grq();
    // SAFETY: scalar sched-entity read of the task being picked (GRQ
    // held); queue walk of the leftmost key only.
    {
        let vr = crate::sched::r#unsafe::task_shared(task).sched_entity().get_vruntime();
        let minv = g.cfs_rq.get_min_vruntime();
        let leftv = g
            .cfs_rq
            .leftmost_vruntime()
            .unwrap_or(minv);
        (vr, minv, leftv)
    }
}

/// s16 (A2 census): count of timeline entries keyed at or before the
/// victim's vruntime — the tie-mass it stands behind (see
/// CfsRunQueue::ww_tie_mass). Rate-gated strand print path only, GRQ
/// held by the caller.
pub fn ww_cfs_tie_mass(task: *mut Task) -> u64 {
    // SAFETY: scalar sched-entity read of the task being picked (GRQ held).
    let vr = crate::sched::r#unsafe::task_shared(task).sched_entity().get_vruntime();
    ww_cfs_tie_mass_vr(vr)
}

/// s16 variant taking the vruntime directly (note_wake side).
pub fn ww_cfs_tie_mass_vr(vr: u64) -> u64 {
    grq().cfs_rq.ww_tie_mass(vr)
}

/// Wakewatch: diagnostic state of a possibly-stalled CPU for the
/// TIMER-SOFTIRQ-STALL report — its current task's pid and preempt_count
/// plus the RIP of its most recent trap frame. Values race live
/// execution (the target CPU keeps running); they name the context the
/// stall recoverer found, nothing more.
pub fn ww_stalled_cpu_state(cpu: usize) -> (u64, u64, u64) {
    if cpu >= MAX_CPUS {
        return (0, 0, 0);
    }
    let cur = cpu_state(cpu).current;
    if cur.is_null() {
        return (0, 0, crate::arch::trap::pt_regs_rip_for_cpu(cpu));
    }
    // SAFETY: init-time valid slot pointer; reads of stable Task fields
    // (pid, preempt_count) are diagnostic-only and tolerate a concurrent
    // switch (worst case a torn-but-bounded value).
    {
        (
            crate::sched::r#unsafe::task_shared(cur).pid() as u64,
            crate::sched::r#unsafe::task_shared(cur).preempt_count() as u64,
            crate::arch::trap::pt_regs_rip_for_cpu(cpu),
        )
    }
}


/// Wakeup-preemption check for a SYNC wake when no CPU is idle:
/// reference parity for WF_SYNC placement is STACKING — wake_affine
/// (fair.c wake_affine_idle: `sync && nr_running==1 → this_cpu`; weight
/// tie-bias → this_cpu) puts the wakee on the WAKER's CPU and only that
/// CPU's wakeup_preempt can fire. We hold the global run queue, so the
/// parity property to preserve is: never notify ANOTHER busy CPU (that
/// would run the wakee in parallel with the still-running waker). Poke
/// only THIS CPU, and only when the wakee wins the same
/// vruntime+granularity comparison check_cfs_preempt uses against the
/// WAKER (the incumbent here). When the comparison fails, no poke: the
/// waker (a binder sender about to park in its read) hands the CPU off
/// through its own schedule().
pub fn preempt_check_wake_sync(task: *mut Task) {
    if task.is_null() {
        return;
    }
    // Task is validated non-null by the caller (Task::wake_up); reads
    // of this CPU's current slot and both tasks' entities (INV-SCE-2).
    {
        let task = crate::sched::r#unsafe::task_shared(task);
        let allowed = task.cpus_allowed();
        let this_cpu = crate::arch::cpu_id() as usize;
        if this_cpu >= MAX_CPUS || (allowed & (1u32 << this_cpu)) == 0 {
            // Wakee may not run here — fall back to the plain policy
            // (better a correct wake than a strict stack).
            preempt_check_wake(core::ptr::from_ref(task) as *mut Task);
            return;
        }
        match task.policy() {
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                let running = cpu_state(this_cpu).current;
                if !running.is_null() && running != cpu_state(this_cpu).idle {
                    let r_policy = crate::sched::r#unsafe::task_shared(running).policy();
                    if r_policy == SchedPolicy::Normal
                        || r_policy == SchedPolicy::Batch
                        || r_policy == SchedPolicy::Idle
                        || ((r_policy == SchedPolicy::Fifo || r_policy == SchedPolicy::Rr)
                            && task.rt_priority() > crate::sched::r#unsafe::task_shared(running).rt_priority())
                    {
                        resched_cpu(this_cpu);
                    }
                }
            }
            SchedPolicy::Deadline => {
                let running = cpu_state(this_cpu).current;
                if !running.is_null() && running != cpu_state(this_cpu).idle {
                    let r_policy = crate::sched::r#unsafe::task_shared(running).policy();
                    let task_dl =
                        task.dl_entity().deadline.load(core::sync::atomic::Ordering::Acquire);
                    if r_policy != SchedPolicy::Deadline
                        || task_dl
                            < crate::sched::r#unsafe::task_shared(running)
                                .dl_entity()
                                .deadline
                                .load(core::sync::atomic::Ordering::Acquire)
                    {
                        resched_cpu(this_cpu);
                    }
                }
            }
            _ => {
                let running = cpu_state(this_cpu).current;
                if running.is_null() || running == cpu_state(this_cpu).idle {
                    return;
                }
                let r_policy = crate::sched::r#unsafe::task_shared(running).policy();
                if r_policy != SchedPolicy::Normal
                    && r_policy != SchedPolicy::Batch
                    && r_policy != SchedPolicy::Idle
                {
                    // RT/DL incumbent: CFS wakee does not preempt.
                    return;
                }
                if crate::sched::r#unsafe::task_shared(task).policy() == SchedPolicy::Idle {
                    return;
                }
                // Same comparison as check_cfs_preempt (vruntime +
                // SCHED_MIN_GRANULARITY_NS), incumbent = the waker.
                let se_vruntime = crate::sched::r#unsafe::task_shared(task).sched_entity().get_vruntime();
                let curr_vruntime = crate::sched::r#unsafe::task_shared(running).sched_entity().get_vruntime();
                if se_vruntime < curr_vruntime {
                    let delta = curr_vruntime - se_vruntime;
                    if delta > crate::sched::hotpath::SCHED_MIN_GRANULARITY_NS {
                        resched_cpu(this_cpu);
                    }
                }
            }
        }
    }
}

/// Pick the next task to run on this CPU.
///
/// Checks in strict priority order: stop → DL → RT → CFS → idle.
/// Respects CPU affinity (cpus_allowed).
pub(crate) fn pick_next_task(grq: &mut GlobalRunQueue, cpu_id: usize, prev: *mut Task) -> *mut Task {
    let pcpu = cpu_state(cpu_id);

    // 1. Stop task (per-CPU, highest priority)
    // TODO: Stop task support - need has_work() check when implemented
    // if !pcpu.stop.is_null() {
    //     return pcpu.stop;
    // }

    // grq-hold-slim Cut 2: the class-chain short-circuit. ONE relaxed
    // load replaces the per-class is_empty() loads (plus the old RT
    // double read: is_empty was evaluated twice, once per if) — a
    // single-class boot never touches the DL/RT arms at all. The mask
    // is derived from each class's own nr_running counter at its
    // empty/nonempty transitions, every one of which holds this same
    // GRQ (see CLASS_NONEMPTY above), so under this lock it is exactly
    // as fresh as the is_empty() loads it replaces.
    let class_mask = CLASS_NONEMPTY.load(core::sync::atomic::Ordering::Relaxed);

    // 2. Deadline — pick earliest-deadline task that can run on this CPU
    if class_mask & CLASS_DL_BIT != 0 {
        if let Some(task) = grq.dl_rq.pick_next_cpu(cpu_id, prev) {
            // PID-0 GATE (b64-b66 fatal chain): PIDs below RESERVED_PIDS
            // are never allocated, so a class-queue entry reading pid 0
            // is an idle task or freed/recycled memory — never a
            // legitimately runnable task. The pick already removed it
            // from the queue; pair the count and fall through to the
            // next class instead of context-switching onto a garbage
            // context (observed: PICK-LIVE-FORCED pid=0 → resume at a
            // stale boot-stack sp → fatal).
            // DEAD-PICK GATE (sc193, v4): a task whose state reads DEAD
            // must never be handed a CPU either — the exit path dequeued
            // it, so a queue entry here means a racing wake/heal re-linked
            // a dying task between the terminal deschedule's dequeue and
            // this pick. Resuming it re-enters the wfi park loop in the
            // dead context and strands the CPU for the rest of the boot
            // (launcher tgid 5033/5034: slot named the dead IPC thread
            // for 13+ min, rip=wfi+5, preempt_count re-leaked to 0x10000
            // by every interrupt that woke the park). Drop it like a
            // poisoned entry; the terminal deschedule loop keeps
            // re-asserting DEAD+dequeue, so the next round picks idle.
            if crate::sched::r#unsafe::task_shared(task).pid() == 0 || crate::sched::r#unsafe::task_shared(task).state().is_dead() {
                report_poisoned_rq_entry(cpu_id, task as usize);
                grq.nr_running.fetch_update(
                    core::sync::atomic::Ordering::SeqCst,
                    core::sync::atomic::Ordering::SeqCst,
                    |v| v.checked_sub(1),
                );
            } else {
                mark_picked_on_cpu(crate::sched::r#unsafe::task_mut(task), task == prev);
                // R32-DL1: reset the CBS execution clock at pick (mirror of the
                // CFS R20-6 fix). exec_start otherwise still holds the
                // timestamp of the task's PREVIOUS switch-out (or 0 before its
                // first run), so the first scheduler_tick/__schedule charge
                // after a wake billed the entire sleep duration (or the whole
                // uptime) into the runtime budget and throttled the task
                // instantly.
                crate::sched::r#unsafe::task_shared(task)
                    .dl_entity()
                    .exec_start
                    .store(crate::sched::hotpath::sched_clock(), core::sync::atomic::Ordering::Release);
                // R7-B5: pick removes the task from the queue — pair the count.
                grq.nr_running.fetch_update(
                    core::sync::atomic::Ordering::SeqCst,
                    core::sync::atomic::Ordering::SeqCst,
                    |v| v.checked_sub(1),
                );
                return task;
            }
        }
    }

    // 3. RT — pick highest-priority task that can run on this CPU.
    //    Skipped while the RT bandwidth is throttled ON THIS CPU (the
    //    70th's per-cpu rt_rq parity; a throttled cpu falls through to
    //    CFS while other cpus may still pick RT).
    // timer-gap lane: the pinned-RT wedge's prime silent skip — a cpu
    // whose RT class stays throttled picks idle forever while linked RT
    // tasks wait. Name the skip live (rt2xdbg=1, cpu-0-targeted, 32-line
    // budget): the accounting triple (used/period_start/now jiffies)
    // decides between "throttle never lifts" and "lifts but re-arms".
    // grq-hold-slim Cut 2: the class bit gates the whole arm (mask
    // load above); rt_throttled is now evaluated ONCE per RT-nonempty
    // pick instead of twice.
    if class_mask & CLASS_RT_BIT != 0 {
        let rt_is_throttled = rt_throttled(cpu_id);
        if rt_is_throttled {
        // Rule 1 (grq-lock-1st): the opt-in rt2xdbg census — event
        // ring, not the console (a packs used|period_start, b packs
        // now|rt_nr).
        if cpu_id == 0 && crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
            crate::sched_ev::sched_ev(
                rux::api::sched::EV_RT_THROTTLED,
                cpu_id as u32,
                (RT_TIME_USED[cpu_id].load(core::sync::atomic::Ordering::Relaxed) << 32)
                    | RT_PERIOD_START[cpu_id].load(core::sync::atomic::Ordering::Relaxed),
                (crate::timer::get_jiffies() << 32)
                    | grq.rt_rq.nr_running() as u64,
            );
        }
    }
        if !rt_is_throttled {
        // timer-gap lane (self-migration handoff retry): the walk names
        // an affinity-allowed task it refused only because its owner is
        // mid-switch-OUT. Re-arm NEED_RESCHED when that owner's switch
        // is verifiably in flight (SWITCHING_OUT marker set): the
        // release store lands within microseconds, and the next
        // trap/syscall exit retries the pick and takes the task.
        // Without this, the affinity syscall's single migration poke
        // was consumed by a pick that raced the switch-out, leaving a
        // pinned RT task pickable-but-never-picked on the only cpu its
        // mask allows (the pi_stress -g 2 wedge shape). No marker =
        // the ownership mark is orphaned (the R56 family) — do NOT
        // re-arm; the heal machinery owns that shape, and a blind
        // retry would livelock the resched path.
        let mut skip_owner = usize::MAX;
        let rt_picked = crate::rt::rt_pick_next_cpu(&mut grq.rt_rq, cpu_id, prev, &mut skip_owner);
        if skip_owner < MAX_CPUS
            && SWITCHING_OUT[skip_owner].load(core::sync::atomic::Ordering::Acquire) != 0
        {
            if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
                static SOR: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if SOR.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    for &b in b"SOR-REARM-RISK-RETRY owner=" { putchar(b); }
                    let mut v = skip_owner as u64;
                    for _ in 0..4 {
                        let n = (v >> 28) as u8;
                        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                        v <<= 4;
                    }
                    putchar(b'\n');
                }
            }
            set_need_resched();
        }
        if let Some(task) = rt_picked {
            // PID-0 GATE — see the Deadline block above. Also the
            // DEAD-PICK GATE (sc193, v4) — see the Deadline block above.
            if crate::sched::r#unsafe::task_shared(task).pid() == 0 || crate::sched::r#unsafe::task_shared(task).state().is_dead() {
                report_poisoned_rq_entry(cpu_id, task as usize);
                grq.nr_running.fetch_update(
                    core::sync::atomic::Ordering::SeqCst,
                    core::sync::atomic::Ordering::SeqCst,
                    |v| v.checked_sub(1),
                );
            } else {
                mark_picked_on_cpu(crate::sched::r#unsafe::task_mut(task), task == prev);
                grq.nr_running.fetch_update(
                    core::sync::atomic::Ordering::SeqCst,
                    core::sync::atomic::Ordering::SeqCst,
                    |v| v.checked_sub(1),
                );
                return task;
            }
        }
        }
    }

    // 4. CFS — pick min-vruntime task that can run on this CPU
    if class_mask & CLASS_CFS_BIT != 0 {
        if let Some(task) = crate::fair::cfs_pick_next_cpu(&mut grq.cfs_rq, cpu_id, prev) {
            // PID-0 GATE — see the Deadline block above. This is the exact
            // site of the b64-b66 fatal: the CFS walk returned a
            // freed-then-recycled entry whose pid word read 0 with a claim
            // word reading 0 (= cpu 0), mark_picked_on_cpu published it
            // LIVE ("PICK-LIVE-FORCED pid=0") and the switch resumed a
            // stale boot-stack context (rsp=0x10a870).
            // DEAD-PICK GATE (sc193, v4) — see the Deadline block above;
            // the CFS queue is where the re-linked launcher IPC thread
            // (tgid 5033) was re-picked from, parking cpu1 in wfi+5 for
            // 13+ min.
            if crate::sched::r#unsafe::task_shared(task).pid() == 0 || crate::sched::r#unsafe::task_shared(task).state().is_dead() {
                report_poisoned_rq_entry(cpu_id, task as usize);
                grq.nr_running.fetch_update(
                    core::sync::atomic::Ordering::SeqCst,
                    core::sync::atomic::Ordering::SeqCst,
                    |v| v.checked_sub(1),
                );
            } else {
                mark_picked_on_cpu(crate::sched::r#unsafe::task_mut(task), task == prev);
                // grq-10ns: single locked-RMW form (one xadd) instead of
                // the fetch_update CAS loop — every writer of this
                // counter holds the GRQ, so a wrap (old == 0) is
                // restored with a plain store, keeping the checked-sub
                // semantics with one bus op (see fair::nr_running_dec).
                {
                    let old = grq.nr_running.fetch_sub(
                        1,
                        core::sync::atomic::Ordering::SeqCst,
                    );
                    if old == 0 {
                        grq.nr_running.store(
                            0,
                            core::sync::atomic::Ordering::SeqCst,
                        );
                    }
                }
                grq.cfs_rq.set_curr(task);
                // grq-hold-slim Cut 1: the picked task's OWN entity
                // bookkeeping (exec_start reset, activation-window
                // stamp, slice math, time_slice publish) moved OUT of
                // the GRQ hold — it runs in __schedule right after the
                // unlock, still with IRQs off and before the context
                // switch / repick return. Nothing can read those fields
                // before the picked task actually runs: it was
                // unpickable to other CPUs before this pick, the slot
                // publish names it only as ours, and its own CPU's
                // tick/stat readers run post-switch. The R20-6 exec
                // clock reset and the Batch-8 activation window are
                // unchanged, just relocated.
                return task;
            }
        }
    }

    // 5. Nothing runnable → idle task
    pcpu.idle
}

// PT_TASK/PT_CODE/PT_JIF moved FRAMEWORK-side (devirt lane; sched/entities.rs).

// pick_trace moved FRAMEWORK-side (devirt lane): pure diagnostic
// stamping over entities.rs tables — no policy content.

/// Reader-side: the task's last recorded phase (code, jiffies-ago).
// pick_trace_peek moved FRAMEWORK-side (devirt lane).

pub static PSLEDGER_TASK: [core::sync::atomic::AtomicUsize; 1024] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 1024];

pub static PSLEDGER_CPU: [core::sync::atomic::AtomicU32; 1024] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 1024];

pub static PSLEDGER_JIF: [core::sync::atomic::AtomicU64; 1024] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 1024];

pub static PSLEDGER_PK: [core::sync::atomic::AtomicU32; 1024] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 1024];

#[inline]
fn psl_slot(task: *mut Task) -> usize {
    ((task as usize) >> 4) & 1023
}

/// Record a pick publication (GRQ held; single writer per task).
/// rux:leakhunt (retired debug=leakhunt)-gated: the unconditional cost is one relaxed load.
pub fn psledger_note_publish(task: *mut Task, cpu: usize, pk: u32) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    let s = psl_slot(task);
    PSLEDGER_TASK[s].store(task as usize, core::sync::atomic::Ordering::Release);
    PSLEDGER_CPU[s].store(cpu as u32, core::sync::atomic::Ordering::Release);
    PSLEDGER_PK[s].store(pk, core::sync::atomic::Ordering::Release);
    PSLEDGER_JIF[s]
        .store(crate::timer::get_jiffies(), core::sync::atomic::Ordering::Release);
}

/// Consume the ledger at a pick termination (affirm or abort). Returns
/// the consumed entry for the caller's reporting.
pub fn psledger_consume(task: *mut Task) -> bool {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return false;
    }
    let s = psl_slot(task);
    PSLEDGER_TASK[s]
        .compare_exchange(
            task as usize,
            0,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Acquire,
        )
        .is_ok()
}

/// Read the ledger without consuming (heal-side diagnostic).
pub fn psledger_peek(task: *mut Task) -> Option<(u32, u32, u64)> {
    let s = psl_slot(task);
    if PSLEDGER_TASK[s].load(core::sync::atomic::Ordering::Acquire) == task as usize {
        Some((
            PSLEDGER_CPU[s].load(core::sync::atomic::Ordering::Acquire),
            PSLEDGER_PK[s].load(core::sync::atomic::Ordering::Acquire),
            PSLEDGER_JIF[s].load(core::sync::atomic::Ordering::Acquire),
        ))
    } else {
        None
    }
}

/// Mark a picked task on-CPU (R8-1b). Called under the GRQ lock; __switch_to
/// clears it after the task's outgoing context is saved. Other CPUs' picks
/// skip on-CPU tasks, so a RUNNING-but-unsaved task can never be picked
/// twice — the NEW2 root cause. The idle task is never marked (it is not
/// on the class queues; clearing a never-set flag is a harmless no-op).
#[inline]
fn mark_picked_on_cpu(task: &mut Task, is_repick: bool) {
    // The raw identity of the picked task, for the helper APIs still on
    // pointer currency (rings, ledgers) — INV-SCE-1 supplies the
    // reference; the pointer is its address, not a second derivation.
    let task_ptr = core::ptr::from_mut(task);
    {
        // Wakewatch ring (118th): the PICK unlink edge — the pick removes
        // the task from its class queue HERE; if the switch-in is later
        // abandoned (revoked / no affirm), this event plus the missing A
        // names the strand. Single site covers the DL/RT/CFS pick arms.
        ww_record(task_ptr, WW_EV_UNLINK_PICK);
        // Wakewatch forensics: consume the wake stamp and classify the
        // wake→pick latency (direct-console prints follow the R34
        // discipline — we hold the GRQ lock here).
        crate::wakewatch::check_picked(task_ptr, crate::arch::cpu_id() as usize);
        // 63rd: persistent per-task history (strand-wall hunt) — the
        // pick_n counter vs a frozen exec clock is the
        // picked-but-never-executes signature.
        if sched_hist_armed() {
            sched_hist_pick(task_ptr, crate::arch::cpu_id() as usize);
        }
        // Scribble hunter (R65 double-run seed tripwire): we hold the GRQ
        // lock here, so the ownership picture is stable. A task that is
        // STILL another CPU's published current (sched slot) or still
        // carries another CPU's running_on_cpu claim is being resumed
        // while it executes — the pick only got here because the on_cpu
        // byte read false, i.e. the ownership word {ti_on_cpu, claim} was
        // reset outside the publish protocol (x86-scribble3: the word
        // held a kernel-static pointer; a single 8-byte store yields
        // {false, -1}). Name the seed the moment the scheduler first
        // sees it, with the pre-mark state.
        #[cfg(feature = "x86_64")]
        if crate::trace::EVENT_SCRIBBLE.armed() {
            let me = crate::arch::cpu_id() as usize;
            let mut owner_slot: i32 = -1;
            for c in 0..crate::config::MAX_CPUS {
                if c != me && cpu_state(c).current == task {
                    owner_slot = c as i32;
                    break;
                }
            }
            let claim = task.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
            if owner_slot >= 0 || (claim != -1 && claim as usize != me) {
                crate::debug::scribble::double_seed(task_ptr, me as u64, owner_slot, claim);
            }
        }
        // R74 (riscv64 double-run seed tripwire, always on): same check as
        // the x86 scribble hunter's double_seed, SBI-direct and
        // unconditional — the r74g capture ended with TWO harts both
        // executing as pid 420 (both printed noctx with the same tp and a
        // preempt_count of -1: double-executed guard drops), so the seed
        // pick must be named the moment it happens. owner_slot = a foreign
        // slot still naming the task; claim = a foreign claim on it.
        #[cfg(feature = "riscv64")]
        {
            let me = crate::arch::cpu_id() as usize;
            let mut owner_slot: i32 = -1;
            for c in 0..crate::config::MAX_CPUS {
                if c != me && cpu_state(c).current == task_ptr {
                    owner_slot = c as i32;
                    break;
                }
            }
            let claim = task.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
            let pick_st = task.pick_state();
            let tp = crate::arch::cpu::get_thread_id() as usize;
            let tp_is_task = tp == task_ptr as usize;
            // NOTE: pick_st==LIVE alone is NOT a seed signal — it persists
            // after a legitimate switch-out (the release store does not
            // reset it). Only a foreign slot or a foreign claim convicts.
            if owner_slot >= 0 || (claim != -1 && claim as usize != me) {
                r56_print_pid(
                    b"R74-DOUBLE-SEED cpu=", me as u32, b" pid=");
                r56_print_pid(b"", task.pid(), b" owner_slot=");
                r56_print_pid(b"", owner_slot as u32, b" claim=");
                r56_print_pid(b"", claim as u32, b" pick_st=");
                r56_print_pid(b"", pick_st, b" tp_is_task=");
                r56_print_pid(b"", tp_is_task as u32, b"\n");
            }
        }
        // Scribble hunter: record the pick (rux:scribble (retired debug=scribble)).
        #[cfg(feature = "x86_64")]
        crate::debug::scribble::ring_log(
            crate::arch::cpu_id() as usize,
            1,
            task_ptr as u64,
            0,
        );
        // R56-heal: publish the FULL ownership qword {on_cpu = true,
        // claim = this cpu} — not just the on_cpu byte — plus the pick
        // state. A fresh pick publishes PICKED: the switch-in must affirm
        // it LIVE (atomic CAS right before the register restore) or abort
        // on a harvester revocation. The next == prev fast-path repick
        // publishes LIVE directly — the task never stopped running, and
        // marking it PICKED would leave a running task revocable.
        //
        // R57 (choke point): "never stopped running" must be decided by
        // OWNERSHIP, not only by the caller's is_repick arithmetic. If
        // this CPU's slot still names the task or the task's claim names
        // this CPU, the task believes it is executing here RIGHT NOW
        // (its callee-saved context is live on this hart's stack) —
        // publish LIVE no matter which pick route arrived, or a
        // mislabeled "fresh" pick would make the running task revocable
        // by the next steal that reads it PICKED. The forced-LIVE
        // correction is reported: seeing it means a caller computed
        // is_repick=false for a task this CPU owns — a bug worth naming.
        let me = crate::arch::cpu_id() as usize;
        let owned_here = (me < MAX_CPUS && cpu_state(me).current == task_ptr)
            || task.running_on_cpu.load(core::sync::atomic::Ordering::Acquire) == me as i32;
        let live = is_repick || owned_here;
        if owned_here && !is_repick {
            // First-hit-only: an in-tree impossibility — if a caller ever
            // computes is_repick=false for a task this CPU owns, say so
            // once, loudly, then keep the machine safe (LIVE publish).
            static PICK_LIVE_FORCED_SEEN: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            if !PICK_LIVE_FORCED_SEEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
                r56_print_pid(b"PICK-LIVE-FORCED cpu=", me as u32, b" pid=");
                r56_print_pid(b"", task.pid(), b"\n");
            }
        }
        // leg-26 (PICK-PC-ZERO): a task being picked FRESH (not live on
        // this CPU) provably executes nowhere at this instant — the
        // ownership screens just passed — so by the same invariant the
        // parkwalk trusts (guards exist only on the executing task;
        // preemption requires count 0; __schedule refuses to sleep a
        // nonzero-count task) it holds NO legal preempt_count bits.
        // Any residue is phantom (the parked-pc=1 pile, the Signature-A
        // spawn phantoms, the leaked-pc incumbents that strand every
        // wake behind an un-preemptible monopoly until the 200-tick
        // heal). Zero it AT the pick — the one race-free point — so a
        // resurrected phantom runs preemptible from its first
        // instruction instead of monopolizing for up to 2 s first.
        // A legitimately-open guard across a park cannot exist (the
        // invariant above); if one ever does, its drop underflow-clamps
        // and REPORTS rather than poisoning. Rate-gated one line per 2 j.
        if !live {
            let pc = task.preempt_count();
            let base = pc & !crate::interrupt::preempt::HARDIRQ_MASK;
            let nest_extra = (pc & crate::interrupt::preempt::HARDIRQ_MASK)
                .saturating_sub(crate::interrupt::preempt::HARDIRQ_OFFSET);
            if base != 0 || nest_extra > 0 || (pc & crate::interrupt::preempt::NMI_MASK) != 0 {
                task.ti_preempt_count_zero();
                static PICKZERO_LAST: core::sync::atomic::AtomicU64 =
                    core::sync::atomic::AtomicU64::new(0);
                let now = crate::timer::get_jiffies();
                let last = PICKZERO_LAST.load(core::sync::atomic::Ordering::Relaxed);
                if now.wrapping_sub(last) >= 20
                    && PICKZERO_LAST
                        .compare_exchange(
                            last,
                            now,
                            core::sync::atomic::Ordering::AcqRel,
                            core::sync::atomic::Ordering::Acquire,
                        )
                        .is_ok()
                {
                    r56_print_pid(
                        b"PICK-PC-ZERO pid=",
                        task.pid(),
                        b" was=0x",
                    );
                    r56_print_pid(b"", pc as u32, b"\n");
                }
            }
        }
        task.publish_pick_ownership(crate::arch::cpu_id() as i32, live);
        // pick→switch ledger: record the publication; the switch-in
        // affirm/abort consume it. An intact record at a later stale
        // heal = switchless pick.
        psledger_note_publish(
            task_ptr,
            crate::arch::cpu_id() as usize,
            if live { crate::process::task::PICK_LIVE } else { crate::process::task::PICK_PICKED },
        );
        pick_trace(task_ptr, 2);
    }
}

/// R41 (stale on_rq defense, shared): after a REFUSED enqueue_task_locked,
/// find out whether the refusal hid a stale on_rq=true on an UNLINKED task
/// and heal it. Returns true when the task is linked when this returns —
/// either it verifiably already was (legitimate refusal: RR tick rotation,
/// a racing wake, change_task_policy's re-enqueue — a forced insert there
/// would double-link), or the heal cleared the stale flag and re-inserted
/// through the normal path.
///
/// The R41 audit found every on_rq write point in the tree link/unlink-
/// paired under the GRQ lock (fair.rs enqueue/dequeue/pick*, rt.rs
/// enqueue/dequeue, deadline.rs enqueue/dequeue/pick*), so no CURRENT-code
/// path manufactures a stale on_rq=true — it can only arrive from a pre-R39
/// legacy state, an out-of-scheduler memory-corruption engine (this
/// kernel's documented smash families), or a future regression. This
/// defense makes both wedge morphologies unconstructible regardless of
/// provenance: the B-form (prev preempted while phantom-flagged → RUNNING,
/// unlinked, never picked; round-41 capture: pid 359/360 of
/// `echo PP | cat`, 4 CPUs wfi-idle, stacks frozen in
/// schedule → __schedule → context_switch) via requeue_prev_locked, and
/// the A-form twin (wake of a phantom-flagged sleeper refused → the
/// wait-queue's one-shot wake token silently consumed) via
/// wake_up_enqueue. The tripwire print turns every incident into a loud,
/// self-healing event that pins the manufacturer for the follow-up hunt.
///
/// Called with the GRQ lock held, only on the refused path.
pub(crate) fn ensure_linked_locked(grq: &mut GlobalRunQueue, task: &mut Task) -> bool {
    // Pointer identity for the queue-internal linkage queries (the class
    // rqueues match on the embedded node's address).
    let task_ptr = core::ptr::from_mut(task);
    // Re-run the guards that legitimately refuse (freed-page poison,
    // exiting task) BEFORE any field read below — a freed page cannot be
    // healed, and the policy/entity reads must not touch it.
    if task.pid() == TASK_POISON || task.state().is_dead() {
        return false;
    }

    // Batch 8 (CBS): a throttled DL task is LEGITIMATELY unlinked — its
    // budget window has not expired. Do not "heal" it (the forced re-insert
    // below would re-open the 100%-CPU hole CBS closes); the replenish
    // timer is the guaranteed wake that re-links it.
    if task.policy() == SchedPolicy::Deadline
        && task.dl_entity().dl_throttled.load(core::sync::atomic::Ordering::Acquire)
    {
        arm_dl_replenish_timer(task_ptr);
        return false;
    }

    // Check ACTUAL linkage — never trust the flag that just lied.
    let policy = task.policy();
    let linked = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => grq.rt_rq.is_linked(task_ptr),
        SchedPolicy::Deadline => grq.dl_rq.is_linked(task_ptr),
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            grq.cfs_rq.is_linked(task_ptr)
        }
    };
    if linked {
        return true; // legitimate refusal: already queued exactly once
    }

    // Stale on_rq=true on an unlinked task — the wedge precursor. Report
    // (SBI direct write: we hold the GRQ lock — R34 discipline), clear the
    // stale flag, and re-insert through the normal path.
    {
        const MSG: &[u8] = b"R41-STALE-ONRQ healed pid=0x";
        for &b in MSG {
            crate::debug::taskdump::putc_brief(b);
        }
        let v = task.pid() as u64;
        let mut sh = 64;
        while sh > 0 {
            sh -= 4;
            let nb = ((v >> sh) & 0xF) as u8;
            crate::debug::taskdump::putc_brief(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
        }
        crate::debug::taskdump::putc_brief(b'\n');
    }
    match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            task.rt_entity().set_on_rq(false);
        }
        SchedPolicy::Deadline => {
            task.dl_entity().on_rq.store(
                false,
                core::sync::atomic::Ordering::Release,
            );
        }
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            task.sched_entity().set_on_rq(false);
        }
    }
    // With the flag cleared the class insert is admitted, and its success
    // pairs set_state(RUNNING) + grq.nr_running with the actual linkage
    // (R7-B5 discipline).
    // Bridge (until enqueue_task_locked's own conversion lands): the one
    // audited call with pointer currency, same INV-SCE-1 exclusion.
    enqueue_task_locked(grq, task)
}

/// R41 (B-form phantom — prev re-enqueue): requeue `prev` for its upcoming
/// switch-out. `prev` is this CPU's current with state==RUNNING; once the
/// context_switch below stores its context it is on NO cpu, so it MUST be
/// linked on a class queue when this returns. A plain enqueue_task_locked
/// call trusted the class on_rq guard — a stale-true flag dropped prev
/// permanently (RUNNING-but-unlinked, never picked again). On refusal the
/// shared ensure_linked_locked verifies actual linkage and heals only the
/// genuine divergence, so legitimately-linked prevs (RR rotation, racing
/// wake, policy-change re-enqueue) are never double-linked.
///
/// Called with the GRQ lock held.
pub(crate) fn requeue_prev_locked(grq: &mut GlobalRunQueue, prev: &mut Task) {
    // Bridge (same as ensure_linked_locked's tail): pointer currency for
    // the pre-conversion enqueue path.
    if enqueue_task_locked(grq, prev) {
        return; // normal path: fresh insert linked prev (state already RUNNING)
    }
    // Refused — verify and heal (no-op skip when prev is already linked).
    ensure_linked_locked(grq, prev);
}

/// Enqueue a task into the global RQ (called with GRQ lock held).
///
/// R39: returns whether the task was actually inserted by this call. False
/// covers the freed-task poison, the exiting-task guard, AND the class
/// enqueues' own on_rq double-enqueue guard (rt/fair/dl all return false
/// when it skips — R7-B5 depends on that to keep grq.nr_running balanced),
/// which is unreachable for every legitimate caller state. On false the
/// task's state is left untouched (SLEEPING stays wakeable; a prev that was
/// already RUNNING stays RUNNING — it is on_cpu, so the divergence is the
/// pre-existing stale-on_rq form, not manufactured here).
#[track_caller]
pub(crate) fn enqueue_task_locked(grq: &mut GlobalRunQueue, task: &mut Task) -> bool {
    let ENQ_CALLER: &'static core::panic::Location<'static> = core::panic::Location::caller();
    // Pointer identity for the class enqueues / helpers on raw currency.
    let task_ptr = core::ptr::from_mut(task);

    // R15-6 (S-R resurrection guard): refuse to enqueue a FREED Task.
    // free_task_slot poisons state/pid with 0xDEADBEEF, but the poison
    // still satisfies is_sleeping() (bit0 of 0xEF is set), so a stale
    // pid-hash reader (synchronize_rcu is a no-op: RCU_GEN never
    // advances — only rcu_softirq_handler bumps it and nothing raises
    // the Rcu vector) can pass the freed page's on_rq==false guard and
    // insert the dead pointer into tasks_timeline. The next pick then
    // context-switches onto the reused page and __switch_to's
    // `ld ra, thread_ra` + `ret` transfers control to whatever
    // pointer-shaped bytes now occupy that offset — the S-R signature
    // (kernel illegal instruction at a page-aligned linear-map heap
    // address). Detect the poison, report, and drop the enqueue.
    {
        // free_task_slot poisons state AND pid (two u32 writes at their
        // compile-time offsets — task_offsets::TASK_STATE/TASK_PID; R47 fixed
        // the stale hardcoded +0x48 that landed in ti_a2); a real PID can
        // never equal 0xDEADBEEF, so the pid check alone is an unambiguous
        // freed-page detector.
        let pid = task.pid();
        if pid == TASK_POISON {
            // Rule 1 (grq-lock-1st): NO print under the GRQ — the event
            // ring carries the evidence; the tick drain prints it.
            crate::sched_ev::sched_ev(
                rux::api::sched::EV_ENQ_POISONED,
                pid,
                0,
                0,
            );
            return false;
        }
    }

    // R33 (A-family wedge — dead-task resurrection): refuse an EXITING task
    // BEFORE the unconditional set_state(RUNNING) below. do_exit sets ZOMBIE
    // before its own dequeue; a wake_up whose UNLOCKED is_sleeping() filter
    // read predated the target's exit would otherwise flip ZOMBIE→RUNNING
    // and re-queue the dying task AFTER its dequeue. The reaping parent's
    // release_task could then free the kernel stack while another CPU picks
    // the zombie and __switch_to loads thread.sp (freed stack) into sp —
    // the A-type wedge (trap sequence storing to a heap address that no
    // longer maps the stack). ZOMBIE/DEAD are terminal here: nothing
    // legitimately enqueues an exiting task. (0xDEADBEEF poison also trips
    // is_dead() — bit 0x20 set — as a second line behind the pid check.)
    {
        let st = task.state();
        if st.is_dead() {
            // Rule 1: event ring, not the console (was console::putchar
            // under GRQ).
            crate::sched_ev::sched_ev(
                rux::api::sched::EV_ENQ_DEAD,
                task.pid(),
                st.bits() as u64,
                0,
            );
            return false;
        }
    }

    // R39 (B-form hardening): write RUNNING only when the task is actually
    // going to be linked. The old unconditional set_state(RUNNING) ran
    // BEFORE the class insert; every class insert can silently refuse via
    // its on_rq double-enqueue guard. For a task whose on_rq flag is
    // (erroneously or stale-ly) true while it is NOT linked in the tree,
    // the old order produced EXACTLY the captured B-form morphology —
    // state=RUNNING, task on no queue and no CPU, never scheduled again
    // (icount round-38 snapshot: pid 344 mrsh, 4 CPUs wfi-idle, runqueues
    // empty). With the write moved after the insert decision:
    //   - every legitimate path is unchanged — all callers reach here with
    //     state==RUNNING whenever the guard legitimately skips (a linked
    //     task is always RUNNING: __schedule re-enqueues only prev_running,
    //     the RR tick rotation gates on RUNNING, fork/kthread tasks are
    //     freshly inserted as TASK_NEW and get RUNNING only from this
    //     branch, change_task_policy dequeues before re-enqueue);
    //   - a refused insert now leaves the task SLEEPING (honest, form-A,
    //     still wakeable) instead of a phantom RUNNING that nothing will
    //     ever schedule — and leaves a fresh TASK_NEW task TASK_NEW (R52),
    //     so the creating fork/kthread can detect the refusal and unwind.
    let policy = task.policy();

    // R7-B5: count only actual insertions. The class enqueues all carry a
    // double-enqueue guard (on_rq); incrementing nr_running unconditionally
    // while the guard skipped inflated the counter on every redundant
    // wake — the idle fast path in __schedule then never triggered again.
    let inserted = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            // Scribble hunter: record the class insert (rux:scribble (retired debug=scribble)).
            #[cfg(feature = "x86_64")]
            crate::debug::scribble::ring_log(
                crate::arch::cpu_id() as usize,
                3,
                task_ptr as u64,
                1,
            );
            grq.rt_rq.enqueue(task_ptr, false)
        }
        SchedPolicy::Deadline => {
            // Batch 8 (CBS): a throttled task whose deadline has not yet
            // arrived must NOT be re-enqueued (and must NOT be replenished —
            // the old unconditional replenish on every requeue is exactly
            // the defeated-CBS bug). Refuse; the replenish timer owns the
            // re-link. State stays SLEEPING — honest and still wakeable.
            let now = crate::sched::hotpath::sched_clock();
            let dl = task.dl_entity();
            let deadline = dl.deadline.load(core::sync::atomic::Ordering::Acquire);
            if dl.dl_throttled.load(core::sync::atomic::Ordering::Acquire) && now < deadline {
                arm_dl_replenish_timer(task_ptr);
                // R82 (silent-refusal attribution, the sc252/sc253
                // wake-leak hunt): this branch is the ONLY wake refusal
                // for a sleeping task that records nothing anywhere. If
                // the replenish timer keeps failing to re-link (timer
                // table full / timer lost), the task parks forever in
                // exactly the census shape (INTERRUPTIBLE ∧ unlinked ∧
                // slotless, "kicked Nx" forever). Rule 1 (grq-lock-1st):
                // the GRQ is held — record the event, the tick drain
                // prints it (a = delta to deadline in ms, b = timer id;
                // b == 0 with a live deadline is the LOST-timer shape).
                crate::sched_ev::sched_ev(
                    rux::api::sched::EV_DL_ENQ_DEFERRED,
                    task.pid(),
                    (deadline.saturating_sub(now)) / 1_000_000,
                    dl.replenish_timer.load(core::sync::atomic::Ordering::Acquire) as u64,
                );
                return false;
            }
            // Drop any stale timer so a late wake cannot double-enqueue.
            let stale = dl.replenish_timer.swap(0, core::sync::atomic::Ordering::AcqRel);
            if stale != 0 {
                crate::timer::del_timer(stale);
            }
            // Replenish ONLY when the current CBS period has elapsed (or on
            // the first-ever enqueue, where deadline is still 0). A wake
            // WITHIN the period keeps the residual budget — otherwise a task
            // that sleeps just before exhausting its budget would be refilled
            // on every wake, re-opening the 100%-CPU hole (review批次8).
            if now >= deadline {
                dl.update_deadline(now);
                dl.replenish_runtime();
            }
            grq.dl_rq.enqueue(task_ptr)
        }
        SchedPolicy::Normal | SchedPolicy::Batch => {
            // Scribble hunter: record the class insert (rux:scribble (retired debug=scribble)).
            #[cfg(feature = "x86_64")]
            crate::debug::scribble::ring_log(
                crate::arch::cpu_id() as usize,
                3,
                task_ptr as u64,
                0,
            );
            grq.cfs_rq.enqueue(task_ptr)
        }
        SchedPolicy::Idle => {
            // SCHED_IDLE uses CFS with low weight
            let se = task.sched_entity_mut();
            se.load = crate::sched::types::LoadWeight::new(crate::fair::WEIGHT_IDLEPRIO);
            grq.cfs_rq.enqueue(task_ptr)
        }
    };

    if inserted {
        // Linked (or verifiably already linked) — now make the state match
        // the queue membership (R39 ordering, see above).
        task.set_state(TaskState::new(TaskState::RUNNING));
        grq.nr_running.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // sched_update_tick_dependency (add_nr_running tail, sched.h):
        // every load increase re-evaluates the tick dependency — the
        // 0->1 transition kicks the stopped CPUs so they restart their
        // tick at their next exit.
        crate::timer::nohz::sched_update_tick_dependency_on_load_change();
        // ORPHAN-MARK creation tripwire (wake-fastrepro final-boss): the
        // task is being LINKED right now — under the GRQ, no pick or
        // switch can be in flight on it. If its ownership qword shows a
        // claim ({on_cpu=false, claim=X}) or a live pick-state, the marks
        // did NOT come from this task's own lifecycle in progress — they
        // arrived with the page (recycle residue) or survived a lost
        // release. ticks= names whether the task has EVER been scheduled
        // (0 = born with the marks: recycle residue; >0 = mid-life
        // contamination). Direct console (GRQ held), budgeted.
        {
            let oncpu = task.on_cpu();
            let claim = crate::sched::r#unsafe::task_shared(task)
                .running_on_cpu
                .load(core::sync::atomic::Ordering::Acquire);
            // Skip the legitimate windows: (a) __schedule re-linking a
            // preempted prev whose context is still being saved (the
            // SWITCHING_OUT marker names it — the R63 discipline), and
            // (b) the preempt-requeue window BEFORE that marker is set —
            // the requeued prev is still the claim CPU's published
            // current (slot-backed, the pick_owner_live discipline). A
            // link whose marks are backed by NEITHER is the orphan-mark
            // creation shape.
            let mid_swout = task_is_mid_switch_out(task_ptr);
            let slot_backed = claim >= 0
                && (claim as usize) < MAX_CPUS
                && cpu_state(claim as usize).current == task_ptr;
            // (ENQ-CLAIMED-SLOTBK print DELETED: it fired on EVERY
            // tick-preempt requeue of a CPU-bound task — the perf-sample
            // capture of the lmbench strand (2026-11-15) shows it is the
            // normal preempt-requeue window, present in completed boots
            // too, carrying no signal; ~1 ms of polled UART per tick on
            // the measured CPU was pure disturbance. The nohz round's
            // `rq_load() <= 1` slice guard already keeps the solo case
            // out of this path entirely.)
            if (oncpu || claim != -1) && !mid_swout && !slot_backed && task.pid() != 0 {
                static ENQC_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if ENQC_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn pdec2(mut v: u64) {
                        use crate::debug::taskdump::putc_brief as p;
                        let mut buf = [0u8; 20];
                        let mut n = 0;
                        if v == 0 {
                            p(b'0');
                            return;
                        }
                        while v > 0 && n < buf.len() {
                            buf[n] = b'0' + (v % 10) as u8;
                            v /= 10;
                            n += 1;
                        }
                        for i in (0..n).rev() {
                            p(buf[i]);
                        }
                    }
                    const M: &[u8] = b"\nENQ-CLAIMED pid=";
                    for &b in M {
                        putchar(b);
                    }
                    pdec2(task.pid() as u64);
                    const M2: &[u8] = b" oncpu=";
                    for &b in M2 {
                        putchar(b);
                    }
                    pdec2(oncpu as u64);
                    const M3: &[u8] = b" claim=";
                    for &b in M3 {
                        putchar(b);
                    }
                    pdec2(claim.unsigned_abs() as u64);
                    const M4: &[u8] = b" pk=";
                    for &b in M4 {
                        putchar(b);
                    }
                    pdec2(task.pick_state() as u64);
                    const M5: &[u8] = b" ticks=";
                    for &b in M5 {
                        putchar(b);
                    }
                    pdec2(
                        task.utime_ticks.load(core::sync::atomic::Ordering::Relaxed)
                            + crate::sched::r#unsafe::task_shared(task)
                                .stime_ticks
                                .load(core::sync::atomic::Ordering::Relaxed),
                    );
                    const M6: &[u8] = b" st=0x";
                    for &b in M6 {
                        putchar(b);
                    }
                    pdec2(task.state().bits() as u64);
                    putchar(b'\n');
                }
            }
        }
        // Wakewatch ring (118th): the LINK edge — pairs with W so the dump
        // distinguishes "wake verdict ok but never linked" from
        // "linked then unlinked before running".
        ww_record(task_ptr, WW_EV_LINK);
        // Wakewatch forensics: stamp every successful link (wake, fork,
        // heal re-insert) so the pick side can measure wake→run latency.
        crate::wakewatch::note_wake(task_ptr);
        // 63rd: persistent per-task history (strand-wall hunt).
        if sched_hist_armed() {
            sched_hist_link(task_ptr);
        }
        // Per-task schedstat (T7): open the wait window — the pick edge
        // in __schedule closes it into schedstat_wait_ns. GRQ is held
        // here and there, so the stamp needs no lock of its own.
        if crate::sched::hotpath::enabled() && task.pid() != 0 {
            crate::sched::r#unsafe::task_shared(task)
                .schedstat_wait_start
                .store(crate::sched::hotpath::sched_clock(), core::sync::atomic::Ordering::Relaxed);
        }
    }
    inserted
}


/// R49 (on_cpu clear-protocol gap — the seed): steer `task`'s ti_cpu to
/// `target` only while it is verifiably NOT on a CPU. ti_cpu is the CPU
/// identity source — cpu_id() reads tp->ti_cpu (smp.rs) and trap.S picks
/// the per-CPU interrupt stack the same way — so a steer store that lands
/// while the task is picked/executing detaches the hardware CPU from its
/// PER_CPU slot: the next __schedule there resolves `prev` from the WRONG
/// slot, __switch_to saves/clears the wrong task, and mark_picked_on_cpu
/// marks accumulate with no owning switch — the permanent
/// RUNNING ∧ on_cpu=1 ∧ unlinked phantoms (round-49 capture: pid
/// 344/345/356, all four CPUs wfi-idle).
///
/// The two pre-R49 call sites gated the steer on an UNLOCKED on_cpu read
/// (check-then-write TOCTOU): the gate can pass on CPU B, then another
/// CPU's racing wake links the task and a third CPU picks it
/// (mark_picked_on_cpu under the GRQ lock) and stamps ti_cpu in
/// sched::context_switch — and CPU B's delayed steer store STILL lands,
/// while the task executes. Under the GRQ lock the re-check is atomic
/// with every pick (picks hold this lock), and any later switch-in
/// re-stamps ti_cpu itself (sched::context_switch line-order: stamp
/// before __switch_to), so a steer admitted here can never race a mark.
pub fn steer_task_cpu(task: *mut Task, target: i32) {
    if task.is_null() {
        return;
    }
    let _g = grq().lock_irqsave();
    // SAFETY: task is non-null and validated by the caller; holding the
    // GRQ lock excludes concurrent picks, so on_cpu==false here means no
    // CPU owns the task in the mark_picked_on_cpu → __switch_to window.
    {
        if !crate::sched::r#unsafe::task_shared(task).on_cpu() {
            // The sched:sched_migrate_task tracepoint — the reference
            // records it in set_task_cpu, just before the stamp moves.
            // Marshals only behind armed().
            if crate::trace::EVENT_SCHED_MIGRATE_TASK.armed() {
                crate::trace::trace_sched_migrate_task(
                    &*crate::sched::r#unsafe::task_shared(task).comm(),
                    crate::sched::r#unsafe::task_shared(task).pid() as u32,
                    crate::sched::r#unsafe::task_shared(task).prio(),
                    crate::sched::r#unsafe::task_shared(task).ti_cpu(),
                    target,
                );
            }
            crate::sched::r#unsafe::task_shared(task).set_ti_cpu(target);
        }
    }
}


/// Wakewatch ring (rux:schedtrace (retired debug=schedtrace)): the last WW_RING scheduler
/// lifecycle events system-wide, each stamped with jiffies. The
/// binder-stall WAKE-RUN-LEAK census prints a victim's tail of the
/// ring, so a "kick claimed success yet the task never ran" capture
/// names WHICH stage (wake-ok / link / unlink-pick / unlink-sched /
/// unlink-deq / pick-publish / affirm) last touched the task and WHEN —
/// the offline answer to "who unlinked a never-running task". Silent
/// (one index swap + two relaxed stores) unless schedtrace is armed;
/// every event is recorded from GRQ-held context, so the ring's
/// ordering is the lock's ordering.
pub const WW_RING: usize = 1024;










static WW_TASK: [core::sync::atomic::AtomicUsize; WW_RING] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; WW_RING];

static WW_DATA: [core::sync::atomic::AtomicU64; WW_RING] =
    [const { core::sync::atomic::AtomicU64::new(0) }; WW_RING];

/// Caller attribution per ring slot (118th unlink-without-run hunt): the
/// SOURCE LINE of the event site, captured via #[track_caller] on
/// `ww_record` (the debug build outlines it, so an asm ra capture lands
/// inside ww_record itself — Location::caller() is exact regardless of
/// inlining). All event sites live in sched.rs except WW_EV_AFFIRM
/// (task.rs); the dump token reads `P@12j:L2990` = "unlinked at pick 12
/// jiffies ago at sched.rs:2990".
static WW_RA: [core::sync::atomic::AtomicUsize; WW_RING] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; WW_RING];

static WW_IX: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// 118th: PER-WATCHED-TASK mini-rings. The global 1024-slot ring rolls in
/// SECONDS once 64 tasks are watched (timer-class watchers emit
/// wake/park/pick/affirm pairs at jiffy rates), so a victim's stranding
/// event is overwritten before the next binder census reads it — the
/// boot-3 capture showed only single-jiffy probe bursts (many victims × 5
/// events in one instant) and never the quiet stranding unlinks. Each
/// watch slot now carries its own 8-event tail: written under the same
/// GRQ hold as the global ring, immune to other tasks' churn.
const WW_TAIL: usize = 8;

static WW_TAIL_IX: [core::sync::atomic::AtomicUsize; 64] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 64];

static WW_TAIL_DATA: [core::sync::atomic::AtomicU64; 64 * WW_TAIL] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 64 * WW_TAIL];

static WW_TAIL_LINE: [core::sync::atomic::AtomicUsize; 64 * WW_TAIL] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 64 * WW_TAIL];

/// Census-populated watch list: once the binder-stall scan flags a
/// WAKE-RUN-LEAK victim, its Task pointer lands here and ww_record
/// retains its events — otherwise the system's timer churn (VSync
/// 10 ms wait/wake pairs alone) rolls a 1024-entry global ring in
/// seconds and the victim's unlink event is gone before the next
/// census reads it.
pub static WW_WATCH: [core::sync::atomic::AtomicUsize; 64] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 64];

/// Census side: add a victim to the watch list (idempotent).
pub fn ww_watch_add(task: *mut Task) {    if task.is_null() {
        return;
    }
    let t = task as usize;
    for w in WW_WATCH.iter() {
        let cur = w.load(core::sync::atomic::Ordering::Relaxed);
        if cur == t {
            return;
        }
        if cur == 0
            && w.compare_exchange(
                0,
                t,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            return;
        }
    }
}

/// 119th conviction: raw fill count of watch slot `ix`'s per-task mini-ring
/// (the monotonic event counter, saturating at u64 — the dump prints it so
/// "watched but zero events" is provable). 0 = the slot recorded nothing.
pub fn ww_tail_fill(ix: usize) -> u64 {
    if ix >= WW_TAIL_IX.len() {
        return 0;
    }
    WW_TAIL_IX[ix].load(core::sync::atomic::Ordering::Relaxed) as u64
}

/// Capture the calling event site's source line (wakewatch caller
/// attribution, 118th unlink-without-run hunt) — thin wrapper so each
/// event token in the dump names its exact origin line.
#[inline]
#[track_caller]
pub fn ww_record(task: *mut Task, code: u32) {
    if !crate::trace::EVENT_SCHEDTRACE.armed() {
        return;
    }
    // Watch-list filter: a 64-way cached-atomic scan per event is
    // nothing next to the GRQ the caller already holds.
    let t = task as usize;
    let mut watch_ix: Option<usize> = None;
    for (i, w) in WW_WATCH.iter().enumerate() {
        let cur = w.load(core::sync::atomic::Ordering::Relaxed);
        if cur == t {
            watch_ix = Some(i);
            break;
        }
    }
    let watch_ix = match watch_ix {
        Some(i) => i,
        None => return,
    };
    let loc = core::panic::Location::caller();
    let j = crate::timer::get_jiffies();
    // Global ring (context; rolls fast under churn).
    let ix = WW_IX.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % WW_RING;
    WW_TASK[ix].store(t, core::sync::atomic::Ordering::Relaxed);
    WW_DATA[ix].store(
        ((code as u64) << 56) | (j & 0x00FF_FFFF_FFFF_FFFF),
        core::sync::atomic::Ordering::Relaxed,
    );
    WW_RA[ix].store(loc.line() as usize, core::sync::atomic::Ordering::Relaxed);
    // Per-task mini-ring (the capture that survives churn): the recording
    // context holds the GRQ, and each task's tail index advances only from
    // its own events, so the 8 newest events for THIS task are always
    // complete regardless of other watchers' volume.
    let tix = WW_TAIL_IX[watch_ix].fetch_add(1, core::sync::atomic::Ordering::Relaxed) % WW_TAIL;
    WW_TAIL_DATA[watch_ix * WW_TAIL + tix].store(
        ((code as u64) << 56) | (j & 0x00FF_FFFF_FFFF_FFFF),
        core::sync::atomic::Ordering::Relaxed,
    );
    WW_TAIL_LINE[watch_ix * WW_TAIL + tix].store(loc.line() as usize, core::sync::atomic::Ordering::Relaxed);
    // 118th pipeline bisect: a watched task's WAKE_OK must be observable
    // in the serial stream (rate-limited) — if the re-kicks of stranded
    // victims never produce a WWTICK line, the funnel's W record never
    // ran for them and their kicks' true-return came from a different
    // leg. Direct console (GRQ held).
    if code == WW_EV_WAKE_OK {
        static WWT_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        if WWT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
            use crate::debug::taskdump::putc_brief as putchar;
            const M: &[u8] = b"\nWWTICK pid=";
            for &b in M {
                putchar(b);
            }
            let v = crate::sched::r#unsafe::task_shared(task).pid() as u64;
            let mut sh = 64;
            while sh > 0 {
                sh -= 4;
                let nb = ((v >> sh) & 0xF) as u8;
                putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
            }
            const M2: &[u8] = b" jif=";
            for &b in M2 {
                putchar(b);
            }
            let mut v = j;
            let mut buf = [0u8; 20];
            let mut n = 0;
            while v > 0 {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            if n == 0 {
                putchar(b'0');
            }
            while n > 0 {
                n -= 1;
                putchar(buf[n]);
            }
            putchar(b'\n');
        }
    }
}

/// Append `task`'s last few ring events (oldest first) to `out` as one
/// line of `code@jiffies-ago` tokens. Codes: W=wake_ok L=link
/// P=unlink_pick S=unlink_sched D=unlink_deq X=unlink_poison
/// K=pick_publish R=wake_refused A=affirm. Reader-side only; called
/// from the binder-stall census (no GRQ held — the ring is atomics).
pub fn ww_append_dump(out: &mut dyn core::fmt::Write, task: *mut Task, now: u64) {
    if !crate::trace::EVENT_SCHEDTRACE.armed() {
        return;
    }
    const TAGS: [(u32, char); 9] = [
        (WW_EV_WAKE_OK, 'W'),
        (WW_EV_LINK, 'L'),
        (WW_EV_UNLINK_PICK, 'P'),
        (WW_EV_UNLINK_SCHED, 'S'),
        (WW_EV_UNLINK_DEQ, 'D'),
        (WW_EV_UNLINK_POISON, 'X'),
        (WW_EV_PICK_PUB, 'K'),
        (WW_EV_WAKE_REFUSED, 'R'),
        (WW_EV_AFFIRM, 'A'),
    ];
    // 118th: read the PER-TASK mini-ring (survives global churn — see
    // WW_TAIL_DATA). Fall back to the global ring walk when the task is
    // not in the watch list (should not happen: census adds before dump).
    let t = task as usize;
    let mut watch_ix: Option<usize> = None;
    for (i, w) in WW_WATCH.iter().enumerate() {
        if w.load(core::sync::atomic::Ordering::Relaxed) == t {
            watch_ix = Some(i);
            break;
        }
    }
    if let Some(wi) = watch_ix {
        let n0 = WW_TAIL_IX[wi].load(core::sync::atomic::Ordering::Relaxed);
        let filled = WW_TAIL.min(n0);
        let _ = write!(out, "binder-stall:  ww=[");
        let mut printed = 0usize;
        for k in 0..filled {
            // oldest-first: the mini-ring wraps mod 8; slot order for the
            // `filled` valid entries is (n0 - filled + k) % 8.
            let slot = (n0.wrapping_sub(filled) + k) % WW_TAIL;
            let d = WW_TAIL_DATA[wi * WW_TAIL + slot].load(core::sync::atomic::Ordering::Relaxed);
            let line = WW_TAIL_LINE[wi * WW_TAIL + slot].load(core::sync::atomic::Ordering::Relaxed);
            let code = (d >> 56) as u32;
            let jif = d & 0x00FF_FFFF_FFFF_FFFF;
            let tag = TAGS
                .iter()
                .find(|(c, _)| *c == code)
                .map(|(_, t)| *t)
                .unwrap_or('?');
            if printed != 0 {
                let _ = write!(out, " ");
            }
            let _ = write!(out, "{}@{}j:L{}", tag, now.saturating_sub(jif), line);
            printed += 1;
        }
        let _ = writeln!(out, "]");
        return;
    }
    // Global-ring fallback (task not on the watch list).
    let ix0 = WW_IX.load(core::sync::atomic::Ordering::Relaxed);
    // Collect the NEWEST events for this task by walking the ring back.
    let mut events: [(char, u64, usize); 8] = [('-', 0, 0); 8];
    let mut n = 0usize;
    for k in 0..WW_RING.min(ix0) {
        let slot = (ix0.wrapping_sub(1 + k)) % WW_RING;
        if WW_TASK[slot].load(core::sync::atomic::Ordering::Relaxed) != task as usize {
            continue;
        }
        let d = WW_DATA[slot].load(core::sync::atomic::Ordering::Relaxed);
        let code = (d >> 56) as u32;
        let jif = d & 0x00FF_FFFF_FFFF_FFFF;
        let ra = WW_RA[slot].load(core::sync::atomic::Ordering::Relaxed);
        let tag = TAGS
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, t)| *t)
            .unwrap_or('?');
        events[n] = (tag, now.saturating_sub(jif), ra);
        n += 1;
        if n == 8 {
            break;
        }
    }
    // events[0..n) is newest-first; print oldest-first for readability.
    // 118th: each token carries the event site's source line (WW_RA,
    // via #[track_caller]) — `S@3j:L2990` reads "unlinked-by-schedule
    // 3 jiffies ago at sched.rs:2990" (A tokens are task.rs lines).
    let _ = write!(out, "binder-stall:  ww=[");
    for i in (0..n).rev() {
        let (t, ago, line) = events[i];
        if i != n - 1 {
            let _ = write!(out, " ");
        }
        let _ = write!(out, "{}@{}j:L{}", t, ago, line);
    }
    let _ = writeln!(out, "]");
}


/// Snapshot the refusing guard's view of `task` (used by task.rs's W2
/// report). Takes the GRQ lock briefly, mirroring wake_up_enqueue_opt's
/// own acquisition (same context, same IRQ discipline).
pub fn w2_refusal_census(task: *mut Task) -> W2RefusalCensus {
    let mut out = W2RefusalCensus {
        linked: false,
        on_rq: false,
        busy_slot: -1,
        idle_slot: -1,
        exec_ns: 0,
        jiffies: crate::timer::get_jiffies(),
        quantum_ms: 0,
    };
    if task.is_null() {
        return out;
    }
    let grq_guard = grq().lock_irqsave();
    {
        out.linked = match crate::sched::r#unsafe::task_shared(task).policy() {
            SchedPolicy::Fifo | SchedPolicy::Rr => grq_guard.rt_rq.is_linked(task),
            SchedPolicy::Deadline => grq_guard.dl_rq.is_linked(task),
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                grq_guard.cfs_rq.is_linked(task)
            }
        };
        let se = crate::sched::r#unsafe::task_shared(task).sched_entity();
        out.on_rq = se.on_rq.load(core::sync::atomic::Ordering::Acquire);
        out.exec_ns = se
            .sum_exec_runtime
            .load(core::sync::atomic::Ordering::Acquire);
        out.quantum_ms = crate::sched::r#unsafe::task_shared(task).get_time_slice();
        let idle_mask = grq().idle_cpus.load(core::sync::atomic::Ordering::Acquire);
        for c in 0..MAX_CPUS {
            if cpu_state(c).current == task {
                if idle_mask & (1u32 << c) != 0 {
                    out.idle_slot = c as i32;
                } else {
                    out.busy_slot = c as i32;
                }
            }
        }
    }
    out
}


/// Per-CPU count of __schedule() entries (runq-strand lane): the
/// monopolist detector's truth source. A CPU executing a preemptible
/// workload calls schedule() hundreds of times per 5 s burst; a CPU
/// held by an unpreemptible kernel loop (or a task whose preemption
/// requests are all refused) stops calling it entirely. One relaxed
/// fetch_add per schedule — same cost class as the cctx probe's
/// existing per-schedule counters.
// SCHED_CALLS moved FRAMEWORK-side (devirt lane; sched/entities.rs).

/// MONOPOLIST census (runq-strand lane, 121st): the RUNQ-STRAND census
/// names the victims; this names their CULPRITS. The cotask-decode
/// campaign's frozen-skipall evidence (walks stop completing while 50+
/// entries stay linked and every CPU reads busy with FIXED pids for
/// minutes) says the strand is produced by CPUs that never reach a
/// schedule() again — incumbents monopolizing their harts. A CPU is a
/// MONOPOLIST when the SAME real task holds its slot across a full
/// 5 s burst while SCHED_CALLS for that CPU advances by < 20 (a
/// preemptible holder re-picks itself through the walk hundreds of
/// times per burst; a frozen counter means schedule() stopped running
/// there). The row carries identity, preempt_count, need_resched, exec
/// activation, and the last timer-tick stamp of WHERE the CPU was
/// interrupted (TICK_ENTRY_RIP + CS ring bits: kernel vs user mode +
/// stamp age). A kernel-mode RIP constant across bursts names the
/// unpreemptible spin loop (symbolize offline); a user-mode RIP with a
/// live need_resched names a broken user-return preempt gate; a stale
/// stamp names a halted/tick-starved CPU. Runs under rux:fuxstuck (retired debug=fuxstuck) (the
/// futex census burst calls it, right after runq_strand_census); the
/// stamps themselves ride the tick (TICK_ENTRY_* are always on, same
/// as leg-33B). Prints only when at least one monopolist exists —
/// quiet boots pay nothing.
pub fn monopolist_census(now_j: u64) {
    use core::sync::atomic::Ordering::{Acquire, Relaxed};
    use core::fmt::Write as _;

    static LINES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    struct Prev {
        pid: core::sync::atomic::AtomicU32,
        sched: core::sync::atomic::AtomicU64,
    }
    static PREV: [Prev; MAX_CPUS] = {
        #[allow(clippy::declare_interior_mutable_const)]
        const ZERO: Prev = Prev {
            pid: core::sync::atomic::AtomicU32::new(0),
            sched: core::sync::atomic::AtomicU64::new(0),
        };
        [ZERO; MAX_CPUS]
    };

    let mut any = false;
    let mut body = alloc::string::String::new();
    for c in 0..MAX_CPUS {
        let cur = cpu_state(c).current;
        let pid = if cur.is_null() || cur == cpu_state(c).idle {
            0
        } else {
            // SAFETY: per-CPU current is a valid task or idle (idle
            // excluded above); diagnostic reads only.
            let cur = crate::sched::r#unsafe::task_shared(cur);
            if cur.pid() == 0 {
                0
            } else {
                cur.pid()
            }
        };
        let sched_now = SCHED_CALLS[c].load(Relaxed);
        let prev_pid = PREV[c].pid.swap(pid, Relaxed);
        let prev_sched = PREV[c].sched.swap(sched_now, Relaxed);
        // Skip the first burst for a holder (no delta yet).
        let held_over_burst = pid != 0 && prev_pid == pid;
        let sched_dx = sched_now.saturating_sub(prev_sched);
        if !held_over_burst || sched_dx >= 20 {
            continue;
        }
        // SAFETY: revalidated above (non-null, non-idle, pid != 0).
        let cur = crate::sched::r#unsafe::task_shared(cpu_state(c).current);
        let se = cur.sched_entity();
        let act_ms = se
            .sum_exec_runtime
            .load(Relaxed)
            .saturating_sub(se.prev_sum_exec_runtime.load(Relaxed))
            / 1_000_000;
        any = true;
        if LINES.load(Relaxed) > 400 {
            continue;
        }
        let trip = crate::interrupt::preempt::TICK_ENTRY_RIP[c].load(Relaxed);
        let tcs = crate::interrupt::preempt::TICK_ENTRY_CS[c].load(Relaxed);
        let tpid = crate::interrupt::preempt::TICK_ENTRY_PID[c].load(Relaxed);
        let tjif = crate::interrupt::preempt::TICK_ENTRY_JIF[c].load(Relaxed);
        // SAFETY: NEED_RESCHED is an AtomicBool array sized MAX_CPUS;
        // static mut access is single-word atomic reads here.
        let nrs = NEED_RESCHED[c].load(Acquire) as u8;
        let mode = if tcs & 3 == 3 { 'U' } else { 'K' };
        let stamp_age = now_j.saturating_sub(tjif);
        let _ = write!(
            body,
            " |c{}={}({}) sdx={} act={}ms pc={:#x} nrs={} rip={:#x}{}jif={} spid={}",
            c,
            cur.pid(),
            core::str::from_utf8(cur.comm()).unwrap_or("").trim_end_matches('\0'),
            sched_dx,
            act_ms,
            cur.preempt_count(),
            nrs,
            trip,
            mode,
            stamp_age,
            tpid,
        );
        // Kernel-mode holder: capture the live call chain — scan down
        // from below the entry frame, keeping kernel-text words (the
        // same contract TXN-HANG-STK's top-dn walk uses; for an ON-CPU
        // task the frames under the entry frame are the current
        // syscall's chain, symbolized offline with addr2line).
        #[cfg(feature = "x86_64")]
        if mode == 'K' {
            if let Some(top) = cur.kernel_stack_top_checked() {
                let base = top.wrapping_sub(
                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                );
                let mut stk = alloc::string::String::new();
                let mut kept = 0usize;
                // SAFETY: entry-frame read of this task's pinned
                // kernel-stack pages (kernel_stack_top_checked
                // contract); diagnostic reads only.
                {
                    let mut p = (base as *const u64).wrapping_sub(1);
                    for _ in 0..64 {
                        if kept >= 14 {
                            break;
                        }
                        let v = crate::sync::r#unsafe::read_volatile_u64(p as usize);
                        if (0xffffffff8000_0000..0xffffffff8200_0000).contains(&v) {
                            let _ = write!(stk, " {v:#x}");
                            kept += 1;
                        }
                        p = p.wrapping_sub(1);
                    }
                }
                if kept > 0 {
                    let _ = write!(body, "\nMONOPOLIST-STK c={} pid={}{}", c, cur.pid(), stk);
                }
            }
        }
    }
    if !any || LINES.load(Relaxed) > 400 {
        return;
    }
    LINES.fetch_add(1, Relaxed);
    // HOTPATH-PRINT: fuxstuck graph-burst gate.
    crate::pr_warn!(
        "MONOPOLIST nrrun={} j={}{}",
        GlobalRunQueue::grq_nr_running(),
        now_j,
        body
    );
}

/// RUNQ-STRAND census (59th session, ohc74 close() strand): scan every
/// burst for tasks in the PICKABLE-by-ownership shape — on_rq, state
/// RUNNING, claim == -1, on_cpu == false, no CPU's current slot — whose
/// exec clock has been FROZEN since a previous burst. The existing heal
/// machinery (R85 adjudication, stale-claim/stale-oncpu resets) only
/// covers ownership-shaped strands; ohc74's terminal state had TWO samgr
/// threads (main + OS_IPC_0, both mid-`close(16)`) linked, pickable,
/// and unexecuted for 290 s — a refusal class the scheduler never names
/// (affinity? cgroup throttle? a pick-path miss). This census prints the
/// candidate with every refusal input the pick path consults, plus the
/// incumbent per CPU (with activation length + preempt_count) so a
/// preempt-disabled monopolist is named in the same line. Runs under
/// rux:fuxstuck (retired debug=fuxstuck) (the futex census burst calls it); zero cost otherwise.
pub fn runq_strand_census(now_j: u64) {
    use core::sync::atomic::Ordering::{Acquire, Release};

    const SLOTS: usize = 16;
    struct Slot {
        task: core::sync::atomic::AtomicUsize,
        exec_ns: core::sync::atomic::AtomicU64,
        first_j: core::sync::atomic::AtomicU64,
    }
    static TABLE: [Slot; SLOTS] = [const {
        Slot {
            task: core::sync::atomic::AtomicUsize::new(0),
            exec_ns: core::sync::atomic::AtomicU64::new(0),
            first_j: core::sync::atomic::AtomicU64::new(0),
        }
    }; SLOTS];
    static LINES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    // Report only once the exec clock has been frozen for >= 15 s of
    // burst time (3 bursts) — transient mid-switch-out windows must not
    // print. Whole-boot budget keeps a storm bounded.
    const MIN_AGE_J: u64 = 15 * 100;

    // Phase A: snapshot every pickable-shaped runnable task.
    let mut cands: [(usize, u64); SLOTS] = [(0, 0); SLOTS];
    let mut nc = 0usize;
    crate::sched::hotpath::for_each_task_try(|task| {
        if nc >= SLOTS {
            return;
        }
        // Diagnostic reads under the for_each_task contract (typed).
        if !task.sched_entity().is_on_rq() {
            return;
        }
        if task.state().bits() != crate::process::task::TaskState::RUNNING {
            return;
        }
        if task.on_cpu() {
            return;
        }
        if task.running_on_cpu.load(Acquire) != -1 {
            return;
        }
        let mut is_current = false;
        let task_id = task as *const Task as usize;
        for c in 0..MAX_CPUS {
            if cpu_state(c).current as usize == task_id {
                is_current = true;
                break;
            }
        }
        if is_current {
            return;
        }
        let exec = task
            .sched_entity()
            .sum_exec_runtime
            .load(core::sync::atomic::Ordering::Relaxed);
        cands[nc] = (task_id, exec);
        nc += 1;
    });
    // Mark slots not seen this burst dead so a recycled pointer cannot
    // inherit the age (task pages are recycled).
    for s in TABLE.iter() {
        let t = s.task.load(Acquire);
        if t != 0 && !cands.iter().take(nc).any(|&(c, _)| c == t) {
            s.task.store(0, Release);
        }
    }
    // Phase B: age the survivors; print the frozen ones with reasons.
    let mut printed = 0u32;
    for &(task, exec) in cands.iter().take(nc) {
        // First observation: claim a free slot (or steal the oldest).
        let mut slot: Option<&Slot> = None;
        let mut oldest: Option<(usize, u64)> = None;
        for (i, s) in TABLE.iter().enumerate() {
            let t = s.task.load(Acquire);
            if t == task {
                slot = Some(s);
                break;
            }
            if t == 0 {
                if slot.is_none() {
                    slot = Some(s);
                }
                continue;
            }
            let fj = s.first_j.load(Acquire);
            if oldest.map_or(true, |(_, ofj)| fj < ofj) {
                oldest = Some((i, fj));
            }
        }
        let slot = match slot {
            Some(s) => s,
            None => match oldest {
                Some((i, _)) => &TABLE[i],
                None => continue,
            },
        };
        let prev_exec = slot.exec_ns.load(Acquire);
        if slot.task.load(Acquire) != task || prev_exec != exec {
            slot.task.store(task, Release);
            slot.exec_ns.store(exec, Release);
            slot.first_j.store(now_j, Release);
            continue;
        }
        let first_j = slot.first_j.load(Acquire);
        let age = now_j.saturating_sub(first_j);
        if age < MIN_AGE_J {
            continue;
        }
        if LINES.load(core::sync::atomic::Ordering::Relaxed) > 400 || printed >= 4 {
            continue;
        }
        LINES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        printed += 1;
        // SAFETY: task was validated by phase A this burst.
        let tk = crate::sched::r#unsafe::task_shared(task as *mut Task);
        let linked = match grq_diag_cfs_linked(task as *mut crate::process::task::Task) {
            Some(v) => v as u8,
            None => 2,
        };
        let throttled = crate::sched::hotpath::task_cgroup_throttled(task as *const Task);
        let vr = tk.sched_entity().get_vruntime();
        // 63rd: the pick-path's decision-time history, now persistent on
        // the task (never refused = pick_visit_n 0; the old peek's None
        // also covered slot-stolen ambiguity which no longer exists).
        let skips: u64 = tk.pick_visit_n.load(core::sync::atomic::Ordering::Relaxed);
        {
            use core::sync::atomic::Ordering::Relaxed as Rlx;
            let pk_n = tk.pick_n.load(Rlx);
            let wk_n = tk.wake_n.load(Rlx);
            let last_pick = tk.pick_last_jif.load(Rlx);
            let last_wake = tk.wake_last_jif.load(Rlx);
            let first_ref = tk.pick_first_refuse_jif.load(Rlx);
            let w0 = tk.pick_refuse_why[0].load(Rlx);
            let w1 = tk.pick_refuse_why[1].load(Rlx);
            let w2 = tk.pick_refuse_why[2].load(Rlx);
            let w3 = tk.pick_refuse_why[3].load(Rlx);
            let w4 = tk.pick_refuse_why[4].load(Rlx);
            let lp_ms = if last_pick == 0 {
                u64::MAX
            } else {
                now_j.saturating_sub(last_pick)
            };
            let lw_ms = if last_wake == 0 { u64::MAX } else { now_j.saturating_sub(last_wake) };
            let wait_last_us = tk.wait_last_ns.load(Rlx) / 1000;
            let wait_max_us = tk.wait_max_ns.load(Rlx) / 1000;
            let wait_open = tk.wait_open_ns.load(Rlx) != 0;
            let fr_ms = if first_ref == 0 { u64::MAX } else { now_j.saturating_sub(first_ref) };
            // HOTPATH-PRINT: fuxstuck graph-burst gate.
            crate::pr_warn!(
                "STRAND-HIST tid={} pk={} wk={} visits={} why1/2/3/4/5={}/{}/{}/{}/{} firstref={}j lp={}j lw={}j wait_last={}us wait_max={}us wopen={} exec={}",
                tk.pid(),
                pk_n,
                wk_n,
                skips,
                w0, w1, w2, w3, w4,
                fr_ms,
                lp_ms,
                lw_ms,
                wait_last_us,
                wait_max_us,
                wait_open as u8,
                exec / 1_000_000
            );
            // 63rd: the victim's persistent event ring, chronological
            // (oldest first) — kind: L=link R=refuse(why) P=pick
            // H4=claim-heal H5=oncpu-heal; jiffies since boot; aux hex.
            let seq = tk.pick_ring_seq.load(Rlx);
            if seq > 0 {
                let mut ring_s = alloc::string::String::new();
                use core::fmt::Write as _;
                let n = (seq as usize).min(16);
                for e in 0..n {
                    let idx = (seq as usize - n + e) & 15;
                    let v = tk.pick_ring[idx].load(Rlx);
                    if v == 0 && e + 1 < n {
                        continue;
                    }
                    let kind = v >> 60;
                    let why = (v >> 52) & 0xFF;
                    let jif = (v >> 22) & 0x3FFF_FFFF;
                    let aux = v & 0x3F_FFFF;
                    let k = match kind {
                        1 => "L",
                        2 => "R",
                        3 => "P",
                        4 => "H4",
                        5 => "H5",
                        _ => "?",
                    };
                    let _ = write!(ring_s, " {}{}.{:#x}@{}", k, if kind == 2 { why } else { 0 }, aux, jif);
                }
                // HOTPATH-PRINT: fuxstuck graph-burst gate.
                crate::pr_warn!("VITCHRON tid={} n={}{}", tk.pid(), seq, ring_s);
            }
        }
        // 60th: the two pick-walk give-up shapes (stash overflow /
        // skip-all), plus the WALL below this victim: entries every
        // pick consults first. Held under the same GRQ window.
        let (ovf, skipall) = crate::fair::pick_giveups();
        let mut wall_s = alloc::string::String::new();
        let mut inc_s = alloc::string::String::new();
        let mut min_vr: u64 = 0;
        {
            let mut g = match try_grq_lock() {
                Some(g) => g,
                None => {
                    use core::fmt::Write as _;
                    // BUSY: report the give-up counters (lock-free) and
                    // name the window busy; skip the tree walk.
                    use core::fmt::Write as _;
                    let _ = write!(
                        wall_s,
                        " grq=busy ovf={} skipall={}",
                        ovf, skipall
                    );
                    // HOTPATH-PRINT: fuxstuck graph-burst gate.
                    crate::pr_warn!(
                        "RUNQ-STRAND tid={} tgid={} ({}) age={}s exec_frozen={}ms linked={} aff={:#x} throttled={} pol={:?} claim={} pk={:#x} ts={}ms skips={} vr={:#x} nrrun={} min_vr=?{}{}",
                        tk.pid(),
                        tk.tgid(),
                        core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0'),
                        age / 100,
                        exec / 1_000_000,
                        linked,
                        tk.cpus_allowed(),
                        throttled as u8,
                        tk.policy(),
                        tk.running_on_cpu.load(core::sync::atomic::Ordering::Acquire),
                        tk.pick_state(),
                        tk.get_time_slice() * 1000 / crate::config::KERNEL_HZ as u32,
                        skips,
                        vr,
                        GlobalRunQueue::grq_nr_running(),
                        wall_s,
                        inc_s
                    );
                    continue;
                }
            };
            min_vr = g.cfs_rq.get_min_vruntime();
            // The wall: up to 8 entries sorting before the victim.
            {
                use core::fmt::Write as _;
                let mut rows: [crate::sched::types::StrandWallRow; 8] = [
                    crate::sched::types::StrandWallRow {
                        vruntime: 0,
                        pid: 0,
                        policy: 0,
                        on_cpu: false,
                        claim: 0,
                        aff: 0,
                        skips: 0,
                        why: 0,
                        slot: -1,
                        lp_jif: 0,
                        wakes: 0,
                    };
                    8
                ];
                let mut st_hist = [0usize; 3];
                let (below, total, filled) =
                    g.cfs_rq.strand_wall_probe(task as *mut Task, &mut rows, &mut st_hist);
                let _ = write!(
                    wall_s,
                    " ovf={} skipall={} below={} total={} wallst=run{}/slp{}/oth{}",
                    ovf, skipall, below, total, st_hist[0], st_hist[1], st_hist[2]
                );
                // 61st: the wall rows go on their OWN line — the main
                // strand line blows the printk record cap at the first
                // row (ohw12/15 kept only |858 / |321) and the wall
                // identities (who is stuck on_cpu at pick time) are the
                // refusal input the census exists to name.
                let mut wr = alloc::string::String::new();
                for r in rows.iter().take(filled) {
                    // 63rd: sl = owning CPU slot (-1 none), lp = jiffies
                    // since the entry's last pick (X = never picked), w =
                    // wake count — sl>=0 on a linked entry is the R56d
                    // lying-slot / mid-switch-out shape at census time.
                    let lp = if r.lp_jif == u32::MAX {
                        alloc::string::String::from("X")
                    } else {
                        alloc::format!("{}", r.lp_jif)
                    };
                    let _ = write!(
                        wr,
                        " |{}p{}vr{:#x}oc{}cl{}af{:#x}sk{}wh{}sl{}lp{}w{}",
                        r.pid, r.policy, r.vruntime, r.on_cpu as u8, r.claim, r.aff, r.skips, r.why, r.slot, lp, r.wakes
                    );
                }
                if below > filled {
                    let _ = write!(wr, " |+{}", below - filled);
                }
                if !wr.is_empty() {
                    // HOTPATH-PRINT: fuxstuck graph-burst gate.
                    crate::pr_warn!(
                        "STRAND-WALL vid={} tid={} {}",
                        tk.pid(),
                        tk.pid(),
                        wr
                    );
                }
                let _ = wall_s;
            }
            {
                use core::fmt::Write as _;
            for c in 0..MAX_CPUS {
                let cur = cpu_state(c).current;
                if cur.is_null() {
                    let _ = write!(inc_s, " c{}=nil", c);
                    continue;
                }
                // SAFETY: per-CPU current is a valid task or idle.
                let cur = crate::sched::r#unsafe::task_shared(cur);
                let se = cur.sched_entity();
                let act_ms = se
                    .sum_exec_runtime
                    .load(core::sync::atomic::Ordering::Relaxed)
                    .saturating_sub(se.prev_sum_exec_runtime.load(core::sync::atomic::Ordering::Relaxed))
                    / 1_000_000;
                // runq-strand lane: COMPACT form — the old long form
                // (comm + act + pc + pol + prio) blew the printk record
                // cap after c0, so 7 of 8 incumbents were NEVER visible
                // on the strand rows. pid/pc/act identifies each CPU's
                // holder; comm/policy ride the MONOPOLIST line.
                let _ = write!(
                    inc_s,
                    " c{}={}/pc{:#x}/act{}s",
                    c,
                    cur.pid(),
                    cur.preempt_count(),
                    act_ms / 1000
                );
            }
            }
        }
        // HOTPATH-PRINT: fuxstuck graph-burst gate.
        crate::pr_warn!(
            "RUNQ-STRAND tid={} tgid={} ({}) age={}s exec_frozen={}ms linked={} aff={:#x} throttled={} pol={:?} claim={} pk={:#x} ts={}ms skips={} vr={:#x} nrrun={} min_vr={:#x}{}{}",
            tk.pid(),
            tk.tgid(),
            core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0'),
            age / 100,
            exec / 1_000_000,
            linked,
            tk.cpus_allowed(),
            throttled as u8,
            tk.policy(),
            tk.running_on_cpu.load(core::sync::atomic::Ordering::Acquire),
            tk.pick_state(),
            tk.get_time_slice() * 1000 / crate::config::KERNEL_HZ as u32,
            skips,
            vr,
            GlobalRunQueue::grq_nr_running(),
            min_vr,
            wall_s,
            inc_s
        );
    }
}

/// 63rd session (mission lever 2): per-burst wake→pick latency profile
/// for the FFRT thread group (every task whose comm starts with
/// "OS_FFRT" — the workers AND the delayed-worker wheel). Prints one
/// FFRTWAIT line per fuxstuck census burst: the per-thread pick/wake
/// counts and wait samples, plus the AGGREGATE average link→pick wait
/// over the burst window (delta vs the previous burst) — the quiet-
/// phase vs wedge-phase wake-to-run cost comparison the 62nd asked
/// for. Zero cost when the history gate is off.
pub fn ffrt_wait_census() {
    use alloc::format;
    use core::sync::atomic::Ordering::Relaxed;

    const SCAN: usize = 40;
    const MAXR: usize = 12;
    // (pid, pick_n, wake_n, wait_last, wait_max, wait_open) for every
    // OS_FFRT thread (bounded), then the 12 most interesting are
    // printed: never-picked-but-woken (the wheel strand class) first,
    // then currently-waiting, then the rest.
    let mut scan: [(u32, u64, u64, u64, u64, u64); SCAN] = [(0, 0, 0, 0, 0, 0); SCAN];
    let mut ns = 0usize;
    let mut sum = 0u64;
    let mut cnt = 0u64;
    let mut mx = 0u64;
    crate::sched::hotpath::for_each_task_try(|task| {
        if ns >= SCAN {
            return;
        }
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        let comm = tk.comm();
        if comm[..7] != *b"OS_FFRT" {
            return;
        }
        let pick_n = tk.pick_n.load(Relaxed);
        let wake_n = tk.wake_n.load(Relaxed);
        let wait_last = tk.wait_last_ns.load(Relaxed) / 1000;
        let wait_max = tk.wait_max_ns.load(Relaxed) / 1000;
        let wait_open = tk.wait_open_ns.load(Relaxed);
        scan[ns] = (tk.pid(), pick_n, wake_n, wait_last, wait_max, wait_open);
        ns += 1;
        sum = sum.wrapping_add(tk.wait_sum_ns.load(Relaxed) / 1000);
        cnt += wake_n;
        mx = mx.max(wait_max);
    });
    // Rank: never-picked-but-woken (score 0) < currently-waiting (1) <
    // rest (2); stable within a class by pid.
    let mut order: [usize; SCAN] = [0; SCAN];
    for (i, o) in order.iter_mut().enumerate() {
        *o = i;
    }
    let key = |i: usize| -> (u8, u32) {
        let r = scan[i];
        if r.1 == 0 && r.2 > 0 {
            (0, r.0)
        } else if r.5 != 0 {
            (1, r.0)
        } else {
            (2, r.0)
        }
    };
    order[..ns].sort_by(|&a, &b| key(a).cmp(&key(b)));
    let nr = ns.min(MAXR);
    static PREV_SUM: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static PREV_CNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let prev_sum = PREV_SUM.swap(sum, Relaxed);
    let prev_cnt = PREV_CNT.swap(cnt, Relaxed);
    // Ohw31 lesson: tasks EXIT between bursts, so the per-task sums can
    // shrink (an exited thread's contribution disappears) — a plain
    // subtraction panicked the boot mid-wedge. Saturating: a shrinking
    // aggregate simply reports 0 for the window.
    let davg = if cnt > prev_cnt && sum > prev_sum {
        (sum - prev_sum) / (cnt - prev_cnt)
    } else {
        0
    };
    let mut s = alloc::string::String::new();
    use core::fmt::Write as _;
    for &oi in order.iter().take(nr) {
        let r = scan[oi];
        let _ = write!(
            s,
            " |{}pk{}wk{}wl{}wm{}",
            r.0, r.1, r.2, r.3, r.4
        );
    }
    // HOTPATH-PRINT: fuxstuck graph-burst gate.
    crate::pr_warn!(
        "FFRTWAIT n={} davg={}us max={}us{}",
        nr,
        davg,
        mx,
        s
    );
}

/// rux:claiminject (retired debug=claiminject) adjudication hook (GNOME-wedge hunt): leak a claim of
/// the nonexistent CPU 7 onto every linked CFS task currently showing the
/// pickable state {on_cpu=false, claim=-1}. Called from the timer softirq
/// every 50 jiffies. Returns the number of tasks poisoned (the caller
/// prints it); a busy GRQ simply skips that round.
pub fn debug_claim_inject() -> usize {
    match try_grq_lock() {
        Some(mut g) => g.cfs_rq.claim_inject_scan(),
        None => 0,
    }
}

/// rux:oncpuinject (retired debug=oncpuinject) adjudication hook (R85 tail-wedge hunt): stamp the
/// lost-release ownership shape {on_cpu=true, claim=7, pk=LIVE} onto
/// every linked CFS task currently showing the pickable state
/// {on_cpu=false, claim=-1}. Called from the timer softirq every 50
/// jiffies. Returns the number of tasks poisoned (the caller prints it);
/// a busy GRQ simply skips that round.
pub fn debug_oncpu_inject() -> usize {
    match try_grq_lock() {
        Some(mut g) => g.cfs_rq.oncpu_inject_scan(),
        None => 0,
    }
}

/// 105th wake-dispatch census (bindertrace consumer): the LAST wake's
/// dispatch outcome, written by Task::wake_up_impl right after the
/// claim/poke decision. Global (per-CPU races only blur the label —
/// the binder hunt reads it immediately after its own wake call):
///   1 = idle CPU claimed + IPI
///   2 = no idle; sync wake, wakee won the vruntime compare -> poke
///   3 = no idle; sync wake, compare lost -> NO poke (park handoff)
///   4 = no idle; async wake, preempt check poked
///   5 = no idle; async wake, recency/compare gate -> NO poke
// WAKE_DISP moved FRAMEWORK-side (devirt lane; sched/entities.rs).

/// Same census: the idle-CPU mask latched at wake ENTRY (before the
/// claim), so "idle existed but claim lost/failed" vs "no idle at all"
/// are told apart.
// WAKE_IDLE_MASK moved FRAMEWORK-side (devirt lane; sched/entities.rs).

pub(crate) fn g_diag_idle_bit(cpu: usize) -> bool {
    grq().idle_cpus.load(core::sync::atomic::Ordering::Relaxed) & (1u32 << cpu) != 0
}

/// Check if a newly-enqueued CFS task should preempt a running CFS task.
///
/// Batch 8 (HIGH): CFS had NO wakeup preemption — check_preempt() in
/// fair.rs existed but had no callers, so a woken task waited for the next
/// tick (up to 10ms) even when its vruntime was far behind the CPU hog it
/// should have preempted. Mirrors check_rt/dl_preempt: resched the first
/// CPU (within the task's affinity) whose current task loses to the waker
/// by the wakeup granularity.
pub(crate) fn check_cfs_preempt(task: *mut Task, cpus_allowed: u32) {
    let wakee = crate::sched::r#unsafe::task_shared(task);
    let se_vruntime = wakee.sched_entity().get_vruntime();
    for cpu in 0..MAX_CPUS {
        if (cpus_allowed & (1u32 << cpu)) == 0 {
            continue;
        }
        let running = cpu_state(cpu).current;
        if running.is_null() || running == cpu_state(cpu).idle {
            continue;
        }
        let incumbent = crate::sched::r#unsafe::task_shared(running);
        let r_policy = incumbent.policy();
        if r_policy == SchedPolicy::Normal
            || r_policy == SchedPolicy::Batch
            || r_policy == SchedPolicy::Idle
        {
            // Idle-policy tasks never preempt anyone (Linux).
            if wakee.policy() == SchedPolicy::Idle {
                continue;
            }
            let curr_vruntime = incumbent.sched_entity().get_vruntime();
            if se_vruntime < curr_vruntime {
                let delta = curr_vruntime - se_vruntime;
                if delta > crate::sched::hotpath::SCHED_MIN_GRANULARITY_NS {
                    resched_cpu(cpu);
                    return;
                }
            }
        }
        // RT/DL incumbents preempt CFS regardless — their own checks handle those.
    }
}

/// R3 (rt-r3 lane): one poke attempt for an RT wakee against `cpu`'s
/// incumbent — `true` when the wakeup-preemption comparison poked (the
/// caller stops scanning). Extracted from check_rt_preempt's loop body
/// so the placement-first probe and the affinity scan share ONE decision
/// path. No latency window: the RT wake compares priorities only, the
/// reference's check_preempt_curr_rt shape.
// SAFETY: task is a valid pointer from enqueue_task; cpu_state(cpu).current/idle
// are valid pointers set during init; null checks before dereference.
fn rt_try_poke_cpu(task: &Task, task_prio: u32, cpu: usize) -> bool {
    let running = cpu_state(cpu).current;
    if running.is_null() || running == cpu_state(cpu).idle {
        // timer-gap fix (the RT-2X §3.3 double-refusal): an
        // idle-SLOT cpu whose idle BIT is CLEAR is not provably
        // empty — it is either mid pick→mark_idle transition or
        // a lying slot (a pick that never reached its switch,
        // or a heal writing idle while a real task runs;
        // syscost pinned-RT capture: 433 user + 5724
        // idle-classified ticks on the same cpu).
        // claim_idle_cpu_for ALSO refuses it (bit clear), so
        // skipping here left NO dispatch path willing to poke
        // the cpu — the wedge. Poke instead: worst case the
        // cpu's pick re-runs and re-picks its incumbent (one
        // spurious IPI on a transient); best case it breaks a
        // lying-slot wedge. A genuinely-idle cpu has its bit
        // SET and stays on the claim path.
        let idle_bit_set = grq()
            .idle_cpus
            .load(core::sync::atomic::Ordering::Acquire)
            & (1u32 << cpu)
            != 0;
        if !running.is_null() && !idle_bit_set {
            // RT-2X item-7 hunt print kept (rt2xdbg=1,
            // 200-line budget): the poke decision, live.
            if task_prio > 85 && crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
                static CSP: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if CSP.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 200 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn ph(mut v: u64) {
                        for _ in 0..16 {
                            let n = (v >> 60) as u8;
                            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                            v <<= 4;
                        }
                    }
                    for &b in b"CRP-POKE-IDLE-SLOT cpu=" { putchar(b); }
                    ph(cpu as u64);
                    for &b in b" wakee=0x" { putchar(b); }
                    ph(task.pid() as u64);
                    for &b in b" prio=" { putchar(b); }
                    ph(task_prio as u64);
                    putchar(b'\n');
                }
            }
            resched_cpu(cpu);
            return true;
        }
        return false;
    }
    let incumbent = crate::sched::r#unsafe::task_shared(running);
    let r_policy = incumbent.policy();
    if r_policy == SchedPolicy::Normal
        || r_policy == SchedPolicy::Batch
        || r_policy == SchedPolicy::Idle
    {
        resched_cpu(cpu);
        return true;
    }
    if (r_policy == SchedPolicy::Fifo || r_policy == SchedPolicy::Rr)
        && task_prio > incumbent.rt_priority()
    {
        resched_cpu(cpu);
        return true;
    }
    false
}

/// Check if a newly-enqueued RT task should preempt a running task on another CPU.
/// R3 (rt-r3 lane): `prefer` is the wakee's PLACEMENT cpu (its ti_cpu after
/// select_task_rq/steer at the wake point; -1 when the caller has no
/// placement). The reference runs check_preempt_curr on the rq the wakee was
/// enqueued on — the poke targets the PLACEMENT, not "whatever eligible cpu
/// the ascending scan hits first". Placement-first matters because the
/// ascending scan's first winner is usually cpu0 — commonly the WAKER's own
/// cpu, where the poke sits pending behind the waker's syscall (kernel
/// preemption is off) while the placement cpu (often the cpu the timer
/// hardirq fired on, for a timer wake: a SELF-poke with no IPI exit cost at
/// all) could switch immediately. Falls through to the affinity scan when
/// the placement loses the comparison, so no dispatch is lost vs the old
/// shape.
pub(crate) fn check_rt_preempt(task: *mut Task, cpus_allowed: u32, prefer: i32) {
    let wakee = crate::sched::r#unsafe::task_shared(task);
    let task_prio = wakee.rt_priority();
    if prefer >= 0 && (prefer as usize) < MAX_CPUS && (cpus_allowed & (1u32 << prefer)) != 0 {
        if rt_try_poke_cpu(wakee, task_prio, prefer as usize) {
            return;
        }
    }
    for cpu in 0..MAX_CPUS {
        if (cpus_allowed & (1u32 << cpu)) == 0 {
            continue;
        }
        if rt_try_poke_cpu(wakee, task_prio, cpu) {
            return;
        }
    }
}

/// R3 (rt-r3 lane): one poke attempt for a DL wakee against `cpu`'s
/// incumbent — `true` when the comparison poked. Same extraction rationale
/// as rt_try_poke_cpu (placement-first + scan share one decision path).
// SAFETY: cpu_state(cpu).current/idle are valid pointers set during CPU
// init; null checks before dereference.
fn dl_try_poke_cpu(task_dl: u64, cpu: usize) -> bool {
    let running = cpu_state(cpu).current;
    if running.is_null() || running == cpu_state(cpu).idle {
        return false;
    }
    let incumbent = crate::sched::r#unsafe::task_shared(running);
    let r_policy = incumbent.policy();
    if r_policy != SchedPolicy::Deadline {
        resched_cpu(cpu);
        return true;
    }
    let r_dl = incumbent.dl_entity().deadline.load(core::sync::atomic::Ordering::Acquire);
    if task_dl < r_dl {
        resched_cpu(cpu);
        return true;
    }
    false
}

/// Check if a newly-enqueued DL task should preempt a running task on another CPU.
/// R3 (rt-r3 lane): `prefer` = the wakee's placement cpu (see
/// check_rt_preempt) — tried first, then the affinity scan.
pub(crate) fn check_dl_preempt(task: *mut Task, cpus_allowed: u32, prefer: i32) {
    let wakee = crate::sched::r#unsafe::task_shared(task);
    let task_dl = wakee.dl_entity().deadline.load(core::sync::atomic::Ordering::Acquire);
    if prefer >= 0 && (prefer as usize) < MAX_CPUS && (cpus_allowed & (1u32 << prefer)) != 0 {
        if dl_try_poke_cpu(task_dl, prefer as usize) {
            return;
        }
    }
    for cpu in 0..MAX_CPUS {
        if (cpus_allowed & (1u32 << cpu)) == 0 {
            continue;
        }
        if dl_try_poke_cpu(task_dl, cpu) {
            return;
        }
    }
}


/// Atomically change a task's scheduling policy (and RT priority) with the
/// proper run-queue migration (review PROC-P07): dequeue from the OLD class
/// queue, update the fields, enqueue into the NEW class queue — all under
/// the GRQ lock. Without the migration a task could sit on two class queues
/// at once (or on none), letting two CPUs pick it simultaneously or
/// stranding it off-queue forever.
pub fn change_task_policy(task: *mut Task, new_policy: crate::process::task::SchedPolicy, rt_prio: u32) {
    use crate::process::task::SchedPolicy;

    let is_current = match current() {
        Some(c) => c as *const Task == task as *const Task,
        None => false,
    };

    let mut grq_guard = grq().lock_irqsave();
    let old_policy = crate::sched::r#unsafe::task_shared(task).policy();

    // Whether the task is actually linked on a class run queue. The RUNNING
    // state is NOT sufficient: a task picked by another CPU is dequeued at
    // pick time — re-enqueueing it lets two CPUs run it simultaneously
    // (regression round 5, HIGH).
    //
    // Batch 8 (CBS): a THROTTLED DL task is deliberately unlinked (it sleeps
    // until its replenish timer fires). Treat it as linked here so the
    // policy switch migrates it onto the new class queue instead of leaving
    // it stranded asleep with its timer cancelled.
    let mut was_throttled = false;
    if old_policy == SchedPolicy::Deadline {
        let dl = crate::sched::r#unsafe::task_shared(task).dl_entity();
        was_throttled = dl.dl_throttled.load(core::sync::atomic::Ordering::Acquire);
        if was_throttled {
            // Cancel the replenish timer — the new class owns the wake now.
            dl.dl_throttled.store(false, core::sync::atomic::Ordering::Release);
            let stale = dl.replenish_timer.swap(0, core::sync::atomic::Ordering::AcqRel);
            if stale != 0 {
                crate::timer::del_timer(stale);
            }
        }
    }
    let linked = was_throttled
        || match old_policy {
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                crate::sched::r#unsafe::task_shared(task).rt_entity().is_on_rq()
            }
            SchedPolicy::Deadline => {
                crate::sched::r#unsafe::task_shared(task).dl_entity().is_on_rq()
            }
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                crate::sched::r#unsafe::task_shared(task).sched_entity().is_on_rq()
            }
        };

    if linked {
        // R31-5 discipline: propagate the class dequeue result (RT/DL now
        // return accurate bools; the old `; true` hardcodes drifted
        // nr_running whenever the guarded dequeue was a no-op).
        let dequeued = match old_policy {
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                grq_guard.rt_rq.dequeue(task)
            }
            SchedPolicy::Deadline => {
                grq_guard.dl_rq.dequeue(task)
            }
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                grq_guard.cfs_rq.dequeue(task)
            }
        };
        if dequeued {
            grq_guard.nr_running.fetch_update(
                core::sync::atomic::Ordering::SeqCst,
                core::sync::atomic::Ordering::SeqCst,
                |v| v.checked_sub(1),
            );
        }
    }

    // SAFETY: exclusive access under the GRQ lock.
    let old_rt_prio = crate::sched::r#unsafe::task_shared(task).rt_priority();
    crate::sched::r#unsafe::task_mut(task).set_rt_priority(rt_prio);
        crate::sched::r#unsafe::task_mut(task).set_policy(new_policy);

    // timer-gap lane (pinned-RT wedge, part 2 — the deterministic leg):
    // a RUNNING task's own setscheduler never touches the queues (it is
    // unlinked from pick until its next schedule), so the linked-path's
    // check_rt_preempt below never runs for it. When the change WEAKENS
    // the running incumbent (priority drop, or class demotion), a task
    // already linked on the queue may now outrank it — the reference
    // runs check_preempt_curr at __setscheduler time for on-rq tasks
    // (running tasks stay linked there), which rescheds the rq. Mirror
    // with a self-poke: this cpu's next trap/syscall exit re-runs the
    // pick, which takes the now-higher queued task. pi_min/pi_stress
    // pinned groups wedge deterministically without it: the migrating
    // thread parks at its INHERITED priority (equal to the incumbent's
    // — no poke from the migration dispatch), then the incumbent's
    // set_fifo LOWER runs here with linked=false and nothing pokes.
    if is_current && !linked {
        fn class_rank(p: crate::process::task::SchedPolicy) -> u32 {
            use crate::process::task::SchedPolicy;
            match p {
                SchedPolicy::Deadline => 3,
                SchedPolicy::Fifo | SchedPolicy::Rr => 2,
                _ => 1,
            }
        }
        let rt_dropped = matches!(old_policy, SchedPolicy::Fifo | SchedPolicy::Rr)
            && matches!(new_policy, SchedPolicy::Fifo | SchedPolicy::Rr)
            && rt_prio < old_rt_prio
            && grq_guard.rt_rq.highest_linked_user_prio(
                crate::sched::r#unsafe::task_shared(task).cpus_allowed(),
                rt_prio,
            );
        let demoted = class_rank(new_policy) < class_rank(old_policy);
        if rt_dropped || demoted {
            set_need_resched();
        }
    }

    // Re-enqueue into the new class queue under the same lock.
    if linked {
        // SAFETY: GRQ lock is held via grq_guard; enqueue_task_locked
        // expects the lock to be held.
        enqueue_task_locked(&mut *grq_guard, crate::sched::r#unsafe::task_mut(task));
        // Cross-CPU preemption checks mirror enqueue_task.
        let policy = crate::sched::r#unsafe::task_shared(task).policy();
        if policy == SchedPolicy::Fifo || policy == SchedPolicy::Rr {
            check_rt_preempt(task, crate::sched::r#unsafe::task_shared(task).cpus_allowed(), crate::sched::r#unsafe::task_shared(task).ti_cpu());
        } else if policy == SchedPolicy::Deadline {
            check_dl_preempt(task, crate::sched::r#unsafe::task_shared(task).cpus_allowed(), crate::sched::r#unsafe::task_shared(task).ti_cpu());
        }
    }
}


/// R25-6: renice a possibly-queued task under the GRQ lock with
/// load_weight rebalance (bare set_nice made enqueue/dequeue weights
/// mismatch and wrap the counter).
///
/// Safe since 2026-11-19 (unsafe-slim): the body takes the GRQ lock
/// itself; the caller's obligation is purely liveness of `task` (hold a
/// pin or be the task's own CPU), expressed by the `&mut Task` type.
pub fn sched_renice_locked(task: &mut Task, niceval: i32) {
    // R31-3: actually take the GRQ lock, and gate the load_weight
    // rebalance on the task being IN THE TREE (picked tasks are off it —
    // the unconditional adjust permanently shifted the counter for the
    // common running/sleeping renice target).
    let _g = grq().lock_irqsave();
    let old_weight = task.sched_entity().load.weight;
    let was_on_rq = task.sched_entity().is_on_rq();
    task.set_nice(niceval);
    let new_weight = task.sched_entity().load.weight;
    if was_on_rq && old_weight != new_weight {
        let g = grq();
        g.cfs_rq.load_weight.fetch_add(new_weight, core::sync::atomic::Ordering::AcqRel);
        g.cfs_rq.load_weight.fetch_sub(old_weight, core::sync::atomic::Ordering::AcqRel);
    }
}

pub fn yield_cpu() {
    // Batch 8: sched_yield was a no-op for CFS — schedule() re-picked the
    // same (leftmost) task. Linux yield_task_fair places the yielder behind
    // its peers: charge any outstanding exec time, then bump vruntime by one
    // full slice so the requeue at pick time puts equal entities first.
    if let Some(cur) = current() {
        let policy = cur.policy();
        if matches!(policy, SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle) {
            let task_ptr = cur as *mut Task;
            let mut grq_guard = grq().lock_irqsave();
            let now = crate::sched::hotpath::sched_clock();
            let se = cur.sched_entity();
            if grq_guard.cfs_rq.get_curr() == task_ptr {
                grq_guard.cfs_rq.update_curr(now);
            } else {
                let delta = se.update_exec_runtime(now);
                if delta > 0 {
                    se.update_vruntime(delta);
                }
            }
            let slice = grq_guard.cfs_rq.sched_slice(se);
            se.add_vruntime(slice);
        }
    }
    schedule();
}


/// Load balance — no-op with global RQ (inherently balanced).
pub fn load_balance() {
    // No-op: global queue is always balanced
}

/// R56 round 2: one sighting record for an orphan shape that needs TIME
/// confirmation before the harvester may act. The slot-capture family (a
/// busy slot still names the orphan while no context switch ever runs it)
/// is indistinguishable from a healthy long runner in every scheduler
/// state — only persistence across ticks separates them, and a task that
/// has been "mid-pick" for several jiffies is not mid-pick.
struct OrphanWatchSlot {
    task: core::sync::atomic::AtomicU64,
    pid: core::sync::atomic::AtomicU32,
    seen_at: core::sync::atomic::AtomicU64,
    /// sum_exec_runtime at the sighting (R81 progress-conviction).
    exec: core::sync::atomic::AtomicU64,
    /// 119th: the record's exec clock has NOT advanced since the
    /// sighting — the task is not running, so this record is AGING
    /// toward confirmation. The eviction policy must not drop such a
    /// record (that is exactly what it did: the oldest seen_at IS the
    /// record closest to confirmation, so every new orphan evicted the
    /// very entry that was about to mature — the reason the R80/R85
    /// heals NEVER fired on a busy box: 299 WAKE-RUN-LEAKs, 0 heals).
    aging: core::sync::atomic::AtomicBool,
}

const EMPTY_ORPHAN_WATCH: OrphanWatchSlot = OrphanWatchSlot {
    task: core::sync::atomic::AtomicU64::new(0),
    pid: core::sync::atomic::AtomicU32::new(0),
    seen_at: core::sync::atomic::AtomicU64::new(0),
    exec: core::sync::atomic::AtomicU64::new(0),
    aging: core::sync::atomic::AtomicBool::new(false),
};

static ORPHAN_WATCH: [OrphanWatchSlot; 16] =
    [const { EMPTY_ORPHAN_WATCH }; 16];

/// Returns true when the same (task, pid) sighting has already been
/// recorded at least `threshold` jiffies ago; records/refreshes the
/// sighting otherwise. The caller clears the entry (orphan_watch_clear)
/// once it acts on a confirmed orphan.
///
/// R81 (progress-conviction, the sc252/sc253 wake-leak cadence bug): the
/// old freshness rule re-recorded the sighting whenever two observations
/// were more than 60 jiffies apart, so confirmation required a second
/// look 50..60 jiffies after the first. The idle-loop harvester feeds
/// the watch at that cadence, but on a BUSY box (the OH boot: both CPUs
/// pegged by ETIMEDOUT/EAGAIN futex spin loops) the only observers are
/// the binder-stall rekicks — REKICK_SECS=15 = 1500 jiffies apart at
/// HZ=100. Every rekick found a >60-jiffy-stale record, re-recorded, and
/// returned false: the R80 phantom heal could NEVER mature while both
/// CPUs stayed busy (sc252: 1359 WAKE-RUN-LEAK lines, zero R80 heals;
/// sc253: 2050, zero). The conviction channel is now the exec clock: a
/// task executing ANYWHERE is charged by update_curr on every tick (and
/// at switch-out), so `sum_exec_runtime` frozen across two sightings
/// `threshold` jiffies apart proves nobody ran it — persistence without
/// a freshness assumption, at ANY observer cadence. An ADVANCING exec
/// clock refreshes the record (the shape is fresh again; a healthy
/// mid-switch-out transient ran between sightings and must not pre-age a
/// later genuine orphan), replacing the 60-jiffy re-record rule.
/// NEVER-RAN EXEMPTION (smp8 orphan-race): a sighting with exec == 0 can
/// never confirm — see the in-body comment; frozen-at-zero is definition,
/// not evidence.
/// CLAIMER-LIVENESS GATE (smp8 orphan-race): even a matured frozen-nonzero
/// record refuses while the claim CPU is demonstrably ALIVE (R57 pair:
/// tick + IRQ both frozen >= 5 s required) — frozen exec only proves the
/// TASK did not run, not that its pick's owner is gone.
pub(crate) fn orphan_watch_confirm(task: *mut Task, pid: u32, now: u64, threshold: u64) -> bool {
    use core::sync::atomic::Ordering;
    let raw = task as u64;
    // SAFETY: read-only sched-entity word; the task is pinned by the
    // caller's context (wake path holds the GRQ, harvester holds the
    // bucket lock).
    let exec = crate::sched::r#unsafe::task_shared(task).sched_entity().sum_exec_runtime.load(Ordering::Relaxed);
    let mut free_idx: usize = usize::MAX;
    let mut oldest_idx = 0usize;
    let mut oldest_at = u64::MAX;
    // 119th: an AGING record (exec clock frozen across sightings) is
    // one jiffy away from confirming — evicting it resets the whole
    // conviction. Prefer a non-aging slot for eviction; among the
    // non-aging ones take the oldest.
    let mut evict_idx = usize::MAX;
    let mut evict_at = u64::MAX;
    for i in 0..ORPHAN_WATCH.len() {
        let s = &ORPHAN_WATCH[i];
        let t = s.task.load(Ordering::Acquire);
        if t == raw {
            if s.pid.load(Ordering::Acquire) == pid {
                let first = s.seen_at.load(Ordering::Acquire);
                let first_exec = s.exec.load(Ordering::Acquire);
                // smp8 orphan-race (never-ran exemption): a task whose
                // exec clock reads 0 has NEVER RUN. The R81 frozen-exec
                // conviction ("exec frozen across two sightings
                // `threshold` jiffies apart proves nobody ran it, so the
                // marks' owner is gone") is the lost-release theory — it
                // presumes the task HAS run, carried true marks into a
                // switch-out, and lost that release. For a never-ran task
                // the theory is vacuous: its clock is frozen at 0 BY
                // DEFINITION, frozen exec cannot distinguish stale marks
                // from a still-legitimate pick — a starved picker between
                // its slot publish and its affirm, or a hart mid-switch-in
                // that has not taken its first tick, both present
                // {marks, no owner slot, no SWITCHING_OUT, exec==0} for as
                // long as TCG starvation stretches the window, because
                // `now` is a GLOBAL jiffy count advanced by OTHER harts
                // while the owning hart takes no ticks. Both smp8 captures
                // died exactly here: a parked-before-first-run kthread
                // (fdreapd, ticks=0, exec==0) convicted on first maturity,
                // its marks released while a hart was still inside the
                // switch protocol toward it, then a mixed-context resume
                // killed the box. Never confirm while exec == 0: treat
                // every never-ran sighting as fresh evidence. When the
                // task first runs, exec advances past 0, the record
                // re-bases at the nonzero clock (exec != first_exec
                // re-records), and the frozen-exec rule governs from there
                // — a task that HAS run and then frozen is the sound
                // lost-release shape and still convicts.
                if exec == 0 {
                    if first != 0 && now >= first && now - first >= threshold {
                        // This sighting would have matured under the old
                        // rule — name the exempted population (budgeted,
                        // once per threshold window per task because the
                        // refresh below re-bases seen_at) so the producer
                        // of never-ran orphan marks stays observable for
                        // the follow-up hunt.
                        static NR_N: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        if NR_N.fetch_add(1, Ordering::Relaxed) < 40 {
                            use crate::console::putchar_no_lock as putchar;
                            fn pdec(mut v: u64) {
                                use crate::console::putchar_no_lock as p;
                                let mut buf = [0u8; 20];
                                let mut n = 0;
                                if v == 0 {
                                    p(b'0');
                                    return;
                                }
                                while v > 0 && n < buf.len() {
                                    buf[n] = b'0' + (v % 10) as u8;
                                    v /= 10;
                                    n += 1;
                                }
                                for k in (0..n).rev() {
                                    p(buf[k]);
                                }
                            }
                            const M: &[u8] = b"\nORPHAN-WATCH-NEVERRAN pid=";
                            for &b in M {
                                putchar(b);
                            }
                            pdec(pid as u64);
                            const M2: &[u8] = b" age=";
                            for &b in M2 {
                                putchar(b);
                            }
                            pdec(now - first);
                            putchar(b'\n');
                        }
                    }
                    s.seen_at.store(now, Ordering::Release);
                    s.exec.store(0, Ordering::Release);
                    s.aging.store(false, Ordering::Release);
                    return false;
                }
                if first != 0
                    && now >= first
                    && now - first >= threshold
                    && exec == first_exec
                {
                    // smp8 orphan-race (claimer-liveness gate, the R57
                    // discipline at the conviction choke point): a matured
                    // frozen-exec record is NOT sufficient evidence that
                    // the marks' owner is gone. sum_exec_runtime advances
                    // only on the TASK's own ticks and switch-outs, while
                    // `now` is a global jiffy count advanced by every
                    // other hart — so under TCG starvation a pick whose
                    // owner is ALIVE (starved between its slot publish
                    // and its affirm, or resuming the task before its
                    // first charge) presents {marks, no owner slot (the
                    // boot-tail R56/R74 slot fights rewrote it), no
                    // SWITCHING_OUT, exec frozen} for as long as the
                    // starvation lasts, at ANY exec value — the sub-tick
                    // park of fdreapd/ksoftirqd-class kthreads shows the
                    // same frozen clock a lost-release orphan shows
                    // (orx84-4/orx88-1: STALE-ONCPU-RESET matured on a
                    // parked, sub-tick-ran kthread whose claimer was
                    // alive and printing; the release made the poisoned
                    // task pickable and a fresh resumer died instantly in
                    // a crossed context). Demand the R57 positive-death
                    // pair before releasing: the claim CPU's scheduler
                    // touch AND its timer-IRQ count both frozen >= 5 s —
                    // the n1-organic lesson (a starved-but-alive hart
                    // serviced 3-10 IRQs/s for 1600 s; the IRQ count is
                    // the aliveness witness). While the claimer lives,
                    // refuse: the record is NOT refreshed, so the next
                    // pass re-adjudicates against fresh liveness data.
                    // Cost: shapes whose marks ARE stale garbage stay
                    // unconvicted while their (innocent, alive) claimer
                    // runs — a soft wedge (the R80 strand-heal latency
                    // grows) — accepted because releasing under a live
                    // claimer is the observed box-killer; a wedge never
                    // executes a second hart on one stack.
                    let claim = crate::sched::r#unsafe::task_shared(task).running_on_cpu.load(Ordering::Acquire);
                    if claim >= 0 && (claim as usize) < MAX_CPUS {
                        let c = claim as usize;
                        let tick_ms = cpu_tick_stale_ms(c);
                        let irq_ms =
                            crate::debug::softlockup::cpu_irq_frozen_ms(c);
                        if tick_ms < 5000 || irq_ms < 5000 {
                            static LC_N: core::sync::atomic::AtomicU32 =
                                core::sync::atomic::AtomicU32::new(0);
                            static LC_LAST: core::sync::atomic::AtomicU64 =
                                core::sync::atomic::AtomicU64::new(0);
                            // Once matured this refusal repeats on every
                            // observer pass — print at most one line per
                            // 25 jiffies system-wide, budget 40 total.
                            let last = LC_LAST.load(Ordering::Relaxed);
                            if now >= last
                                && now - last >= 25
                                && LC_N.fetch_add(1, Ordering::Relaxed) < 40
                            {
                                LC_LAST.store(now, Ordering::Release);
                                use crate::console::putchar_no_lock as putchar;
                                fn pdec3(mut v: u64) {
                                    use crate::console::putchar_no_lock as p;
                                    let mut buf = [0u8; 20];
                                    let mut n = 0;
                                    if v == 0 {
                                        p(b'0');
                                        return;
                                    }
                                    while v > 0 && n < buf.len() {
                                        buf[n] = b'0' + (v % 10) as u8;
                                        v /= 10;
                                        n += 1;
                                    }
                                    for k in (0..n).rev() {
                                        p(buf[k]);
                                    }
                                }
                                const M: &[u8] =
                                    b"\nORPHAN-WATCH-LIVECLAIM pid=";
                                for &b in M {
                                    putchar(b);
                                }
                                pdec3(pid as u64);
                                const M2: &[u8] = b" claim=";
                                for &b in M2 {
                                    putchar(b);
                                }
                                pdec3(claim as u64);
                                const M3: &[u8] = b" tick_ms=";
                                for &b in M3 {
                                    putchar(b);
                                }
                                pdec3(tick_ms);
                                const M4: &[u8] = b" irq_ms=";
                                for &b in M4 {
                                    putchar(b);
                                }
                                pdec3(irq_ms);
                                putchar(b'\n');
                            }
                            return false;
                        }
                    }
                    return true;
                }
                // Re-record when the task RAN between sightings (exec
                // advanced: the shape is fresh, aging restarts) or the
                // record is empty/inverted.
                if first == 0 || now < first || exec != first_exec {
                    s.seen_at.store(now, Ordering::Release);
                    s.exec.store(exec, Ordering::Release);
                    s.aging.store(false, Ordering::Release);
                } else {
                    // The exec clock is frozen: this record is AGING
                    // toward confirmation. Mark it so the eviction
                    // policy leaves it alone.
                    s.aging.store(true, Ordering::Release);
                }
                return false;
            }
            // Same pointer, different pid — the task was freed and the page
            // reused. Re-record under the new identity.
            s.pid.store(pid, Ordering::Release);
            s.seen_at.store(now, Ordering::Release);
            s.exec.store(exec, Ordering::Release);
            s.aging.store(false, Ordering::Release);
            return false;
        }
        if t == 0 && free_idx == usize::MAX {
            free_idx = i;
        }
        let at = s.seen_at.load(Ordering::Acquire);
        if at < oldest_at {
            oldest_at = at;
            oldest_idx = i;
        }
        if !s.aging.load(Ordering::Acquire) && at < evict_at {
            evict_at = at;
            evict_idx = i;
        }
    }
    let i = if free_idx != usize::MAX {
        free_idx
    } else if evict_idx != usize::MAX {
        evict_idx
    } else {
        oldest_idx
    };
    ORPHAN_WATCH[i].task.store(raw, Ordering::Release);
    ORPHAN_WATCH[i].pid.store(pid, Ordering::Release);
    ORPHAN_WATCH[i].seen_at.store(now, Ordering::Release);
    ORPHAN_WATCH[i].exec.store(exec, Ordering::Release);
    ORPHAN_WATCH[i].aging.store(false, Ordering::Release);
    false
}

pub(crate) fn orphan_watch_clear(task: *mut Task) {
    use core::sync::atomic::Ordering;
    let raw = task as u64;
    for s in ORPHAN_WATCH.iter() {
        if s.task.load(Ordering::Acquire) == raw {
            s.task.store(0, Ordering::Release);
        }
    }
}

/// Milliseconds since the given CPU last took a scheduler tick (0 when
/// unknown/early-boot). A slot owned by a CPU that keeps ticking names a
/// healthy runner; a frozen TOUCH_TS names a hart that stopped executing
/// kernel code entirely.
pub(crate) fn cpu_tick_stale_ms(cpu: usize) -> u64 {
    let last = crate::debug::softlockup::last_touch_ns(cpu);
    if last == 0 {
        return 0;
    }
    let now = crate::arch::cpu::read_time()
        .saturating_mul(1_000_000_000)
        / crate::config::TIMER_CLOCK_FREQ_HZ as u64;
    now.saturating_sub(last) / 1_000_000
}

fn r56_print_msg(msg: &[u8]) {
    // R34 discipline: callers may hold the GRQ and/or a PID-hash bucket
    // lock here — SBI/direct console writes only.
    for &b in msg {
        crate::debug::taskdump::putc_brief(b)
    }
}

pub(crate) fn r56_print_pid(prefix: &[u8], pid: u32, suffix: &[u8]) {
    r56_print_msg(prefix);
    let v = pid as u64;
    // R81 (printer hardening, from the bigalloc8 adjudication): this
    // printer is called from heal paths that read POSSIBLY CORRUPTED
    // words (the R80 legs print a garbage claim; the integration
    // branch's capture hit a >=9-digit value from a corrupted pid field
    // and PANICKED here — index out of bounds len 8 index 8 — turning a
    // contained heal into a kernel panic inside the diagnostic itself).
    // u32::MAX is 10 digits; size for that and never index past it.
    let mut buf = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        let mut x = v;
        // Bound the digit count: a corrupted (>=9-digit) pid value must
        // not panic the heal-path printer itself (bigalloc8: index 8 vs
        // len 8 immediately after the R74-SLOTLESS-TP capture) — truncate
        // the garbage rather than losing the diagnostic line.
        while x > 0 && n < buf.len() {
            buf[n] = b'0' + (x % 10) as u8;
            x /= 10;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        crate::debug::taskdump::putc_brief(buf[i])
    }
    r56_print_msg(suffix);
}


/// smp8 orphan-race (sched-orphan-race lane): the abort-side twin of the
/// steal's re-link. Both arch switch-abort handlers assumed "the steal
/// already re-linked next (or a foreign picker owns it)" — true for every
/// REVOKED abort, but an abort can also fire on a LIVE-published pick
/// (mark_picked_on_cpu's owned_here leg publishes LIVE for a task whose
/// stale claim names this CPU; the wrapper's early check then reads
/// pick_state != PICK_PICKED and aborts) where NO steal is involved:
/// nothing revoked the pick, so nothing re-links the task. The pick
/// unlinked it and published its marks {on_cpu=1, claim=cpu} plus this
/// CPU's slot; the abort repairs the slot back to prev, leaving the task
/// UNLINKED, marked as this CPU's, named by no slot and no SWITCHING_OUT
/// — executing nowhere, invisible to every pick-side heal (they scan the
/// class timelines) and refused by the harvester (a LIVE pick-state is
/// never stolen). It strands until a wake survives the R80 50-jiffy
/// orphan-watch confirmation — and the stranded marks are exactly the
/// stale-claim/stale-oncpu food the pick-side adjudications then race
/// (the smp8 matrix panics: ENQ-CLAIMED on a linked-marked kthread,
/// STALE-ONCPU-RESET by a foreign CPU, mixed-context resume).
///
/// Adjudicate under the GRQ (same discipline as
/// member_release_stale_ownership): when `next` still carries THIS cpu's
/// full ownership marks ({on_cpu=1, claim=cpu}), is linked on no class
/// queue, and is named by no slot and no in-flight switch-out, the
/// abort's own publish is the marks' only author and nobody else can be
/// resuming the task — release the marks with the exact store the switch
/// protocol would have made ("explicitly not-own") and re-link it through
/// the normal enqueue. Idempotent by construction: when a steal handled
/// the pick its marks are already released and the task already linked,
/// so every condition below is false and the function is a no-op.
///
/// Called from the arch abort handlers (riscv_switch_abort /
/// x86_switch_abort) with IRQs disabled and no scheduler locks held;
/// takes the GRQ alone.
pub fn abort_next_release_orphans(cpu: usize, next: *mut Task) {
    if next.is_null() || cpu >= MAX_CPUS {
        return;
    }
    let cpus_allowed;
    // PROBE-LOCK DISCIPLINE (grq-lock-1st): the report is deferred to
    // AFTER the guard drop — printk's ring-buffer write ends in
    // `KMSG_READ_WQ.wake_up_all()`, whose waitqueue lock is held across
    // `wake_up_process` -> the GRQ acquire. A printk issued under the GRQ
    // re-acquires the GRQ on this cpu (self-deadlock with IRQs off) or
    // forms the GRQ->WQ / WQ->GRQ ABAA with another cpu. Capture the
    // fields under the lock, report after it is released.
    let mut report: Option<(usize, u32, i32, u32, u8)> = None;
    {
        let mut g = grq().lock_irqsave();
        // SAFETY: next was this CPU's just-aborted pick: the pick's GRQ
        // hold unlinked it and published its marks, and the abort repairs
        // the slot before this runs — under this GRQ hold the picture
        // below cannot move (no pick, no wake, no steal can interleave).
        {
            // INV-SCE-2: next was this CPU's just-aborted pick (see the
            // comment above — the GRQ hold freezes the picture).
            let next_t = crate::sched::r#unsafe::task_shared(next);
            let np = next_t.pid();
            if np == 0 || np == TASK_POISON {
                return; // idle is never marked; poison is not ours to fix
            }
            // Only OUR OWN publication is repairable here: the marks must
            // still name this CPU as the claimer with on_cpu set. A steal
            // that revoked the pick released the marks (claim=-1) and
            // re-linked the task already; a foreign owner never carries
            // our claim.
            let whole = crate::sched::r#unsafe::task_ownership_qword(next);
            let oncpu = (whole & 0xFF) != 0;
            let claim = (whole >> 32) as i32;
            if !oncpu || claim != cpu as i32 {
                return;
            }
            // Belt and braces (the marks' owner scan): if any slot or
            // in-flight switch-out still names the task, some path owns
            // it — leave it to that path.
            if pick_owner_live(next) {
                return;
            }
            // A dying task leaves through its own final schedule(); no
            // heal may re-offer it (R57).
            if task_is_dying(next) {
                return;
            }
            let linked = match next_t.policy() {
                SchedPolicy::Fifo | SchedPolicy::Rr => g.rt_rq.is_linked(next),
                SchedPolicy::Deadline => g.dl_rq.is_linked(next),
                SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                    g.cfs_rq.is_linked(next)
                }
            };
            if linked {
                return; // a racing wake already re-linked it: not an orphan
            }
            cpus_allowed = next_t.cpus_allowed();
            let pk = next_t.pick_state();
            // Explicitly not-own: the exact store __switch_to would have
            // made for a switch-out that will never happen.
            next_t.release_pick_ownership();
            let rq = enqueue_task_locked(&mut g, crate::sched::r#unsafe::task_mut(next));
            static SWABORT_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = SWABORT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 8 {
                report = Some((cpu, np, claim, pk, rq as u8));
            }
        }
    }
    if let Some((cpu, pid, claim, pk, rq)) = report {
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
        crate::pr_warn!(
            "SWITCH-ABORT-RELINK cpu={} pid={} claim={} pk={} rq={}",
            cpu,
            pid,
            claim,
            pk,
            rq
        );
    }
    // Nudge an idle CPU outside the lock (mirrors enqueue_task's tail) so
    // the just-relinked task does not wait for this CPU's next pass.
    if let Some(idle_cpu) = grq().find_idle_cpu(cpus_allowed) {
        grq().clear_idle(idle_cpu);
        resched_cpu(idle_cpu);
    }
}

/// R56-heal: rate-limited tripwire for switch-in aborts (at most one
/// line per jiffy system-wide — a pathological steal/abort ping-pong
/// must not flood the serial console).
pub fn r56_report_switch_abort(cpu: usize, pid: u32) {
    static LAST_ABORT_JIFFY: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST_ABORT_JIFFY.load(core::sync::atomic::Ordering::Relaxed);
    if now.wrapping_sub(last) < 1 {
        return;
    }
    if LAST_ABORT_JIFFY
        .compare_exchange(last, now, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    r56_print_pid(b"R56-SWITCH-ABORTED cpu=", cpu as u32, b" pid=");
    r56_print_pid(b"", pid, b"\n");
}

/// Lost-park idle-lie report (fix-lostpark lane, rux:lostpark (retired debug=lostpark)): called
/// from the idle loop when the CPU's own slot names a real task after
/// schedule() returned. Rate-limited to one line per CPU per 10 jiffies
/// with a global budget, direct-console (R34 discipline — the caller may
/// not hold the GRQ, but the diag path must stay lock-free anyway).
pub(crate) fn lp_idle_lie_report(cpu: usize) {
    use crate::debug::taskdump::putc_brief as putchar;
    static LP_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    static LP_LAST: [core::sync::atomic::AtomicU64; MAX_CPUS] =
        [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];
    if LP_N.load(core::sync::atomic::Ordering::Relaxed) >= 128 {
        return;
    }
    let jif = crate::timer::get_jiffies();
    let last = LP_LAST[cpu.min(MAX_CPUS - 1)].load(core::sync::atomic::Ordering::Relaxed);
    if jif.wrapping_sub(last) < 10 {
        return;
    }
    if LP_LAST[cpu.min(MAX_CPUS - 1)]
        .compare_exchange(
            last,
            jif,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }
    LP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    fn pdec(mut v: u64) {
        use crate::debug::taskdump::putc_brief as putchar;
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
    }
    fn phex(v: u64) {
        use crate::debug::taskdump::putc_brief as putchar;
        for sh in (0..16u32).rev() {
            let nb = ((v >> (sh * 4)) & 0xF) as u8;
            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
        }
    }
    // SAFETY: diagnostic reads of the slot-named task; the slot pins it
    // as this CPU's published current (it cannot be freed while named).
    {
        let curr = cpu_state(cpu).current;
        const M: &[u8] = b"\nLP-IDLE-LIE cpu=";
        for &b in M {
            putchar(b);
        }
        pdec(cpu as u64);
        const M2: &[u8] = b" slot_pid=";
        for &b in M2 {
            putchar(b);
        }
        pdec(crate::sched::r#unsafe::task_shared(curr).pid() as u64);
        const M3: &[u8] = b" st=0x";
        for &b in M3 {
            putchar(b);
        }
        phex(crate::sched::r#unsafe::task_shared(curr).state().bits() as u64);
        const M4: &[u8] = b" pc=0x";
        for &b in M4 {
            putchar(b);
        }
        phex(crate::sched::r#unsafe::task_shared(curr).preempt_count() as u64);
        const M5: &[u8] = b" oncpu=";
        for &b in M5 {
            putchar(b);
        }
        pdec(crate::sched::r#unsafe::task_shared(curr).on_cpu() as u64);
        let claim = crate::sched::r#unsafe::task_shared(curr).running_on_cpu.load(core::sync::atomic::Ordering::Relaxed);
        const M6: &[u8] = b" cl=";
        for &b in M6 {
            putchar(b);
        }
        if (claim as i64) < 0 {
            putchar(b'-');
        }
        pdec((claim as i64).unsigned_abs() as u64);
        const M7: &[u8] = b" pk=";
        for &b in M7 {
            putchar(b);
        }
        pdec(crate::sched::r#unsafe::task_shared(curr).pick_state() as u64);
        const M8: &[u8] = b" onrq=";
        for &b in M8 {
            putchar(b);
        }
        pdec(task_onrq_flag(crate::sched::r#unsafe::task_shared(curr)) as u64);
        const M9: &[u8] = b" slotw=";
        for &b in M9 {
            putchar(b);
        }
        pdec(SLOT_WRITER[cpu].load(core::sync::atomic::Ordering::Relaxed) as u64);
        const M10: &[u8] = b" wtask=0x";
        for &b in M10 {
            putchar(b);
        }
        phex(SLOT_WRITER_TASK[cpu].load(core::sync::atomic::Ordering::Relaxed));
        const M11: &[u8] = b" nr=";
        for &b in M11 {
            putchar(b);
        }
        pdec(GlobalRunQueue::grq_nr_running() as u64);
        putchar(b'\n');
    }
}
