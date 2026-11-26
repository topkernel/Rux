//! The FAIR class (S1b sole copy): the CFS policy over the framework
//! storage (sched/entities.rs owns CfsRunQueue + the entities; the pick
//! core is a FREE fn here — the orphan rule bars crate-side impls).


#![forbid(unsafe_code)]
use crate::sched::{types::CfsRunQueue, types::SchedEntity, types::LoadWeight, hotpath::NICE_0_LOAD, types::StrandWallRow};
use crate::sched::r#unsafe::{node_key, node_of_task, task_of_node};
use rux::api::sched::{CLASS_CFS_BIT, WW_EV_LINK, WW_EV_UNLINK_POISON};
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use crate::config::{MAX_CPUS, KERNEL_HZ, CFS_MIN_GRANULARITY_NS, CFS_LATENCY_NS};
use rux::process::types::{Task, SchedPolicy};
use crate::sched::{hotpath::SchedClass, types::RunQueueRef};

struct SkipHistSlot {
    task: AtomicU64,
    /// Refusal counts by reason (1=ownership/on_cpu/claim-this-cpu,
    /// 2=foreign claim, 3=affinity, 4=throttled, 5=healed reset).
    why: [AtomicU64; 5],
    /// Total refusals recorded.
    n: AtomicU64,
    /// jiffies at the FIRST recorded refusal of this episode.
    first_jif: AtomicU64,
}

const SKIP_HIST_SLOTS: usize = 256;
static SKIP_HIST: [SkipHistSlot; SKIP_HIST_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: SkipHistSlot = SkipHistSlot {
        task: AtomicU64::new(0),
        why: [const { AtomicU64::new(0) }; 5],
        n: AtomicU64::new(0),
        first_jif: AtomicU64::new(0),
    };
    [ZERO; SKIP_HIST_SLOTS]
};

pub(crate) static PICK_SKIP_ALL: AtomicU64 = AtomicU64::new(0);

/// (stash overflows, skip-all picks) since boot — the RUNQ-STRAND census
/// prints them on every burst.
pub fn pick_giveups() -> (u64, u64) {
    (
        PICK_STASH_OVERFLOW.load(Ordering::Relaxed),
        PICK_SKIP_ALL.load(Ordering::Relaxed),
    )
}

/// Total pick-walk entries since boot (WW5 heartbeat; wake-side
/// snapshot for the strand line's visits= field).
pub fn ww_pick_walks() -> u64 {
    // S1b sole-copy bug (the boot wedge): this impl is REGISTERED as the
    // framework hook (lib.rs register_all row `ww_pick_walks`), so the
    // old body's forward back to rux::api::sched::ww_pick_walks() was
    // self-recursion — hooks wrapper -> registered impl -> forwarder ->
    // hooks wrapper, each level copying the 73-row hook table to the
    // stack, until the stack chewed past the .bss page tables and the
    // trap handler re-trapped (sepc=trap_entry, silent wedge at the
    // first kswapd pick). Read the counter directly, like the
    // ww_pick_walk_inc closure row does.
    crate::sched::hotpath::WW_PICK_WALKS.load(core::sync::atomic::Ordering::Relaxed)
}

/// (on_cpu-mark heals, stale-claim heals) since boot — the two R85
/// heal legs. Reported per-second by WW5; ~0 with hundreds of strands
/// convicts "the heal never fires" where the rate-gated prints could
/// only suggest it.
pub fn ww_heals() -> (u64, u64) {
    (
        crate::sched::hotpath::WW_ONCPU_HEALS.load(Ordering::Relaxed),
        crate::sched::hotpath::WW_CLAIM_HEALS.load(Ordering::Relaxed),
    )
}

#[inline]
fn skip_hist_of(task: *mut Task) -> &'static SkipHistSlot {
    let a = task as usize;
    &SKIP_HIST[(a >> 6 ^ a >> 12) & (SKIP_HIST_SLOTS - 1)]
}

/// Record one pick-walk refusal of `task` (called under the GRQ; only
/// when rux:schedtrace (retired debug=schedtrace) is armed).
pub fn note_pick_skip(task: *mut Task, reason: u8) {
    let s = skip_hist_of(task);
    if s.task.load(Ordering::Relaxed) != task as usize as u64 {
        // New episode for this slot: reset the counters.
        s.task.store(task as usize as u64, Ordering::Release);
        for w in &s.why {
            w.store(0, Ordering::Relaxed);
        }
        s.n.store(0, Ordering::Relaxed);
        s.first_jif.store(crate::timer::get_jiffies(), Ordering::Release);
    }
    if let Some(w) = s.why.get(reason as usize) {
        w.fetch_add(1, Ordering::Relaxed);
    }
    s.n.fetch_add(1, Ordering::Relaxed);
}

/// Print (direct console, GRQ-held discipline) and clear the recorded
/// skip history of `task`. Called from wakewatch's strand report.
pub fn print_skip_history(task: *mut Task) {
    use crate::debug::hotpath::putc_brief as putchar;
    let s = skip_hist_of(task);
    if s.task.load(Ordering::Acquire) != task as usize as u64 {
        // s16: the 256-slot table is keyed by Task address and msgstress
        // churn steals slots mid-strand, so "no SKIPHIST line" was
        // ambiguous between "never refused" and "history clobbered by a
        // colliding task". Name the collision explicitly — the census
        // needs the distinction (a stolen slot cannot exonerate the
        // walk).
        for &c in b"\nSKIPHIST coll=1" {
            putchar(c);
        }
        putchar(b'\n');
        return;
    }
    let n = s.n.swap(0, Ordering::AcqRel);
    // s16: print the line even when n == 0. "SKIPHIST stays empty" was
    // previously an inference from no-line-at-all (ambiguous against
    // probe-off and slot-reuse); an explicit skips=0 on every strand
    // line makes the never-refused verdict census-readable.
    let now = crate::timer::get_jiffies();
    let first = s.first_jif.swap(0, Ordering::AcqRel);
    let mut whys = [0u64; 5];
    for (i, w) in s.why.iter().enumerate() {
        whys[i] = w.swap(0, Ordering::AcqRel);
    }
    s.task.store(0, Ordering::Release);
    fn pu(mut v: u64) {
        use crate::debug::hotpath::putc_brief as putchar;
        let mut buf = [0u8; 20];
        let mut k = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && k < buf.len() {
            buf[k] = b'0' + (v % 10) as u8;
            v /= 10;
            k += 1;
        }
        while k > 0 {
            k -= 1;
            putchar(buf[k]);
        }
    }
    for &c in b"\nSKIPHIST skips=" {
        putchar(c);
    }
    pu(n);
    for (i, cnt) in whys.iter().enumerate() {
        if *cnt == 0 {
            continue;
        }
        for &c in b" why" {
            putchar(c);
        }
        pu((i + 1) as u64);
        putchar(b'=');
        pu(*cnt);
    }
    // s16: span only meaningful with at least one recorded refusal
    // (first_jif is never set on a refusal-free episode).
    if n > 0 {
        for &c in b" span_ms=" {
            putchar(c);
        }
        pu(now.wrapping_sub(first) * 1000 / crate::config::KERNEL_HZ as u64);
    }
    putchar(b'\n');
}

/// 60th (RUNQ-STRAND census): NON-destructive peek of `task`'s recorded
/// pick-refusal history. REMOVED by the 63rd: superseded by the
/// persistent per-task counters (Task::pick_visit_n / pick_refuse_why)
/// which cannot be stolen by address collisions — print_skip_history
/// (the wakewatch one-shot under rux:schedtrace (retired debug=schedtrace)) remains.

/// Clock frequency (HZ) - from config
const HZ: u64 = KERNEL_HZ as u64;

/// Weight for SCHED_IDLE tasks
/// This is a very low weight (3) to give idle tasks minimal CPU time
pub const WEIGHT_IDLEPRIO: u64 = 3;


/// Sort key for the CFS timeline.
///
/// Since vruntime may be duplicate, we use (vruntime, task_id) as key
/// This ensures uniqueness
// The timeline engine (VruntimeKey, RbNode, RB_NIL, node_of_task/
// task_of_node, node_key, RbTimeline and its ops) moved into
// crate::sched::unsafe (INV-SCE-6) — the framework zone owns the
// raw-pointer tree surgery; this file keeps the CFS policy.
pub(crate) use crate::sched::r#unsafe::{RbNode, RbTimeline, VruntimeKey};

// R76 (fast-pace starvation hunt): per-pid pick-skip counters, direct-mapped
// by pid into 64 slots. Only read/written from inside pick_next_cpu's report
// block, which always runs under the GRQ lock — atomics are for the static's
// Sync bound only. When one pid's skip count crosses 1000 the report emits a
// SKIP-DEEP line with the task's state word, queue flag, cpu, and both stack
// pointers so a post-mortem can tell a linked-but-starved task from a
// phantom or a sleeper (the full3 wedge shape: PICK-SKIP-ALL naming pids
// 576-599 for minutes with no other explanation in the log).
pub(crate) struct SkipTrackEntry {
    pub(crate) pid: core::sync::atomic::AtomicU32,
    count: core::sync::atomic::AtomicU32,
    dumped_at: core::sync::atomic::AtomicU32,
}

static SKIP_TRACK: [SkipTrackEntry; 64] = [
    const {
        SkipTrackEntry {
            pid: core::sync::atomic::AtomicU32::new(0),
            count: core::sync::atomic::AtomicU32::new(0),
            dumped_at: core::sync::atomic::AtomicU32::new(0),
        }
    };
    64
];

/// grq-10ns: per-walk refusal ring — the successor pick walk's
/// decision-time evidence (task + why code, 32 slots, same cap as the
/// old stash array). All accesses under the GRQ (single writer between
/// the record loop and the report print); atomics exist only for the
/// static's Sync bound. Replaces the 768-byte stack array that the
/// stash dance initialized on EVERY pick inside the GRQ critical
/// section.
pub(crate) const PICK_SKIP_RING_LEN: usize = 32;
pub(crate) static PICK_SKIP_RING_TASK: [core::sync::atomic::AtomicU64; PICK_SKIP_RING_LEN] =
    [const { core::sync::atomic::AtomicU64::new(0) }; PICK_SKIP_RING_LEN];

pub(crate) static PICK_SKIP_RING_WHY: [core::sync::atomic::AtomicU32; PICK_SKIP_RING_LEN] =
    [const { core::sync::atomic::AtomicU32::new(0) }; PICK_SKIP_RING_LEN];

pub(crate) static PICK_SKIP_RING_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// grq-10ns: read the poison counters (debug census hook).
pub fn tl_poison_counters() -> (u64, u64) {
    (
        crate::sched::hotpath::tl_poison_inserts(),
        crate::sched::hotpath::tl_poison_erases(),
    )
}

/// Fair scheduling class - wraps CFS
///
/// With the global RunQueue design, the actual CFS enqueue/dequeue/pick
/// happens directly in sched.rs on GlobalRunQueue::cfs_rq. The SchedClass
/// methods are simplified to no-ops or minimal stubs.
pub struct FairSchedClass;

impl FairSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for FairSchedClass {
    fn name(&self) -> &'static str {
        "fair"
    }

    /// Actual enqueue happens in sched.rs::enqueue_task_locked.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual dequeue happens in sched.rs::dequeue_task.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        false
    }

    /// Actual yield handling happens in sched.rs.
    fn yield_task(&self, _rq: RunQueueRef) {
    }

    /// Preemption checks happen in sched.rs.
    fn wakeup_preempt(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual pick happens in sched.rs::pick_next_task.
    fn pick_next_task(&self, _rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        core::ptr::null_mut()
    }

    /// Actual re-enqueue happens in sched.rs::__schedule.
    fn put_prev_task(&self, _rq: RunQueueRef, _prev: *mut Task, _next: *mut Task) {
    }

    /// Minimal — actual setup happens in sched.rs.
    fn set_next_task(&self, _rq: RunQueueRef, _next: *mut Task, _first: bool) {
    }

    /// CFS load balancing is handled separately
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        false
    }

    /// CPU selection — simplified for global RQ.
    /// The global queue is inherently balanced; just keep on the current CPU.
    fn select_task_rq(&self, task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        if task.is_null() {
            return cpu;
        }

        // With global RQ, no per-CPU load balancing needed.
        // Just return the preferred CPU from the task's affinity.
        // SAFETY: task is a valid pointer from the caller; null check above.
        {
            let task_ref = crate::sched::r#unsafe::task_shared(task);
            let cpus_allowed = task_ref.cpus_allowed();
            let wake_cpu = task_ref.wake_cpu();

            // Prefer wake_cpu if allowed
            if wake_cpu >= 0 {
                let wake = wake_cpu as usize;
                if wake < crate::config::MAX_CPUS && (cpus_allowed & (1u32 << wake)) != 0 {
                    return wake_cpu;
                }
            }

            // Check if current cpu is allowed
            let cpu_usize = cpu as usize;
            if cpu_usize < crate::config::MAX_CPUS && (cpus_allowed & (1u32 << cpu_usize)) != 0 {
                return cpu;
            }

            // Fallback: first allowed CPU
            for c in 0..crate::config::MAX_CPUS {
                if (cpus_allowed & (1u32 << c)) != 0 {
                    return c as i32;
                }
            }

            cpu
        }
    }

    /// Actual tick handling happens in sched.rs::scheduler_tick.
    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
    }

    /// Actual runtime tracking happens in sched.rs.
    fn update_curr(&self, _rq: RunQueueRef) {
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, task: *mut Task) -> u32 {
        if task.is_null() {
            return 1;
        }

        // Return a reasonable default; actual slice is computed in sched.rs.
        1
    }

    /// Actual check happens in sched.rs via GlobalRunQueue::cfs_rq.
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        false
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&crate::idle::IDLE_SCHED_CLASS)
    }
}

/// Global fair scheduling class instance
pub static FAIR_SCHED_CLASS: FairSchedClass = FairSchedClass::new();



pub fn cfs_pick_next_cpu(rq: &mut crate::sched::types::CfsRunQueue, cpu_id: usize, prev: *mut crate::process::Task) -> Option<*mut crate::process::Task> {
        // s16 (WW5): one exact count per walk entry — the visits= field
        // on the strand line divides this by the strand window.
        crate::sched::hotpath::ww_pick_walk_inc();
        // grq-10ns: cctx walk anatomy (picks entered / candidates
        // consulted — the denominator for the walk-cost evidence).
        crate::debug::hotpath::cctx_bump(&crate::debug::hotpath::FAIR_PICK_N);
        // R7-G1 (GDB capture): this walk once kept a Vec::with_capacity
        // (a HEAP allocation per pick under the GRQ irqsave lock), then
        // a 32-slot stack array initialized on EVERY pick (768 B of
        // stores in the critical section). grq-10ns: the successor walk
        // REFUSES candidates in place (nothing is unlinked but the
        // picked one), so the evidence moves to the PICK_SKIP_RING
        // static below — zero per-pick stack or heap cost, and the
        // report path reads the ring.
        // R76 (fast-pace starvation hunt): each ring slot records the
        // DECISION-TIME skip reason — the report must not re-read
        // on_cpu/claim at print time, microseconds AFTER the decision
        // (the switch-out release store legally rewrites the qword in
        // that window). Reason codes:
        // 1 = on_cpu set (mid switch-out / fast-path repick window),
        // 2 = foreign live claim (claim=X, picker sees X as alive),
        // 3 = affinity mask, 4 = cgroup throttled, 5 = stale claim healed.
        let mut skip_count = 0usize;
        let mut result = None;

        // grq-10ns: SUCCESSOR WALK (the reference's rb_next iteration
        // over the timeline). Start at the cached leftmost node; each
        // candidate's successor is computed BEFORE the body so the
        // body's control flow (continue/break) stays the iterator-shape
        // it was; the walk stops after 32 refused candidates exactly
        // like the old stash window (the 60th's census semantics are
        // preserved: PICK_STASH_OVERFLOW counts the give-ups).
        let nil = rq.timeline.nil();
        let mut node = rq.timeline.leftmost;
        loop {
            if !(node != nil && !node.is_null() && RbTimeline::node_ok(node)) {
                break;
            }
            let task = task_of_node(node);
            // SAFETY: linked, in-range node (the key is the ORDER key
            // stored on the entity).
            let key = node_key(node);
            // Advance BEFORE the body: a refused candidate is stepped
            // past without any tree mutation.
            // SAFETY: node is in-range; next() reads only link fields.
            node = rq.timeline.next(node);
            crate::debug::hotpath::cctx_bump(&crate::debug::hotpath::FAIR_WALK_N);

            // 64th: vruntime sanity canary (see vr_canary) — checked on
            // every candidate the walk consults, decision-time.
            vr_canary(rq,task, b"PKC", key.vruntime);

            // b50 #GP guard family: a scribbled timeline link yields a
            // non-canonical *mut Task; the ownership probe below would
            // #GP the kernel on its first qword load (boot b50, ~20 min
            // in). grq-10ns: the poisoned NODE cannot be unlinked (its
            // link fields are the garbage) — report, refuse this CPU's
            // pick, and keep the kernel alive; the counters stay paired
            // with the still-linked entry (the census names the wedge).
            if !crate::sched::hotpath::kernel_task_ptr_range_ok(task as *const u8) {
                crate::sched::hotpath::tl_poison_erase_inc();
                crate::sched::hotpath::report_poisoned_rq_entry(cpu_id, task as usize);
                rux::api::sched::ww_record(task, rux::api::sched::WW_EV_UNLINK_POISON);
                break;
            }

            // SAFETY: task is from the BTreeMap, a valid pointer stored during enqueue.
            // R8-1b: skip tasks still marked on-CPU (context not saved —
            // picking one would resume a STALE thread.sp) UNLESS it is the
            // switching CPU's own prev, which re-picks itself only via the
            // next == prev early return (no context restore happens).
            // R65 (dual-bit ownership): ti_on_cpu and running_on_cpu are
            // set/cleared together by the switch protocol (mark at pick,
            // publish at switch-out), so a pickable task must show
            // on_cpu == false AND claim == -1 (or our own claim). Testing
            // BOTH closes the single-word scribble: a stray 8-byte store
            // of a kernel-static pointer over the ownership word resets
            // it to {false, -1} (x86-scribble3: the word held
            // 0xffffffff_80661100, a .rodata ops pointer), which let a
            // second CPU resume a task that was live on the first CPU's
            // stack — the shared-stack scribble engine. With the claim in
            // the predicate the corrupted task simply stays unpickable
            // (the detector names it) instead of double-running.
            //
            // GNOME-wedge (stale-claim heal): a LINKED task showing
            // {on_cpu == false, claim == X != -1} is unreachable by the
            // switch protocol itrq. The only legitimate linked states
            // are {false, -1} (sleeping/woken, preempted-and-requeued
            // after the switch-out release) and {true, X} (mid
            // switch-out / fast-path re-pick, skipped on the on_cpu leg).
            // The claim is published together with on_cpu at pick time
            // under the GRQ (publish_pick_ownership), and released only
            // by __switch_to's single full-qword {false, -1} store — no
            // code path clears one bit without the other on a linked
            // task. A linked task also cannot be
            // executing anywhere (running tasks are dequeued by their
            // pick), so resetting the claim cannot re-open the R65
            // double-run window: that guard protects RUNNING tasks, which
            // are unlinked and never reach this scan. Every producer of
            // the impossible state (a heal path that cleared on_cpu
            // alone, a qword-zeroing scribble, a wedged picker that never
            // reached its switch-out release) left the task stranded
            // permanently: linked, counted in nr_running, skipped by
            // every CPU forever — the g2 GNOME freeze (10 tasks stuck,
            // all CPUs idle-looping on the R20-2 recheck for 35 min,
            // screen dead, khungtaskd itself stranded). Reset the stale
            // claim, pick the task, and report loudly (rate-limited).
            // cgroup v2 (U1b): skip tasks whose cgroup chain is
            // cpu-throttled — they stay on the timeline and become
            // pickable again when the period rolls over (the same
            // affinity-style skip machinery above).
            // R76: skip_reason is written at DECISION time (see the array
            // comment above) — 0 = none, 1 = on_cpu, 2 = foreign live
            // claim, 3 = affinity, 4 = cgroup throttled, 5 = stale claim
            // healed (pickable on the NEXT pass).
            let mut skip_reason = 0u32;
            let allowed = {
                // R65-pick (probe-slow): the dual-bit ownership test reads
                // the whole qword in one load (Task::ownership_pickable) —
                // same e87a8b40 semantics, half the loads per candidate.
                // The GNOME-wedge stale-claim heal rides the REFUSAL path
                // only: ownership_pickable is the fast path, and a refusal
                // is split by re-reading the two bits — on_cpu=true is the
                // legitimate NEW2 skip, while {on_cpu=false, foreign claim}
                // on a LINKED candidate is the protocol-impossible state
                // the heal resets (see the comment above).
                let ownership_ok = if task == prev {
                    true
                } else if crate::sched::r#unsafe::task_shared(task).ownership_pickable(cpu_id) {
                    true
                } else {
                    // WAKE-FASTREPRO (orphan-mark root cause, protocol
                    // fix): the refusal branch used to consume TWO
                    // separate loads — the claim half here, the on_cpu
                    // byte again inside the branch conditions below —
                    // with the owner scan between them. A task completing
                    // a legitimate switch-out on another CPU transitions
                    // its qword {true,X} -> {false,-1} and clears its
                    // SWITCHING_OUT marker WITHOUT holding this GRQ, so
                    // the split loads could assemble {on_cpu=false,
                    // claim=X} — the "protocol-impossible" orphan pair —
                    // out of a transient that never existed atomically
                    // (12/12 captured STALE-CLAIM-RESET lines on the
                    // fpns rig showed whole=0xffffffff00000000: the
                    // qword was clean the whole time). Read the qword
                    // ONCE, atomically, and decide on the decoded pair:
                    // a mid-switch-out task reads {true,X} and lands on
                    // the why=1 leg; only an ATOMIC {false, X-foreign}
                    // reaches the stale-claim heal.
                    let whole_qword = crate::sched::r#unsafe::task_ownership_qword(task);
                    let split_oncpu = (whole_qword & 0xFF) != 0;
                    let claim = (whole_qword >> 32) as i32;
                    // R85 (tail-phase PICK-SKIP wedge): adjudicate every
                    // refusal against SCHEDULER TRUTH instead of the
                    // advisory bits. pick_owner_live (sched.rs) scans the
                    // per-CPU current slots and the SWITCHING_OUT marks
                    // under this same GRQ hold: a genuinely owned task is
                    // named by its CPU's slot (running / picked — the mark
                    // and slot publishes share one GRQ hold) or by a
                    // SWITCHING_OUT entry (mid switch-out, release store
                    // still pending). A marked task named by NEITHER
                    // executes nowhere and no path will ever release its
                    // marks: every pick would skip it with why=1/why=2
                    // forever (the pid-1 tail wedge: linked,
                    // {oncpu=1, claim=1, pk=1}, box idle).
                    //
                    // Organic captures (fork-storm soak, smp2, no
                    // injectors): pid 1 re-entered these shapes every few
                    // seconds for minutes — {oncpu=1, claim=-1, pk=1,
                    // st=RUNNING, slot=0, swout=0, tcpu=crossed} (a
                    // byte-granular on_cpu flip or a lost release; ti_cpu
                    // naming the OTHER cpu) and {oncpu=0, claim=foreign}
                    // (why=2). On dd5537f1 each occurrence was a
                    // potential permanent strand; with this adjudication
                    // each is swept in <=50 jiffies and the soak completes
                    // all phases.
                    let owner_live =
                        crate::sched::hotpath::pick_owner_live(task);
                    if !split_oncpu
                        && claim != -1
                        && claim as usize != cpu_id
                    {
                        // R74 (double-run gate, superseded): the old gate
                        // trusted pick_state==LIVE plus hart liveness
                        // (irq-frozen < 5s) — but LIVE legitimately
                        // persists after a switch-out and says nothing
                        // about whether the CLAIMER still owns the task.
                        // The r74a capture (reset while CPU0 was inside
                        // its pick → double-run) is now excluded
                        // STRUCTURALLY: a pick's mark→slot publish window
                        // is one continuous GRQ hold, so this scan can
                        // never interleave it; the r74a shape reads
                        // slot(claimer)==task → owner_live → refused. A
                        // {on_cpu=false, foreign claim} candidate owned by
                        // no slot and no in-flight switch is a byte-clear
                        // residue or scribble garbage — reset it.
                        if !owner_live {
                            // WAKE-FASTREPRO contradiction key: the
                            // branch condition decoded the pair from ONE
                            // atomic qword load, but the owner scan that
                            // followed takes time — the (GRQ-free) asm
                            // switch-out release can still land
                            // {false,X} -> {false,-1} under it. Re-read
                            // the qword atomically and heal ONLY if it
                            // still shows the exact observed pair: any
                            // motion means the state was a transient, not
                            // an orphan — refuse (the release already
                            // healed it).
                            let whole = crate::sched::r#unsafe::task_ownership_qword(task);
                            let w_oncpu = (whole & 0xFF) != 0;
                            let w_claim = (whole >> 32) as i32;
                            if w_oncpu || w_claim != claim {
                                // TORN VIEW: the qword moved between the
                                // split loads and this atomic re-read. The
                                // split-read pair never existed atomically
                                // — healing it would corrupt a live pick
                                // (claim-only store over {true,X}).
                                static TORN_N: core::sync::atomic::AtomicU32 =
                                    core::sync::atomic::AtomicU32::new(0);
                                if TORN_N.fetch_add(1, Ordering::Relaxed) < 40 {
                                    use crate::debug::hotpath::putc_brief as putchar;
                                    fn ph6(mut v: u64) {
                                        use crate::debug::hotpath::putc_brief as p;
                                        for sh in (0..16).rev() {
                                            let n = ((v >> (sh * 4)) & 0xF) as u8;
                                            p(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                                        }
                                    }
                                    const M: &[u8] = b"\nSTALE-CLAIM-TORN pid=";
                                    for &b in M { putchar(b); }
                                    ph6(crate::sched::r#unsafe::task_shared(task).pid() as u64);
                                    const M2: &[u8] = b" split_claim=";
                                    for &b in M2 { putchar(b); }
                                    ph6(claim.unsigned_abs() as u64);
                                    const M3: &[u8] = b" whole=0x";
                                    for &b in M3 { putchar(b); }
                                    ph6(whole);
                                    putchar(b'\n');
                                }
                                skip_reason = 2; // treat as foreign-live: refuse
                            } else {
                                crate::sched::r#unsafe::task_shared(task).running_on_cpu.store(-1, Ordering::Release);
                                stale_claim_healed(cpu_id, crate::sched::r#unsafe::task_shared(task).pid(), claim, task);
                                skip_reason = 5;
                            }
                        } else {
                            skip_reason = 2;
                        }
                        !owner_live
                    } else {
                        // why=1 leg: on_cpu set (or the claim names THIS
                        // picker's cpu). This is the legitimate mid
                        // switch-out / fast-repick transient — UNLESS no
                        // owner backs it (slotless, no SWITCHING_OUT): the
                        // release store was lost and the marks are stale
                        // forever. Time-confirm through the orphan watch
                        // (50 jiffies) so a picker that briefly appears
                        // slotless under an out-of-tree divergence window
                        // is re-recorded, then release the FULL qword —
                        // the exact store __switch_to would have made.
                        if !owner_live
                            && crate::sched::hotpath::stale_mark_confirm(
                                task,
                                crate::sched::r#unsafe::task_shared(task).pid(),
                                50,
                            )
                        {
                            // TORN re-read guard (the why=2 twin's
                            // discipline, smp8 orphan-race): the branch
                            // condition decoded the pair from ONE atomic
                            // qword load, but the owner scan and the
                            // orphan-watch confirmation that followed take
                            // time — the (GRQ-free) asm switch-out release
                            // store can still land under them, and no
                            // future divergence may be healed on evidence
                            // the qword no longer supports. Re-read the
                            // qword atomically and heal ONLY if it still
                            // shows the exact observed pair (on_cpu bit
                            // AND claim unchanged); any motion means the
                            // state was a transient, not an orphan —
                            // refuse (a real release already healed it; a
                            // fresh publish means a live owner exists and
                            // the next pass re-adjudicates).
                            let whole = crate::sched::r#unsafe::task_ownership_qword(task);
                            let w_oncpu = (whole & 0xFF) != 0;
                            let w_claim = (whole >> 32) as i32;
                            if w_oncpu != split_oncpu || w_claim != claim {
                                // TORN VIEW: the qword moved between the
                                // split read and this atomic re-read. The
                                // observed pair never existed atomically
                                // at the heal moment — releasing on it
                                // could wipe a live pick's fresh marks.
                                static TORN1_N: core::sync::atomic::AtomicU32 =
                                    core::sync::atomic::AtomicU32::new(0);
                                if TORN1_N.fetch_add(1, Ordering::Relaxed) < 40 {
                                    use crate::console::putchar_no_lock as putchar;
                                    fn ph6(mut v: u64) {
                                        use crate::console::putchar_no_lock as p;
                                        for sh in (0..16).rev() {
                                            let n = ((v >> (sh * 4)) & 0xF) as u8;
                                            p(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                                        }
                                    }
                                    const M: &[u8] = b"\nSTALE-ONCPU-TORN pid=";
                                    for &b in M { putchar(b); }
                                    ph6(crate::sched::r#unsafe::task_shared(task).pid() as u64);
                                    const M2: &[u8] = b" split_oncpu=";
                                    for &b in M2 { putchar(b); }
                                    ph6(split_oncpu as u64);
                                    const M2b: &[u8] = b" split_claim=";
                                    for &b in M2b { putchar(b); }
                                    ph6(claim.unsigned_abs() as u64);
                                    const M3: &[u8] = b" whole=0x";
                                    for &b in M3 { putchar(b); }
                                    ph6(whole);
                                    putchar(b'\n');
                                }
                                skip_reason = 1; // treat as owned/transient: refuse
                            } else {
                                let was_pk = crate::sched::r#unsafe::task_shared(task).pick_state();
                                crate::sched::r#unsafe::task_shared(task).release_pick_ownership();
                                stale_oncpu_healed(b"STALE-ONCPU-RESET", cpu_id, task, claim, was_pk);
                                skip_reason = 5;
                            }
                        } else {
                            skip_reason = 1;
                            // nanosleep-loss lane (hunt instrument): the
                            // owner_live why=1 refusal — name the candidate's
                            // qword + the owner bits (slot / SWITCHING_OUT)
                            // at DECISION time (schedtrace-gated, budgeted).
                            {
                                static WHY1_N: core::sync::atomic::AtomicU32 =
                                    core::sync::atomic::AtomicU32::new(0);
                                let stale_task = crate::sched::hotpath::pick_trace_peek(task)
                                    .is_some_and(|(_, ago)| ago > 30);
                                if crate::trace::EVENT_SCHEDTRACE.armed()
                                    && stale_task
                                    && crate::timer::get_jiffies() > 300
                                    && WHY1_N.fetch_add(
                                        1,
                                        core::sync::atomic::Ordering::Relaxed,
                                    ) < 400
                                {
                                    use crate::debug::hotpath::putc_brief as putchar;
                                    fn ph(mut v: u64) {
                                        for _ in 0..16 {
                                            let n = (v >> 60) as u8;
                                            putchar(if n < 10 {
                                                b'0' + n
                                            } else {
                                                b'a' + n - 10
                                            });
                                            v <<= 4;
                                        }
                                    }
                                    for &b in b"\nWHY1-OWNER-REFUSED pid=" {
                                        putchar(b);
                                    }
                                    let mut v = crate::sched::r#unsafe::task_shared(task).pid() as u64;
                                    for _ in 0..8 {
                                        let n = (v >> 28) as u8;
                                        putchar(if n < 10 {
                                            b'0' + n
                                        } else {
                                            b'a' + n - 10
                                        });
                                        v <<= 4;
                                    }
                                    for &b in b" qword=0x" {
                                        putchar(b);
                                    }
                                    ph(whole_qword);
                                    for &b in b" slot=0x" {
                                        putchar(b);
                                    }
                                    ph(crate::sched::hotpath::owner_slot_mask(task) as u64);
                                    for &b in b" swout=0x" {
                                        putchar(b);
                                    }
                                    ph(crate::sched::hotpath::switching_out_mask(task) as u64);
                                    for &b in b" pk=" {
                                        putchar(b);
                                    }
                                    let mut p9 = crate::sched::r#unsafe::task_shared(task).pick_state() as u64;
                                    for _ in 0..4 {
                                        let n = (p9 >> 28) as u8;
                                        putchar(b'0' + n);
                                        p9 <<= 4;
                                    }
                                    if let Some((ptc, ptj)) =
                                        crate::sched::hotpath::pick_trace_peek(task)
                                    {
                                        for &b in b" pt=" {
                                            putchar(b);
                                        }
                                        putchar(b'0' + ptc as u8);
                                        for &b in b"/ago" {
                                            putchar(b);
                                        }
                                        let mut j = ptj;
                                        let mut buf = [0u8; 12];
                                        let mut n = 0;
                                        while j > 0 && n < 12 {
                                            buf[n] = b'0' + (j % 10) as u8;
                                            j /= 10;
                                            n += 1;
                                        }
                                        if n == 0 {
                                            putchar(b'0');
                                        }
                                        while n > 0 {
                                            n -= 1;
                                            putchar(buf[n]);
                                        }
                                    }
                                    // nanosleep-loss lane: the pick→switch
                                    // handoff ledger entry (publisher cpu,
                                    // published pk, publish jiffies) — an
                                    // intact entry names the CPU whose pick
                                    // never reached its switch.
                                    if let Some((pc, ppk, pj)) =
                                        crate::sched::hotpath::psledger_peek(task)
                                    {
                                        for &b in b" psl=(cpu:" {
                                            putchar(b);
                                        }
                                        let mut v = pc as u64;
                                        for _ in 0..2 {
                                            putchar(b'0' + (v & 0xF) as u8);
                                            v >>= 4;
                                        }
                                        for &b in b",pk:" {
                                            putchar(b);
                                        }
                                        putchar(b'0' + ppk as u8);
                                        for &b in b",jif:" {
                                            putchar(b);
                                        }
                                        let mut j =
                                            crate::timer::get_jiffies().saturating_sub(pj);
                                        let mut buf = [0u8; 12];
                                        let mut n = 0;
                                        while j > 0 && n < 12 {
                                            buf[n] = b'0' + (j % 10) as u8;
                                            j /= 10;
                                            n += 1;
                                        }
                                        if n == 0 {
                                            putchar(b'0');
                                        }
                                        while n > 0 {
                                            n -= 1;
                                            putchar(buf[n]);
                                        }
                                        putchar(b')');
                                    }
                                    putchar(b'\n');
                                }
                            }
                        }
                        false
                    }
                };
                let affinity_ok = crate::sched::r#unsafe::task_shared(task).cpu_allowed(cpu_id);
                if !affinity_ok {
                    skip_reason = 3;
                }
                let throttled = crate::sched::hotpath::task_cgroup_throttled(task);
                if throttled {
                    skip_reason = 4;
                }
                affinity_ok && ownership_ok && !throttled
            };
            if allowed {
                // Found a match — unlink it (the ONLY tree mutation of
                // the walk; the successor for the loop was already
                // computed).
                rq.timeline_remove(&key, task);

                // SAFETY: task was just removed from the queue; no aliasing references exist.
                {
                    let task_ref = crate::sched::r#unsafe::task_mut(task);
                    let se = task_ref.sched_entity();
                    se.set_on_rq(false);

                    rq.nr_running_dec();
                    rq.load_weight_sub(se.load.weight);
                    // min_vruntime is updated after the walk (see the
                    // comment at update_min_vruntime below).
                }

                result = Some(task);
                rux::api::sched::ww_record(task, rux::api::sched::WW_EV_UNLINK_PICK);
                crate::sched::hotpath::pick_trace(task, 1);
                break;
            }

            // Not allowed — record the refusal and step past the
            // candidate IN PLACE (grq-10ns: no unlink, no stash).
            // s14 (msg-region strand family): PICK-SKIP-ALL names only
            // the total-failure shape (nothing pickable). A victim
            // refused AMID other candidates is invisible — yet that is
            // the msg* WAKE-STRANDED shape (100/100 strands with zero
            // skip-alls in ltp-b r2's msg region). Stamp every refusal
            // into a per-task history so the strand report (wakewatch
            // check_picked) can name WHY the victim sat unpicked for
            // the whole strand window. Armed only under rux:schedtrace (retired debug=schedtrace).
            if crate::trace::EVENT_SCHEDTRACE.armed() {
                note_pick_skip(task, skip_reason as u8);
            }
            // 63rd (strand-wall hunt): decision-time refusal evidence,
            // PERSISTENT on the task (the 61st's SKIP_HIST slots were
            // stolen by churn) and armed on the fuxstuck census boots
            // too — RUNQ-STRAND's burst sample missed the wheel
            // thread's input on ohw29/30 precisely because this only
            // armed under schedtrace.
            if crate::sched::hotpath::sched_hist_armed() {
                crate::sched::hotpath::sched_hist_refuse(task, skip_reason, cpu_id);
            }
            // Not allowed — count the refusal into the walk ring
            // (decision-time evidence for the PICK-SKIP-ALL report
            // below), then give up at the old 32-entry cap.
            // 60th (RUNQ-STRAND): the cap is NOT "very unlikely" — >32
            // simultaneously refused entries make EVERY later pick bail
            // out here, and every entry sorting above the walk window
            // (e.g. a linked, pickable, frozen censusd/samgr victim) is
            // never even VISITED while the callers walk away
            // empty-handed. Count it unconditionally; the strand census
            // reports the total.
            if skip_count < PICK_SKIP_RING_LEN {
                PICK_SKIP_RING_TASK[skip_count].store(task as usize as u64, Ordering::Relaxed);
                PICK_SKIP_RING_WHY[skip_count].store(skip_reason, Ordering::Relaxed);
                skip_count += 1;
            } else {
                PICK_STASH_OVERFLOW.fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
        PICK_SKIP_RING_N.store(skip_count, Ordering::Relaxed);

        // Advance min_vruntime over the COMPLETE timeline. grq-10ns: the
        // successor walk never lifted the refused entries out of the
        // tree, so the invariant the old stash-dance ordering restored
        // (min_vruntime computed over the full tree minus the picked
        // task — the reference's update_min_vruntime) holds here by
        // construction. min_vruntime only ever grows; it is the reference
        // point new tasks and waking sleepers are placed against
        // (enqueue_inner's sleep-compensation floor), so a drifted value
        // silently erases their sleep credit.
        if result.is_some() {
            rq.update_min_vruntime();
        }

        // GNOME-wedge canary: the timeline held candidates but NONE was
        // pickable by this CPU — the exact shape that stranded 10 linked
        // tasks in the g2 freeze (Xorg/dbus/gmain linked, every CPU
        // idle-looping on the R20-2 recheck, nothing picked for 35 min).
        // The 5s DEBUG dump shows the state but never the REASON, so name it
        // here at skip time: per task its on_cpu/claim/affinity/throttle
        // legs. Rate-limited to one report per 100 jiffies system-wide so
        // the hot idle-loop callers cannot flood the console. SBI direct
        // write discipline (R34): we hold the GRQ lock here.
        if result.is_none() && skip_count > 0 {
            PICK_SKIP_ALL.fetch_add(1, Ordering::Relaxed);
            use crate::debug::hotpath::putc_brief as putchar;
            fn pd(mut v: u64) {
                use crate::debug::hotpath::putc_brief as putchar;
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
            fn ps(s: &[u8]) {
                for &b in s {
                    crate::debug::hotpath::putc_brief(b);
                }
            }
            static LAST_REPORT: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now = crate::timer::get_jiffies();
            let last = LAST_REPORT.load(core::sync::atomic::Ordering::Acquire);
            // serial-clean: this canary fires on benign mid-switch-out
            // transients too — print only behind rux:schedtrace (retired debug=schedtrace).
            if crate::trace::EVENT_SCHEDTRACE.armed()
                && now.wrapping_sub(last) >= 100
                && LAST_REPORT
                    .compare_exchange(
                        last,
                        now,
                        core::sync::atomic::Ordering::AcqRel,
                        core::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                ps(b"\nPICK-SKIP-ALL cpu=");
                pd(cpu_id as u64);
                ps(b" skipped=");
                pd(skip_count as u64);
                ps(b"\n");
                for i in 0..skip_count {
                    let task = PICK_SKIP_RING_TASK[i].load(Ordering::Relaxed)
                        as usize as *mut crate::process::Task;
                    let reason = PICK_SKIP_RING_WHY[i].load(Ordering::Relaxed);
                    // SAFETY: diagnostic read of stable Task fields of a task
                    // that is linked on this queue (pinned by the GRQ lock we
                    // hold).
                    {
                        let tr = crate::sched::r#unsafe::task_shared(task);
                        let claim = tr.running_on_cpu.load(Ordering::Relaxed);
                        ps(b"  pid=");
                        pd(tr.pid() as u64);
                        // R76: WHY the decision refused — captured at scan
                        // time, immune to the switch-out release racing the
                        // print. The oncpu/claim/pk fields below are the
                        // print-time snapshot (kept for continuity with the
                        // older captures).
                        ps(b" why=");
                        pd(reason as u64);
                        ps(b" oncpu=");
                        putchar(b'0' + crate::sched::r#unsafe::task_shared(task).on_cpu() as u8);
                        ps(b" claim=");
                        if (claim as i64) < 0 {
                            putchar(b'-');
                        }
                        pd((claim as i64).unsigned_abs());
                        // R57: pick-state word (0=PICKED 1=LIVE 2=REVOKED) —
                        // with oncpu/claim it names the exact protocol leg
                        // that refused the candidate.
                        ps(b" pk=");
                        pd(crate::sched::r#unsafe::task_shared(task).pick_state() as u64);
                        // R85 (tail-phase wedge hunt): the ownership
                        // ADJUDICATION fields — st= the state word (the
                        // harvester only heals RUNNING orphans; a wedged
                        // waiter shows SLEEPING here), slot= the CPUs whose
                        // current slot names the task, swout= the CPUs
                        // whose in-flight switch-out names it. why=1 with
                        // slot=0 swout=0 persisting across reports is the
                        // un-owned stale mark the R85 pick-side heal
                        // releases (STALE-ONCPU-RESET).
                        ps(b" st=0x");
                        let st = crate::sched::r#unsafe::task_shared(task).state().bits();
                        let mut v = st as u64;
                        for sh in (0..8u32).rev() {
                            let nb = ((v >> (sh * 4)) & 0xF) as u8;
                            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                        }
                        ps(b" slot=");
                        pd(crate::sched::hotpath::owner_slot_mask(task) as u64);
                        ps(b" swout=");
                        pd(crate::sched::hotpath::switching_out_mask(task) as u64);
                        ps(b" aff=0x");
                        let mut v = crate::sched::r#unsafe::task_shared(task).cpus_allowed() as u64;
                        for sh in (0..8u32).rev() {
                            let nb = ((v >> (sh * 4)) & 0xF) as u8;
                            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                        }
                        if crate::sched::hotpath::task_cgroup_throttled(task) {
                            ps(b" THROTTLED");
                        }
                        putchar(b'\n');
                        // R76 (fast-pace starvation hunt): per-pid skip
                        // counting. The full3 wedge showed freshly spawned
                        // tasks named in PICK-SKIP-ALL reports for minutes;
                        // a task whose skip count crosses 1000 is either a
                        // genuine starvation victim or a decision-time
                        // transient that repeats forever — both deserve one
                        // full-context dump (state word, queue flags, stack
                        // pointers) so the post-mortem knows whether the
                        // task was linked/sleeping/phantom at skip time.
                        // Table is direct-mapped by pid (64 slots), only
                        // ever touched under the GRQ lock this pick holds.
                        {
                            use core::sync::atomic::Ordering as Rel;
                            let pid = crate::sched::r#unsafe::task_shared(task).pid();
                            let slot = (pid as usize) & (SKIP_TRACK.len() - 1);
                            let entry = &SKIP_TRACK[slot];
                            if entry.pid.load(Rel::Relaxed) != pid {
                                entry.pid.store(pid, Rel::Relaxed);
                                entry.count.store(0, Rel::Relaxed);
                                entry.dumped_at.store(0, Rel::Relaxed);
                            }
                            let count = {
                                let c = entry.count.load(Rel::Relaxed);
                                if c < u32::MAX { c + 1 } else { c }
                            };
                            entry.count.store(count, Rel::Relaxed);
                            let dumped_at = entry.dumped_at.load(Rel::Relaxed);
                            if count >= 1000 && count / 1000 > dumped_at {
                                entry.dumped_at.store(count / 1000, Rel::Relaxed);
                                ps(b"  SKIP-DEEP pid=");
                                pd(pid as u64);
                                ps(b" n=");
                                pd(count as u64);
                                ps(b" state=");
                                let st = crate::sched::r#unsafe::task_shared(task).state().bits();
                                let mut v = st as u64;
                                for sh in (0..8u32).rev() {
                                    let nb = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                                }
                                ps(b" onrq=");
                                putchar(b'0' + crate::sched::r#unsafe::task_shared(task).sched_entity().is_on_rq() as u8);
                                ps(b" ti_cpu=");
                                if (crate::sched::r#unsafe::task_shared(task).ti_cpu() as i64) < 0 {
                                    putchar(b'-');
                                }
                                pd((crate::sched::r#unsafe::task_shared(task).ti_cpu() as i64).unsigned_abs() as u64);
                                ps(b" ksp=0x");
                                let mut v = crate::sched::r#unsafe::task_shared(task).ti_kernel_sp() as u64;
                                for sh in (0..16u32).rev() {
                                    let nb = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                                }
                                ps(b" thsp=0x");
                                let mut v = crate::sched::r#unsafe::task_shared(task).thread().sp;
                                for sh in (0..16u32).rev() {
                                    let nb = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                                }
                                ps(b" vrun=");
                                pd(crate::sched::r#unsafe::task_shared(task).sched_entity().get_vruntime());
                                putchar(b'\n');
                            }
                        }
                    }
                }
            }
        }

        result
    }

pub(crate) static PICK_STASH_OVERFLOW: AtomicU64 = AtomicU64::new(0);


    /// R85 (tail-phase PICK-SKIP wedge): always-on, rate-limited report
    /// for the stale on_cpu heal (pick-side leg passes the classic tag,
    /// the tick-side CLAIM-SWEEP passes its own) — the released FULL qword is
    /// the store __switch_to would have made; the print names the victim
    /// (pid, state word, ti_cpu), the pre-heal claim, the advisory
    /// pick-state and the owner masks, so a capture can tell a lost
    /// release (slot=0 swout=0) from a lying-slot double-run fight
    /// (marks flipping between two cpus' claims). Same R34 discipline
    /// and 2s system-wide budget as the claim twin.
    pub fn stale_oncpu_healed(tag: &[u8], cpu_id: usize, task: *mut crate::process::Task, claim: i32, pk: u32) {
        crate::sched::hotpath::WW_ONCPU_HEALS.fetch_add(1, Ordering::Relaxed);
        // 63rd: the heal event goes on the task's ring BEFORE the print
        // budget can swallow the line (a budgeted-out heal must still
        // appear in the victim's history).
        if crate::sched::hotpath::sched_hist_armed() && !task.is_null() {
            rux::api::sched::sched_hist_ring(
                crate::sched::r#unsafe::task_shared(task),
                5,
                1,
                (cpu_id as u64 & 0xF) | (((claim as u64) & 0xF) << 4) | (((pk as u64) & 0xF) << 8),
            );
        }
        if !crate::sched::types::CfsRunQueue::heal_report_budget_ok() {
            return;
        }
        // SAFETY: diagnostic reads of stable Task fields; the task is
        // linked on this queue (pinned by the GRQ lock the caller holds).
        {
            let tr = crate::sched::r#unsafe::task_shared(task);
            let pid = tr.pid();
            let st = tr.state().bits();
            let tcpu = tr.ti_cpu() as i64;
            let slot = crate::sched::hotpath::owner_slot_mask(task);
            let swout = crate::sched::hotpath::switching_out_mask(task);
            crate::sched::types::CfsRunQueue::stale_heal_line(tag, cpu_id, pid, claim, pk);
            // Context tail: st= state word, tcpu= ti_cpu, slot=/swout=
            // owner masks at heal time (both must be 0 — that is the
            // adjudication's precondition; a nonzero mask here names a
            // racing owner).
            use crate::debug::hotpath::putc_brief as putchar;
            const M: &[u8] = b" st=0x";
            for &b in M {
                putchar(b);
            }
            let mut v = st as u64;
            for sh in (0..8u32).rev() {
                let nb = ((v >> (sh * 4)) & 0xF) as u8;
                putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
            }
            const M2: &[u8] = b" tcpu=";
            for &b in M2 {
                putchar(b);
            }
            if tcpu < 0 {
                putchar(b'-');
            }
            crate::sched::types::CfsRunQueue::put_dec(tcpu.unsigned_abs());
            const M3: &[u8] = b" slot=";
            for &b in M3 {
                putchar(b);
            }
            crate::sched::types::CfsRunQueue::put_dec(slot as u64);
            const M4: &[u8] = b" swout=";
            for &b in M4 {
                putchar(b);
            }
            crate::sched::types::CfsRunQueue::put_dec(swout as u64);
            putchar(b'\n');
        }
    }

    /// GNOME-wedge report for the stale-claim heal inside pick_next_cpu
    /// (R34: GRQ-held callers, direct console writes only).
    /// 63rd: `task` added for the persistent ring (the print can be
    /// budget-swallowed; the ring cannot).
    fn stale_claim_healed(cpu_id: usize, pid: u32, was: i32, task: *mut crate::process::Task) {
        crate::sched::hotpath::WW_CLAIM_HEALS.fetch_add(1, Ordering::Relaxed);
        if crate::sched::hotpath::sched_hist_armed() && !task.is_null() {
            rux::api::sched::sched_hist_ring(
                crate::sched::r#unsafe::task_shared(task),
                4,
                2,
                (cpu_id as u64 & 0xF) | (((was as u64) & 0xF) << 4),
            );
        }
        if !crate::sched::types::CfsRunQueue::heal_report_budget_ok() {
            return;
        }
        crate::sched::types::CfsRunQueue::stale_heal_line(b"STALE-CLAIM-RESET", cpu_id, pid, was, u32::MAX);
        // wake-fastrepro correlation: the victim's runtime so far (0 =
        // never scheduled: recycle-residue class) and the jiffy stamp —
        // pairs with ENQ-CLAIMED-SLOTBK jif= lines (same pid+claim).
        if !task.is_null() {
            use crate::debug::hotpath::putc_brief as putchar;
            fn pdec5(mut v: u64) {
                use crate::debug::hotpath::putc_brief as p;
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
            const M: &[u8] = b" ticks=";
            for &b in M {
                putchar(b);
            }
            pdec5(crate::sched::r#unsafe::task_shared(task).utime_ticks.load(Ordering::Relaxed)
                + crate::sched::r#unsafe::task_shared(task).stime_ticks.load(Ordering::Relaxed));
            const M2: &[u8] = b" jif=";
            for &b in M2 {
                putchar(b);
            }
            pdec5(crate::timer::get_jiffies());
            // pick→switch ledger: an intact publication record for this
            // victim = the claim came from a pick that was never
            // affirmed (switched to) NOR aborted — the switchless pick,
            // named with its publisher and age.
            if let Some((pcpu, ppk, pjif)) =
                crate::sched::hotpath::psledger_peek(task)
            {
                const A: &[u8] = b" PSLEDGER=SWITCHLESS pub_cpu=";
                for &b in A {
                    putchar(b);
                }
                pdec5(pcpu as u64);
                const B: &[u8] = b" pub_pk=";
                for &b in B {
                    putchar(b);
                }
                pdec5(ppk as u64);
                const C: &[u8] = b" age_jif=";
                for &b in C {
                    putchar(b);
                }
                pdec5(crate::timer::get_jiffies().wrapping_sub(pjif));
            }
        }
        crate::debug::hotpath::putc_brief(b'\n');
    }

    /// 64th (EEVDF seed hunt): per-visit vruntime sanity canary. Checks the
    /// ONE invariant every legit vruntime satisfies — |vr - min_vr| within
    /// VR_CANARY_SPAN_NS (see the constant's derivation) — at the four
    /// decision sites (enqueue placement, dequeue key, both pick walks).
    /// On violation prints the task, both values, the writer site, and the
    /// last 3 pick-ring events so the FIRST anomaly names the seed (the
    /// 63rd's EEVDF boot corrupted vruntimes within a minute — frozen
    /// 0xfd79-class values, init unpicked from t=53s — with no instrument
    /// to localize the write). Armed on rux:fuxstuck (retired debug=fuxstuck)|schedtrace (same gate
    /// as the pick/wake history); zero cost otherwise (one switch load).
    /// GRQ-held callers only; prints via the taskdump brief-budget funnel (R34 discipline, obs-hygiene: staged line-atomic writes).
    pub fn vr_canary(rq: &mut crate::sched::types::CfsRunQueue, task: *mut crate::process::Task, site: &[u8; 3], vr: u64) {
        if !crate::sched::hotpath::sched_hist_armed() {
            return;
        }
        let min_vr = rq.get_min_vruntime();
        let up = vr.saturating_sub(min_vr);
        let dn = min_vr.saturating_sub(vr);
        if up <= VR_CANARY_SPAN_NS && dn <= VR_CANARY_SPAN_NS {
            return;
        }
        let trip = VR_TRIPS.fetch_add(1, Ordering::Relaxed) + 1;
        let now = crate::timer::get_jiffies();
        // First 8 trips always print (the seed hunt needs the first-bad
        // write); afterwards one per 300 jiffies so a corruption storm
        // cannot flood the serial console.
        let last = VR_TRIP_LAST_JIF.load(Ordering::Acquire);
        if trip > 8
            && now.wrapping_sub(last) < 300
            && VR_TRIP_LAST_JIF
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        VR_TRIP_LAST_JIF.store(now, Ordering::Release);
        use crate::debug::hotpath::putc_brief as putchar;
        fn ph(mut v: u64) {
            use crate::debug::hotpath::putc_brief as putchar;
            putchar(b'0');
            putchar(b'x');
            let mut buf = [0u8; 16];
            let mut n = 0;
            while n < 16 {
                buf[n] = (v & 0xF) as u8;
                v >>= 4;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                putchar(if buf[n] < 10 { b'0' + buf[n] as u8 } else { b'a' + buf[n] as u8 - 10 });
            }
        }
        fn pd(mut v: u64) {
            use crate::debug::hotpath::putc_brief as putchar;
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
        fn ps(s: &[u8]) {
            for &b in s {
                crate::debug::hotpath::putc_brief(b);
            }
        }
        ps(b"\nVRTRIP site=");
        ps(site);
        ps(b" trip=");
        pd(trip);
        if !task.is_null() {
            // SAFETY: diagnostic read of a Task the caller holds under the
            // GRQ (linked on this queue, or being placed by enqueue).
            let t = crate::sched::r#unsafe::task_shared(task);
            ps(b" tid=");
            pd(t.pid() as u64);
            let comm = t.comm();
            ps(b"(");
            let mut j = 0;
            while j < comm.len() && comm[j] != 0 && j < 12 {
                putchar(comm[j]);
                j += 1;
            }
            ps(b")");
        }
        ps(b" vr=");
        ph(vr);
        ps(b" min=");
        ph(min_vr);
        if up > VR_CANARY_SPAN_NS {
            ps(b" up=");
            pd(up / 1_000_000);
            ps(b"ms");
        } else {
            ps(b" dn=");
            pd(dn / 1_000_000);
            ps(b"ms");
        }
        ps(b" j=");
        pd(now);
        // The last 3 pick-ring events (63rd instrument; entry layout
        // kind:4 | why:8 | jiffies:30 | aux:22).
        if !task.is_null() {
            // SAFETY: as above.
            let t = crate::sched::r#unsafe::task_shared(task);
            let seq = t.pick_ring_seq.load(Ordering::Relaxed);
            ps(b" ring=");
            for k in 1..=3u64 {
                let idx = (seq.wrapping_sub(k) & 15) as usize;
                let v = t.pick_ring[idx].load(Ordering::Relaxed);
                let kind = (v >> 60) & 0xF;
                let why = (v >> 52) & 0xFF;
                let jif = (v >> 22) & 0x3FFF_FFFF;
                putchar(match kind {
                    1 => b'L',
                    2 => b'R',
                    3 => b'P',
                    4 | 5 => b'H',
                    _ => b'?',
                });
                putchar(b'0' + (why & 0xF) as u8);
                putchar(b'@');
                pd(jif);
                putchar(b' ');
            }
            ps(b"pk=");
            pd(t.pick_n.load(Ordering::Relaxed));
            ps(b" wk=");
            pd(t.wake_n.load(Ordering::Relaxed));
        }
        putchar(b'\n');
    }

// WW_PICK_WALKS / WW_ONCPU_HEALS / WW_CLAIM_HEALS moved FRAMEWORK-side
// (devirt lane; sched/entities.rs) — same counters, same stores; this
// file reaches them through crate::sched::hotpath::entities.

/// 64th: vruntime sanity canary state (VRTRIP rate limiting).
static VR_TRIPS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

static VR_TRIP_LAST_JIF: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// 64th (EEVDF seed hunt): how far a virtual runtime may legitimately sit
/// from min_vruntime, in ns-of-virtual-time. The extreme legitimate case is
/// a nice-19 CPU hog (weight 15 vs NICE_0 1024, vr grows 68x wall) running
/// the whole boot: 600 s wall * 68 = 4.1e13. Everything past 1e15 is
/// corruption-class — ohw36's frozen garbage was ~1e19 (u64 views of a
/// truncated i128). Trips on |vr - min_vr| beyond this bound.
const VR_CANARY_SPAN_NS: u64 = 1_000_000_000_000_000;
