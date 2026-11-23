//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Scheduler Implementation — Global RunQueue Design
//!
//! # Architecture
//!
//! One global run queue (GRQ) shared by all CPUs, with per-class sub-queues:
//!   - DL queue (BTreeMap, EDF)
//!   - RT queue (bitmap + per-priority lists)
//!   - CFS queue (BTreeMap, vruntime)
//!
//! Per-CPU state is minimal: just `current`, `idle`, `stop` task pointers.
//!
//! # Scheduling Flow
//!
//!   schedule() → lock GRQ → pick_next_task() → unlock GRQ → context_switch()
//!
//! No per-CPU load balancing or stealing is needed — the global queue is
//! inherently balanced.  CPUs pull tasks on demand; idle CPUs are woken via IPI.
//!
//! # Note
//!
//! Raw pointers are used to avoid borrow checker limitations, which is common
//! practice in OS kernel development.

use super::entities::W2RefusalCensus;
use super::sched_hooks::{SLOT_W_SCHED_PICK, SLOT_W_R56D, SLOT_W_R56C, SLOT_W_STEAL, SLOT_W_ABORT_REPUB, WW_EV_WAKE_OK, WW_EV_LINK, WW_EV_UNLINK_PICK, WW_EV_UNLINK_SCHED, WW_EV_UNLINK_DEQ, WW_EV_UNLINK_POISON, WW_EV_PICK_PUB, WW_EV_WAKE_REFUSED, WW_EV_AFFIRM};
use super::class::SchedClass;

// ==================== Core wake/enqueue/current family ====================
// (Back-carve from the policy half, 2026-11-22 S1b pre-work: the design
// keeps these CORE — tp/current derivation, context_switch, the
// NEED_RESCHED atomics, resched IPIs, and the GRQ intrusive-link
// enqueue/dequeue API the policy calls into. They carry zero unsafe
// either way; placement is what the S1b crate boundary needs.)

#[inline]
pub fn need_resched() -> bool {
    // SAFETY: cpu_id is bounds-checked against MAX_CPUS before array access;
    // NEED_RESCHED elements are AtomicBool, safe for concurrent reads.
    {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id >= MAX_CPUS {
            return false;
        }
        NEED_RESCHED[cpu_id].load(core::sync::atomic::Ordering::Acquire)
    }
}

#[inline]
pub fn set_need_resched() {
    // The static is a plain Sync array of AtomicBool (unsafe-slim wave 5):
    // only the current CPU writes its own entry.
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    if cpu_id < MAX_CPUS {
        NEED_RESCHED[cpu_id].store(true, core::sync::atomic::Ordering::Release);
    }
}

#[inline]
pub(crate) fn clear_need_resched() {
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    if cpu_id < MAX_CPUS {
        NEED_RESCHED[cpu_id].store(false, core::sync::atomic::Ordering::Release);
    }
}

pub fn resched_curr() {
    set_need_resched();
}

/// Send reschedule IPI to target CPU
pub fn resched_cpu(cpu: usize) {
    // cctx probe: the IPI send itself (APIC write cost, waker-side).
    let cc_t = crate::debug::cctx::now();
    crate::debug::cctx::bump(&crate::debug::cctx::IPI_N);
    // SAFETY: cpu is bounds-checked against MAX_CPUS; NEED_RESCHED[cpu] is AtomicBool.
    {
        if cpu < MAX_CPUS {
            NEED_RESCHED[cpu].store(true, core::sync::atomic::Ordering::Release);
            let this_cpu = crate::arch::cpu_id() as usize;
            // closegap: skip the IPI when the target is in its polling-idle
            // window — it observes NEED_RESCHED/nr_running itself within
            // tens of ns (the IPI + halt-exit costs ~2-5 us of KVM exit
            // latency per handoff; cctx probe measured 935-1139 ns for the
            // waker-side send alone). Lost-wake safety (Dekker pairing
            // with the idle loop's flag-clear + final recheck): the
            // SeqCst fence between our NEED_RESCHED store and the flag
            // load, mirrored by the idle loop's fence between its
            // flag-clear store and its recheck loads, guarantees at least
            // one side observes the other — either we see the flag clear
            // and send the IPI, or the idler's recheck sees
            // NEED_RESCHED and skips the halt.
            if this_cpu != cpu {
                core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
                if !POLLING_IDLE[cpu].load(core::sync::atomic::Ordering::Acquire) {
                    crate::arch::ipi::send_reschedule_ipi(cpu);
                }
            }
            // RT-2X item-7 hunt: name every poke decision for the wedge
            // boot (cmdline rt2xdbg=1; cpu-0-targeted only, 200-line
            // budget — self-pokes flood the log): target, sender,
            // polling flag, and the target's current task (pid/policy).
            if cpu == 0
                && crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1")
            {
                static RSP: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if RSP.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 200 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn ph(mut v: u64) {
                        for _ in 0..16 {
                            let n = (v >> 60) as u8;
                            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                            v <<= 4;
                        }
                    }
                    let cur = cpu_state(cpu).current;
                    let (cpid, cpolicy) = if cur.is_null() {
                        (0xDEADu64, 9u64)
                    } else {
                        (crate::sched::edges::task_shared(cur).pid() as u64,
                         crate::sched::edges::task_shared(cur).policy() as u64)
                    };
                    let poll = POLLING_IDLE[cpu].load(core::sync::atomic::Ordering::Acquire);
                    for &b in b"RESCHED-CPU tgt=" { putchar(b); }
                    ph(cpu as u64);
                    for &b in b" from=" { putchar(b); }
                    ph(this_cpu as u64);
                    for &b in b" cur=0x" { putchar(b); }
                    ph(cpid);
                    for &b in b" pol=" { putchar(b); }
                    ph(cpolicy);
                    for &b in b" polling=" { putchar(b); }
                    ph(poll as u64);
                    putchar(b'\n');
                }
            }
        }
    }
    crate::debug::cctx::add(&crate::debug::cctx::IPI_NS, cc_t);
}

pub fn wake_up_process(task: *mut Task) -> bool {
    use crate::process::Task;
    Task::wake_up(task)
}

/// WF_SYNC wake (66th session, binder sync-txn/reply parity): same
/// transition as wake_up_process; the dispatch stacks the wakee with the
/// waker (see Task::wake_up_impl's sync leg).
pub fn wake_up_process_sync(task: *mut Task) -> bool {
    use crate::process::Task;
    Task::wake_up_sync(task)
}

/// Enqueue a task and try to wake an idle CPU.
///
/// R52 (fork enqueue gap): returns whether the task is actually linked
/// now (the GRQ-locked insert's own verdict). For a freshly constructed
/// task every legitimate guard passes — a `false` here is a guard
/// divergence (poison, dead-state, or a stale class on_rq flag landing
/// on a never-enqueued task) and the caller MUST NOT report creation
/// success: the child would be a RUNNING-looking task on no queue and
/// no CPU that nothing will ever schedule (the form-A phantom), while
/// its parent waits on a PID that never runs. fork.rs and kthread.rs
/// unwind the child on `false`; the task's state stays TASK_NEW on
/// refusal (R39 discipline: no phantom RUNNING is manufactured).
pub fn enqueue_task(task: &'static mut Task) -> bool {
    let task_ptr = task as *mut Task;
    let cpus_allowed = task.cpus_allowed();

    let this_cpu = crate::arch::cpu_id() as usize;

    // Assign CPU if unassigned
    if task.ti_cpu() as usize >= MAX_CPUS {
        task.set_ti_cpu(this_cpu as i32);
    }

    // Lock GRQ and enqueue
    let mut grq_guard = grq().lock_irqsave_site(grq_site(GRQ_CALLER_ENQUEUE, GRQ_STEP_TRY));
    // SAFETY: GRQ lock is held via grq_guard; enqueue_task_locked expects the lock to be held.
    let inserted = enqueue_task_locked(&mut *grq_guard, crate::sched::edges::task_mut(task_ptr));

    // Check for cross-CPU preemption (RT/DL direct; CFS via wakeup granularity)
    if inserted {
        let policy = task.policy();
        if policy == SchedPolicy::Fifo || policy == SchedPolicy::Rr {
            check_rt_preempt(task_ptr, cpus_allowed, task.ti_cpu());
        } else if policy == SchedPolicy::Deadline {
            check_dl_preempt(task_ptr, cpus_allowed, task.ti_cpu());
        } else {
            // Normal / Batch / Idle
            check_cfs_preempt(task_ptr, cpus_allowed);
        }
    }

    drop(grq_guard);

    if !inserted {
        // Nothing was linked — do not spend resched IPIs on a task that
        // is not runnable.
        return false;
    }

    // Try to wake an idle CPU
    if let Some(idle_cpu) = grq().find_idle_cpu(cpus_allowed) {
        grq().clear_idle(idle_cpu);
        resched_cpu(idle_cpu);
    } else if cpus_allowed & (1u32 << this_cpu) == 0 {
        // Task can't run on this CPU — IPI a CPU that can
        let target = cpus_allowed.trailing_zeros() as usize;
        if target < MAX_CPUS && target != this_cpu {
            resched_cpu(target);
        }
    }

    true
}

/// R33 (A-family wedge — dead-task resurrection): atomically, under the GRQ
/// lock, re-verify that `task` is still in a wakeable state (sleeping or
/// stopped), transition it to RUNNING and enqueue it.
///
/// `Task::wake_up` filters on an UNLOCKED state read. Between that read and
/// the old unlocked set_state(RUNNING)+enqueue_task, the target could be
/// woken by another CPU, run to completion, pass do_exit (set ZOMBIE +
/// dequeue + final schedule) and be reaped by its parent — release_task
/// frees the kernel stack BEFORE the pinned task_put, so the stale wake
/// would then resurrect a freed-stack task onto the runqueue. Holding the
/// GRQ lock across re-check + transition + enqueue closes the window:
///   - a task still INTERRUPTIBLE/STOPPED cannot exit while we hold the
///     lock (to reach do_exit it must first be woken and scheduled, and
///     both the wake transition and the pick happen under this lock);
///   - a task that already exited reads ZOMBIE/DEAD here and is refused
///     (enqueue_task_locked's dead-guard double-checks before its
///     set_state(RUNNING)).
/// Returns true if the task was transitioned and enqueued.
pub fn wake_up_enqueue(task: *mut Task) -> bool {
    wake_up_enqueue_opt(task, None)
}

/// INV-LOCK-5 load-bearing twin: wake while a futex bucket lock is
/// held (futex_wake's R12-2 section). The proof is required by
/// `lock_irqsave_nested`, which only exists for the
/// GrqKey-inside-FutexBucketKey impl — the nesting that used to be a
/// comment is now a compile fact.
pub fn wake_up_enqueue_nested<'a>(
    task: *mut Task,
    proof: &crate::sync::lockkey::NestProof<'a, crate::sync::lockkey::FutexBucketKey>,
) -> bool {
    wake_up_enqueue_opt(task, Some(proof))
}

/// Shared body of the wake-up enqueue entry points. `nested` selects
/// the proof-checked GRQ acquire (one predictable branch; the proof
/// itself is zero-sized). Public for `Task::wake_up_impl`'s call sites
/// (task.rs).
pub fn wake_up_enqueue_opt(
    task: *mut Task,
    nested: Option<&crate::sync::lockkey::NestProof<'_, crate::sync::lockkey::FutexBucketKey>>,
) -> bool {
    if task.is_null() {
        return false;
    }
    let mut grq_guard = match nested {
        Some(proof) => grq().lock_irqsave_nested_site(proof, grq_site(GRQ_CALLER_WAKE, GRQ_STEP_TRY)),
        None => grq().lock_irqsave_site(grq_site(GRQ_CALLER_WAKE, GRQ_STEP_TRY)),
    };
    // SAFETY: task is non-null and validated by the caller (Task::wake_up's
    // poison check plus an unlocked wakeable-state filter, or a pinned
    // pid-hash lookup); the GRQ lock is held across the state transition so
    // the re-check and the enqueue are one atomic step.
    {
        // INV-SCE-2: task is non-null and validated by the caller (see the
        // comment above); the GRQ hold makes the re-check and the enqueue
        // one atomic step. task_t keeps the raw `task` usable for the
        // queue/ledger helpers on pointer currency.
        let task_t = crate::sched::edges::task_shared(task);
        let st = task_t.state();
        // PID-0 tasks (per-CPU idles, the boot init's idle) are RUNNING by
        // design and are owned by their own cpu's idle loop — no wake path
        // ever legitimately targets one. The idle task of an OFFLINE cpu
        // presents exactly the R46 phantom shape (RUNNING, unlinked, owned
        // by no current slot) forever, so the heal below would re-link it
        // onto the CFS timeline (unit-test capture:
        // "R46-RUNNING-PHANTOM healed pid=0x0 ... cpu=3" on a 2-cpu boot)
        // only for the pick-side PID-0 gates to drop it again. Refuse.
        if crate::sched::edges::task_shared(task).pid() == 0 {
            return false;
        }
        if !(st.is_sleeping() || st.contains(TaskState::STOPPED)) {
            // RUNNING (woken elsewhere / never slept), ZOMBIE, or DEAD —
            // the wake is stale or redundant; drop it. (Freed-page poison
            // keeps bit0 set so it passes is_sleeping(); it is caught one
            // step later by the pid poison check — and the dead-guard —
            // inside enqueue_task_locked.)
            //
            // R46 (B-form phantom — the refused-wake mirror of R41): RUNNING
            // is legitimate ONLY while the task is on a CPU (on_cpu, set at
            // pick, cleared by __switch_to once the outgoing context is
            // saved) or linked on a class queue. __schedule's discipline
            // makes every context switch-out leave prev either
            // (RUNNING ∧ linked) or (¬RUNNING ∧ unlinked), and every RUNNING
            // write in the tree is paired with an insert or with
            // on-CPU-ness — so RUNNING ∧ !on_cpu ∧ unlinked is NOT
            // producible by any interleaving of the scheduler itself. It
            // arrives from outside (a zero/write into the state word by the
            // documented Task-struct smash families — note RUNNING == 0 —
            // or a future regression), and because every upstream wake
            // filter refuses a RUNNING target, nothing would ever schedule
            // the task again: no queue holds it, no wake reaches it
            // (round-46 capture: pid 335, state=RUNNING, on_rq=0, 4 CPUs
            // wfi-idle, stack frozen in trap_exit -> asm_need_resched ->
            // schedule -> __schedule -> context_switch). Verify ACTUAL
            // linkage (never trust the flag), report, and fall through to
            // the insert below, which links it; a co-existing stale
            // on_rq=true is healed by the R41 branch this falls into, so
            // both mirror phantoms now heal at the same point.
            //
            // R49 (on_cpu clear-protocol gap — the orphaned pick mark): a
            // pick mark is OWNED by the PER_CPU slot it was booked into
            // (__schedule writes current = next under this same GRQ lock,
            // before releasing it), so under the lock "is on a CPU" is
            // decidable by scanning the current slots — no CPU can be
            // mid-pick while we hold the lock, and every completed pick
            // has already published current == picked. A RUNNING task that
            // is unlinked AND owned by no CPU's current slot carries an
            // ORPHANED mark: the mark's CPU fast-path-returned or resolved
            // prev from a diverged PER_CPU slot (the R49 steer TOCTOU
            // cascade), so no __switch_to will ever run with it as prev
            // and clear the mark — while the class picks (fair/rt/dl)
            // REFUSE on_cpu-marked tasks and every wake route refused it
            // (round-49 capture: pid 344/345/356, RUNNING, on_cpu=1,
            // linked=0, on_rq=0, all CPUs wfi). The on_cpu read is
            // therefore no longer part of the phantom predicate; the
            // current-slot scan is authoritative. An orphaned mark must be
            // cleared BEFORE the re-link below, or the freshly linked task
            // remains unpickable (the pick skip treats a queued on_cpu
            // task as "context not saved yet").
            let linked = match crate::sched::edges::task_shared(task).policy() {
                SchedPolicy::Fifo | SchedPolicy::Rr => grq_guard.rt_rq.is_linked(task),
                SchedPolicy::Deadline => grq_guard.dl_rq.is_linked(task),
                SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                    grq_guard.cfs_rq.is_linked(task)
                }
            };
            // R56c (slot-capture orphan): a slot that names this task while
            // its CPU is IDLE-MARKED is lying. In-tree the combination is
            // grq-lock-1st: checkpoint before the current-slot scan (the
            // R56c lying-slot heal).
            grq_site_mark_at(
                crate::arch::cpu_id() as usize,
                grq_site(GRQ_CALLER_WAKE, GRQ_STEP_CLASS_OP),
            );
            // unconstructible: a pick publishes current=next and clears its
            // idle bit BEFORE releasing this lock, and mark_idle runs only
            // from the idle loop with the slot naming the idle task
            // (current.pid == 0), so no interleaving of the scheduler itself
            // can leave a real task named in an idle-marked slot. Reaching
            // this state means the pick that published the slot never
            // reached its context switch (the orphaned-mark capture family).
            // Heal the lying slots (rewrite them to the idle task — a wedged
            // CPU resolving prev from its slot would otherwise re-queue and
            // re-pick the orphan forever without ever running it) and treat
            // the task as unowned. A BUSY slot that names the task keeps
            // refusing the wake exactly as before: it is either genuinely
            // running it or mid-switch-to-it, and the idle-loop harvester's
            // time-confirmed steal (harvest_relink) owns that shape.
            let mut curr_on = false;
            for c in 0..MAX_CPUS {
                if cpu_state(c).current == task {
                    let slot_idle = grq()
                        .idle_cpus
                        .load(core::sync::atomic::Ordering::Acquire)
                        & (1u32 << c)
                        != 0;
                    if !slot_idle {
                        curr_on = true;
                    } else {
                        let idle_task = cpu_state(c).idle;
                        if !idle_task.is_null() && idle_task != task {
                            cpu_state_mut(c).current = idle_task;
                            slot_tag_write(c, SLOT_W_R56C, idle_task);
                            // r11: the healed-away pick never reached its
                            // switch — supersede its generation.
                            pick_gen_supersede(c, idle_task);
                        }
                    }
                }
            }
            // 119th (epoll-pwait conviction — WAKE side): the
            // contradiction-keyed guard, NARROWED by measurement.
            //
            // MEASURED FACT (119th, boot 3/4/5 PARAM-INIT + PARAM-SENDER
            // dumps): `pk == PICK_LIVE` is NOT a contradiction — it is the
            // NORMAL value for every task that has ever run (the pick
            // publishes LIVE at the switch-in affirm and nothing resets
            // it on the way out; all 20+ sampled tasks — sleeping OS_IPC
            // workers included — carry pk=1). A woken-but-not-yet-picked
            // task therefore always shows {pk=1, on_cpu=0, claim=-1} and
            // is still PICKABLE (ownership_pickable: !oncpu && claim==-1
            // → true), so clearing that mark would be a wake-path state
            // write on EVERY wake with no strand to remove — the r74a
            // double-run recipe for nothing.
            //
            // The shape that IS a strand: a RUNNING task with a LIVE
            // mark that NO run queue holds (linked == false) and no CPU
            // owns (no slot names it, no SWITCHING_OUT mark, claim==-1,
            // on_cpu==0). Such a task is on no timeline → no pick can
            // ever take it → its wake is refused forever. Under this GRQ
            // hold no pick can be in flight (a pick publishes the mark
            // and the slot in ONE hold), so the combination cannot be a
            // transient. Clear the stale mark so the pick-skip protocol
            // cannot veto it, and let the insert below link it.
            {
                let pk = crate::sched::edges::task_shared(task).pick_state();
                let claim = crate::sched::edges::task_shared(task).running_on_cpu.load(core::sync::atomic::Ordering::Relaxed);
                let mid_sw = task_is_mid_switch_out(task);
                if !linked
                    && pk == crate::process::task::PICK_LIVE
                    && !curr_on
                    && !mid_sw
                    && claim == -1
                    && !crate::sched::edges::task_shared(task).on_cpu()
                {
                    crate::sched::edges::task_shared(task).release_pick_ownership();
                    crate::sched::edges::task_shared(task).ti_pick_state.store(
                        crate::process::task::PICK_PICKED,
                        core::sync::atomic::Ordering::Release,
                    );
                    // Rule 1 (grq-lock-1st): event ring, not the console.
                    crate::sched::sched_hooks::sched_ev(
                        crate::sched::sched_hooks::EV_R119_STALE_LIVE,
                        crate::sched::edges::task_shared(task).pid(),
                        0,
                        0,
                    );
                }
            }
            // Not RUNNING-and-unowned: ZOMBIE/DEAD/TASK_NEW, already linked,
            // or some (live) CPU's current — refuse exactly as before.
            if st != TaskState::new(TaskState::RUNNING) || linked || curr_on {
                // sc99: this refusal was silent, and the binder-stall
                // WAKE-RUN-LEAK family shows tasks parked in exactly the
                // refused shapes for minutes while REKICKs keep arriving —
                // name the refusing guard so the next capture says which
                // condition ate the wake (curr_on = a lying busy slot).
                // PROBE-LOCK DISCIPLINE (the C-line ABAA conviction): this
                // branch runs under the GRQ irqsave lock, and printk ->
                // write_to_ring_buffer -> KMSG_READ_WQ.wake_up_all() ->
                // wake_up_process -> wake_up_enqueue re-takes the GRQ: a
                // same-CPU self-deadlock (IRQs off) whenever a kmsg reader
                // is parked, a cross-CPU GRQ<->waitqueue ABAA otherwise.
                // Capture every print field into locals here, drop the
                // guard at the branch tail, then printk. Same line formats
                // as before (the W3 census greps stay stable).
                static W3_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                // Budget guard (brkseq repro, leg-20): a refusal of a
                // RUNNING task that a non-idle slot legitimately names
                // (a wake racing the task's own execution — the brkseq
                // death window produced 4+ per SIGSEGV child) is the
                // BENIGN form of curr_on; printed unconditionally it ate
                // the whole 32-line budget in 8 deaths and blinded the
                // census for the shapes it exists for. The pathological
                // RUNNING+curr_on twin is exactly what the frozen-exec
                // conviction below distinguishes, so defer this shape's
                // plain refusal to the W3-FROZEN-EXEC-SLOT line and
                // spend the budget on everything else (sleeping-state
                // refusals, dead/zombie targets, linked-but-starved).
                let w3_benign_running =
                    st == TaskState::new(TaskState::RUNNING) && curr_on;
                let mut w3_print_refused = false;
                // Serial-cleanliness: the W3 refusal census joins the
                // wwcensus family — ordinary boots stay clean.
                if !w3_benign_running
                    && crate::trace::EVENT_WWCENSUS.armed()
                    && crate::timer::get_jiffies() > 300
                {
                    w3_print_refused =
                        W3_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 400;
                }
                let w3_pid = crate::sched::edges::task_shared(task).pid();
                let w3_st_bits = st.bits();
                let w3_on_cpu = crate::sched::edges::task_shared(task).on_cpu();
                let w3_pk = crate::sched::edges::task_shared(task).pick_state();
                let w3_cpu = crate::sched::edges::task_shared(task).ti_cpu();
                // nanosleep-loss lane (hunt instrument, printed after the
                // guard drop below): the linked-true refusal is the freeze's
                // visible face — capture the ownership qword, the stored
                // timeline key, on_rq, and the timeline's head rows so one
                // boot adjudicates "node in tree but skipped" (vr position
                // + wall) against "stale key, node unreachable" (the walk
                // never lists the pid).
                let w3_qword = crate::sched::edges::task_ownership_qword(task);
                let w3_onrq = crate::sched::edges::task_shared(task).sched_entity().on_rq.load(core::sync::atomic::Ordering::Relaxed);
                let (w3_key_vr, w3_key_id) =
                    match crate::sched::edges::task_shared(task).sched_entity().timeline_key() {
                        Some(k) => (k.vruntime, k.task_id),
                        None => (u64::MAX, u64::MAX),
                    };
                let w3_cfs_nr = grq_guard.cfs_rq.nr_running();
                let mut w3_tree: [(u32, u64, bool, i32); 6] = [(0, 0, false, 0); 6];
                let w3_tree_total =
                    grq_guard.cfs_rq.timeline_head_diag(&mut w3_tree);
                // nanosleep-loss lane: the victim's preempt_count and saved
                // kernel context (switch-out retaddr + first stack words)
                // — names the loop a spin-shaped freeze sits in.
                let w3_pc = crate::sched::edges::task_shared(task).preempt_count();
                #[cfg(feature = "x86_64")]
                let (w3_ra, w3_sp, w3_w) = {
                    let sp = crate::sched::edges::task_shared(task).thread.sp as *const u64;
                    let mut ws = [0u64; 24];
                    if sp as usize > 0x1000 {
                        for k in 0..24 {
                            ws[k] = crate::sync::edges::read_volatile_u64((sp as usize) + 8 * k);
                        }
                    }
                    (crate::sched::edges::task_shared(task).thread.callee.ret_addr as u64, sp as u64, ws)
                };
                #[cfg(not(feature = "x86_64"))]
                let (w3_ra, w3_sp, w3_w) = {
                    let sp = crate::sched::edges::task_shared(task).thread.sp as *const u64;
                    let mut ws = [0u64; 24];
                    if sp as usize > 0x1000 {
                        for k in 0..24 {
                            ws[k] = crate::sync::edges::read_volatile_u64((sp as usize) + 8 * k);
                        }
                    }
                    (crate::sched::edges::task_shared(task).thread.ra as u64, sp as u64, ws)
                };
                // Frozen-slot capture (filled by the W3S block below; read
                // out after the guard drop).
                let mut w3_fz_ready = false;
                let mut w3_fz_naming = MAX_CPUS;
                let mut w3_fz_exec: u64 = 0;
                let mut w3_fz_djif: u64 = 0;
                let mut w3_fz_retaddr: u64 = 0;
                let mut w3_fz_sp: u64 = 0;
                let mut w3_fz_w: [u64; 4] = [0; 4];
                // sc184 (session R, lane 1 wake-loss): the perf-audit census
                // splits the residual WAKE-RUN-LEAK family into "lying busy
                // slots" (curr_on refusal) and "linked but starved". A lying
                // busy slot is convicted by PROGRESS: a CPU genuinely
                // executing the named task advances its sum_exec_runtime; a
                // slot that keeps naming a task whose exec clock is FROZEN
                // (and which stays RUNNING+unlinked across repeated kicks)
                // is stale bookkeeping nobody will ever clear. Record
                // (task, sum_exec_runtime, jiffies) per refusal and print
                // the frozen-clock signature when it matures. The saved
                // switch retaddr + first stack words discriminate the
                // legitimate userspace-spinner shape offline.
                if curr_on && !linked && st == TaskState::new(TaskState::RUNNING) {
                    const W3S: usize = 8;
                    static W3_TASK: [core::sync::atomic::AtomicUsize; W3S] =
                        [const { core::sync::atomic::AtomicUsize::new(0) }; W3S];
                    static W3_EXEC: [core::sync::atomic::AtomicU64; W3S] =
                        [const { core::sync::atomic::AtomicU64::new(0) }; W3S];
                    static W3_JIF: [core::sync::atomic::AtomicU64; W3S] =
                        [const { core::sync::atomic::AtomicU64::new(0) }; W3S];
                    let jif = crate::timer::get_jiffies();
                    let exec = crate::sched::edges::task_shared(task)
                        .sched_entity()
                        .sum_exec_runtime
                        .load(core::sync::atomic::Ordering::Relaxed);
                    let key = task as usize;
                    for s in 0..W3S {
                        let prev = W3_TASK[s].load(core::sync::atomic::Ordering::Relaxed);
                        if prev == key {
                            let prev_exec =
                                W3_EXEC[s].load(core::sync::atomic::Ordering::Relaxed);
                            let prev_jif =
                                W3_JIF[s].load(core::sync::atomic::Ordering::Relaxed);
                            W3_EXEC[s].store(exec, core::sync::atomic::Ordering::Relaxed);
                            W3_JIF[s].store(jif, core::sync::atomic::Ordering::Relaxed);
                            if exec == prev_exec && jif > prev_jif + (crate::config::KERNEL_HZ as u64) * 2 {
                                let mut naming = MAX_CPUS;
                                for c in 0..MAX_CPUS {
                                    if cpu_state(c).current == task {
                                        naming = c;
                                        break;
                                    }
                                }
                                let sp = crate::sched::edges::task_shared(task).thread.sp;
                                // Capture (task fields + stack words) while
                                // the GRQ still pins the task; print after
                                // the guard drop.
                                w3_fz_naming = naming;
                                w3_fz_exec = exec;
                                w3_fz_djif = jif - prev_jif;
                                #[cfg(feature = "x86_64")]
                                {
                                    w3_fz_retaddr =
                                        crate::sched::edges::task_shared(task).thread.callee.ret_addr as u64;
                                }
                                #[cfg(not(feature = "x86_64"))]
                                {
                                    w3_fz_retaddr = crate::sched::edges::task_shared(task).thread.ra as u64;
                                }
                                w3_fz_sp = sp as u64;
                                w3_fz_w[0] =
                                    crate::sync::edges::read_volatile_u64(sp as usize);
                                w3_fz_w[1] = crate::sync::edges::read_volatile_u64((sp as usize) + 8);
                                w3_fz_w[2] = crate::sync::edges::read_volatile_u64((sp as usize) + 16);
                                w3_fz_w[3] = crate::sync::edges::read_volatile_u64((sp as usize) + 24);
                                w3_fz_ready = true;
                            }
                            break;
                        }
                        if prev == 0
                            && W3_TASK[s]
                                .compare_exchange(
                                    0,
                                    key,
                                    core::sync::atomic::Ordering::Relaxed,
                                    core::sync::atomic::Ordering::Relaxed,
                                )
                                .is_ok()
                        {
                            W3_EXEC[s].store(exec, core::sync::atomic::Ordering::Relaxed);
                            W3_JIF[s].store(jif, core::sync::atomic::Ordering::Relaxed);
                            break;
                        }
                    }
                }
                // Drop the GRQ BEFORE printk (PROBE-LOCK DISCIPLINE above):
                // the refusal decision is complete and every field was
                // captured under the lock, so release-then-print cannot
                // change the outcome; the printk wake path may then take
                // the GRQ freely.
                drop(grq_guard);
                if w3_print_refused {
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                    crate::pr_warn!(
                        "W3-RUNWAKE-REFUSED pid={} state={:#x} linked={} curr_on={} on_cpu={} pk={} cpu={}",
                        w3_pid,
                        w3_st_bits,
                        linked,
                        curr_on,
                        w3_on_cpu,
                        w3_pk,
                        w3_cpu
                    );
                    // nanosleep-loss lane: the same refusal with the
                    // linkage/ownership anatomy (see the capture above).
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                    crate::pr_warn!(
                        "W3-ANATOMY pid={} qword={:#x} on_rq={} key={:#x}/{} cfs_nr={} tree_total={} head={:?} stashovf={} skipall={}",
                        w3_pid,
                        w3_qword,
                        w3_onrq,
                        w3_key_vr,
                        w3_key_id,
                        w3_cfs_nr,
                        w3_tree_total,
                        w3_tree,
                        crate::sched::sched_hooks::pick_giveups().0,
                        crate::sched::sched_hooks::pick_giveups().1
                    );
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                    crate::pr_warn!(
                        "W3-CTX pid={} preempt={:#x} ra={:#x} sp={:#x} pt={:?} sw={:?}",
                        w3_pid,
                        w3_pc,
                        w3_ra,
                        w3_sp,
                        pick_trace_peek(task),
                        w3_w
                    );
                    {
                        // user trap frame at stack_top-168: dump the frame
                        // words (userspace rip/cs/rfl/rsp/ss live there)
                        let mut uf = [0u64; 21];
                        let okf = crate::sched::edges::task_shared(task).get_kernel_stack().map_or(false, |top| {
                            let base = (top as usize).saturating_sub(168) as *const u64;
                            // SAFETY: diagnostic read of the task's own
                            // kernel-stack entry frame region.
                            {
                                for k in 0..21 {
                                    uf[k] = crate::sync::edges::read_volatile_u64((base as usize) + 8 * k);
                                }
                            }
                            true
                        });
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                        crate::pr_warn!(
                            "W3-USER pid={} frame_ok={} uf={:?}",
                            w3_pid,
                            okf,
                            uf
                        );
                        let k = crate::sched::edges::task_shared(task).timed_sleep_kind.load(core::sync::atomic::Ordering::Relaxed);
                        let d = crate::sched::edges::task_shared(task).timed_sleep_deadline.load(core::sync::atomic::Ordering::Relaxed);
                        let n = crate::timer::read_time();
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                        crate::pr_warn!(
                            "W3-TSL pid={} kind={} deadline={} now={} delta_ms={} dur_ms={} timer_id={} sigpend={:#x} sigmask={:#x}",
                            w3_pid,
                            k,
                            d,
                            n,
                            d.saturating_sub(n) * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ as u64,
                            crate::sched::edges::task_shared(task).timed_sleep_dur_ms.load(core::sync::atomic::Ordering::Relaxed),
                            crate::sched::edges::task_shared(task).timed_sleep_timer_id.load(core::sync::atomic::Ordering::Relaxed),
                            crate::sched::edges::task_shared(task).pending.get_all(),
                            crate::sched::edges::task_shared(task).sigmask,
                        );
                    }
                    // nanosleep-loss lane: enroll the refusing pid in the
                    // WW watch on first sight, and on every later refusal
                    // dump its lifecycle ring (W/L/P/S/D/K/R/A tokens) —
                    // the pick/abort livelock vs the wake/link loop vs the
                    // silent park, told apart in one line.
                    {
                        static W3_WATCHED: [core::sync::atomic::AtomicU32; 8] =
                            [const {
                                core::sync::atomic::AtomicU32::new(0)
                            }; 8];
                        let mut enrolled = false;
                        for s in W3_WATCHED.iter() {
                            if s.load(core::sync::atomic::Ordering::Relaxed) == w3_pid {
                                enrolled = true;
                                break;
                            }
                        }
                        if !enrolled {
                            for s in W3_WATCHED.iter() {
                                if s
                                    .compare_exchange(
                                        0,
                                        w3_pid,
                                        core::sync::atomic::Ordering::AcqRel,
                                        core::sync::atomic::Ordering::Acquire,
                                    )
                                    .is_ok()
                                {
                                    ww_watch_add(task);
                                    break;
                                }
                            }
                        } else {
                            let mut buf = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(
                                &mut buf,
                                "W3-WWDUMP pid={} ",
                                w3_pid
                            );
                            ww_append_dump(
                                &mut buf,
                                task,
                                crate::timer::get_jiffies(),
                            );
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                            crate::pr_warn!("{}", buf);
                        }
                    }
                }
                if w3_fz_ready {
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                    crate::pr_warn!(
                        "W3-FROZEN-EXEC-SLOT pid={} naming_cpu={} exec={:#x} frozen for {}jif pk={} on_cpu={} retaddr={:#x}",
                        w3_pid,
                        w3_fz_naming,
                        w3_fz_exec,
                        w3_fz_djif,
                        w3_pk,
                        w3_on_cpu,
                        w3_fz_retaddr,
                    );
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                    crate::pr_warn!(
                        "W3-FROZEN-EXEC-STK pid={} sp={:#x} w={:#x} {:#x} {:#x} {:#x}",
                        w3_pid,
                        w3_fz_sp,
                        w3_fz_w[0],
                        w3_fz_w[1],
                        w3_fz_w[2],
                        w3_fz_w[3],
                    );
                    // timer-gap lane: name the naming cpu's LIVE position —
                    // the frozen slot alone cannot distinguish "vCPU dead
                    // mid-switch" from "vCPU alive elsewhere with a stale
                    // name". One remote bt per boot, gated to SMP2+ boots
                    // (the reporter would deadlock self-IPI'ing its own
                    // cpu on a UP box).
                    #[cfg(feature = "x86_64")]
                    {
                        static W3BT: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        let me = crate::arch::cpu_id() as usize;
                        // timer-gap: bt the naming cpu AND the sibling
                        //(SMP2 diagnostic — the g2 wedge needs both sides;
                        //rt2xdbg=1-gated: it fires remote IPIs).
                        if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
                            for tgt in [w3_fz_naming, w3_fz_naming ^ 1] {
                                if tgt != me
                                    && tgt < crate::config::MAX_CPUS
                                    && W3BT.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 2
                                {
                                    let _ = crate::arch::ipi::remote_backtrace(
                                        tgt,
                                        "w3-frozen-exec",
                                    );
                                }
                            }
                        }
                    }
                }
                return false;
            }
            // R63: RUNNING ∧ unlinked ∧ owned by no current slot still has
            // TWO producers. (a) A CPU is INSIDE context_switch with this
            // task as prev right now (its current slot already shows next,
            // but its callee-saved/fpu context is not saved until the
            // switch publish). (b) A genuinely orphaned mark (the R49
            // capture family). Only (b) may be cleared; for (a) the mark
            // is the ONLY thing holding every class pick's skip, and the
            // outgoing publish clears it itself once the context is fully
            // saved. Either way we fall through to the enqueue below —
            // linking the task is always safe (the pick skip sequences
            // the actual resume); the difference is solely whether the
            // mark survives until the publish.
            let mid_switch_out = task_is_mid_switch_out(task);
            if !mid_switch_out && crate::sched::edges::task_shared(task).on_cpu() {
                // R74 (false-orphan double-run): the pick-state machine is
                // AUTHORITATIVE while on_cpu is set — on_cpu/claim are
                // published together at pick time (publish_pick_ownership)
                // and released together by the switch-out, so {on_cpu=1,
                // claim=X} means CPU X's pick is still in flight (or the
                // task is LIVE on X). The slot scan above protects only the
                // window where X's slot still names the task; once any
                // heal/abort rewrote that slot, this branch was the last
                // line of defense — and it cleared the mark anyway,
                // producing linked+{on_cpu=0, claim=X}: the pick-side
                // stale-claim reset then handed the LIVE task to a second
                // CPU (r74a capture: R49-ONCPU-ORPHAN healed pid=344 with
                // claim=0, STALE-CLAIM-RESET by_cpu=3, instant double-run
                // KERNPANIC). Mirror harvest_relink's discipline: LIVE is
                // executing somewhere — refuse; PICKED must be revoked so
                // a starved-but-live picker aborts at its affirm CAS; only
                // then may the orphaned mark be cleared.
                let ps = crate::sched::edges::task_shared(task).pick_state();
                if ps == crate::process::task::PICK_LIVE {
                    // R80 (LIVE-phantom heal, the v1-r1 Xorg capture): we
                    // are holding the GRQ with the task RUNNING ∧ unlinked ∧
                    // owned by no slot (the curr_on scan above — lying idle
                    // slots were already rewritten — and no SWITCHING_OUT
                    // names it). A LIVE pick-state is published together
                    // with the publisher's slot naming the task (fresh
                    // PICKED + slot at pick; LIVE either by the switch-in
                    // affirm or by the repick/forced-LIVE publish — all
                    // under this lock, all with slot == task). A hart
                    // executing the task keeps its slot naming it until its
                    // next __schedule, and that __schedule either releases
                    // the ownership qword (full switch-out) or leaves the
                    // mid-switch-out mark. NONE of those legs can present
                    // LIVE ∧ unlinked ∧ slotless ∧ !SWITCHING_OUT — the
                    // state is impossible in-tree, and the v1-r1 capture
                    // held it for 30 minutes with every picker, waker and
                    // harvester refusing on the LIVE word alone. Time-confirm
                    // through the orphan watch (fed by the harvester's
                    // retries): a LIVE mark that persists >=50 jiffies in this shape
                    // is stale, and clearing it un-strands the task. The
                    // m25 double-run lesson is respected — the r74a capture
                    // cleared a mark whose claimer's slot STILL advanced only
                    // because the R49 heal ran without the pick-state
                    // adjudication; here the slot scan, the SWITCHING_OUT
                    // check and the time confirmation all run under one GRQ
                    // hold, and a hart that is genuinely resuming the task
                    // keeps its slot naming it for the whole affirm window.
                    if !orphan_watch_confirm(
                        task,
                        crate::sched::edges::task_shared(task).pid(),
                        crate::timer::get_jiffies(),
                        50,
                    ) {
                        return false;
                    }
                    // (Threshold 50 jiffies = 500 ms, two orders above
                    // every legitimate in-flight window. R81: conviction
                    // is the frozen exec clock, so the threshold matures
                    // at ANY observer cadence — the old [50,60]-jiffy
                    // window could never open for 15-s-apart rekicks.)
                    // Rule 1 (grq-lock-1st): event ring, not the console.
                    crate::sched::sched_hooks::sched_ev(
                        crate::sched::sched_hooks::EV_R80_LIVE_PHANTOM,
                        crate::sched::edges::task_shared(task).pid(),
                        0,
                        0,
                    );
                    // Fall through to the release + insert below.
                } else if ps == crate::process::task::PICK_PICKED
                    && !crate::sched::edges::task_shared(task).revoke_pending_pick()
                {
                    // The picker affirmed LIVE under us (it is resuming
                    // the task right now) — not healable.
                    return false;
                }
                // Rule 1 (grq-lock-1st): event ring, not the console
                // (a = the stale ownership qword being cleared).
                crate::sched::sched_hooks::sched_ev(
                    crate::sched::sched_hooks::EV_R49_ONCPU_ORPHAN,
                    crate::sched::edges::task_shared(task).pid(),
                    crate::sched::edges::task_ownership_qword(task),
                    0,
                );
                // No CPU owns this mark (curr_on == false under the lock AND
                // no in-flight switch names it): clearing it cannot break the
                // NEW2 pick-skip protocol.
                // R74: release the FULL ownership qword ({on_cpu=false,
                // claim=-1}), not the byte alone — a byte-only clear leaves
                // {false, foreign-claim}, the protocol-impossible linked
                // state the pick-side stale-claim reset was invented for;
                // with a live claimer that reset double-ran the task (r74a).
                crate::sched::edges::task_shared(task).release_pick_ownership();
            }
            // Rule 1 (grq-lock-1st): the R46 heal record — event ring,
            // not the console. a = the stale ownership qword, b = the
            // parked kernel sp (the producer-hunt anchor; the historical
            // in-line stack-words dump is gone with the print).
            crate::sched::sched_hooks::sched_ev(
                crate::sched::sched_hooks::EV_R46_RUNNING_PHANTOM,
                crate::sched::edges::task_shared(task).pid(),
                crate::sched::edges::task_ownership_qword(task),
                crate::sched::edges::task_shared(task).thread().sp as u64,
            );
            // Fall through: enqueue_task_locked links the task (its
            // set_state(RUNNING) is a no-op re-write; nr_running pairs with
            // the new linkage per R7-B5).
        }
        // R39: propagate the insert result. A refused insert (only possible
        // via a guard divergence for a sleeping/STOPPED target) leaves the
        // task SLEEPING — honest and still wakeable — and now also REPORTED
        // to the caller as a failed wake instead of a silent success that
        // consumed the waker's one-shot token while linking nothing.
        //
        // R41 (A-form twin): "still wakeable" is only true if a LATER wake
        // can succeed — but the refusing guard was the task's own stuck
        // on_rq=true, which nothing else clears for an unlinked task, so
        // without the heal below every future wake would also be refused
        // (the sleeper wedges permanently while the waker's token was
        // consumed). On refusal, verify actual linkage and heal the stale
        // flag — the same defense requeue_prev_locked applies to prev.
        //
        // R42 (lost wakeup in the sleeper's link-drop window): a `true`
        // from ensure_linked_locked means the task is LINKED — but this
        // function's contract is "transitioned AND enqueued", and there is
        // a fully legitimate, frequently raced producer of exactly this
        // refusal: the target is between its prepare_to_wait/set_state
        // (SLEEPING written under the WAITQUEUE/futex-bucket lock, NOT the
        // GRQ lock) and its own schedule() — still linked on its class
        // queue (the link is dropped only by its __schedule, under this
        // same GRQ lock). The waker's one-shot token (wait-queue entry
        // marked woken, futex waiter unlinked from the chain, semaphore/
        // pipe token consumed) is spent BEFORE Task::wake_up returns, and
        // wait-queue/futex wakers ignore the result — so leaving the state
        // SLEEPING here lets the sleeper's __schedule dequeue it with no
        // future wake available: a permanent SMP hang (the silent form-A
        // residual in every gate since R39; the R41 tripwire never fires
        // because the task is genuinely linked — this is NOT the stale
        // flag the heal below targets). Flip the state, exactly as the
        // pre-R39 unconditional set_state did for this window: the
        // sleeper's schedule() then sees prev_running, stays linked, and
        // its wait loop re-checks the condition the waker just made true
        // (a spurious wake simply loops back to sleep). For the heal
        // branch ensure_linked_locked's re-insert already set RUNNING;
        // for the poison/dead refusal it returned false and we pass that
        // through untouched.
        //
        // The sched:sched_waking tracepoint — the reference records it in
        // try_to_wake_up right after the state match passes, before the
        // enqueue attempt. This is the matching point in the funnel:
        // every target that still reaches the enqueue below (a genuinely
        // sleeping/STOPPED task, or a healed phantom the R46 family just
        // re-armed) is "about to be woken"; the refused stale shapes
        // returned above, exactly like the reference's state-mismatch
        // goto-out before the tracepoint. GRQ is held here; record()
        // takes no locks (the sched_switch site relies on the same
        // contract under IRQs-off).
        if crate::trace::EVENT_SCHED_WAKING.armed() {
            crate::trace::trace_sched_waking(
                crate::sched::edges::task_shared(task).comm(),
                crate::sched::edges::task_shared(task).pid(),
                crate::sched::edges::task_shared(task).prio(),
                crate::sched::edges::task_shared(task).ti_cpu(),
            );
        }
        //
        // WAKE-HEAL (launcher perf wall, 2026-10-31): the OH boots show
        // the same few service tasks (mmi_service, os_ds_service, OS_IPC
        // workers) parking with preempt_count==1 and waking back up with
        // it still set — every wake runs them UNPREEMPTIBLE, so each
        // kernel-side IPC loop monopolizes its CPU until the user-mode
        // return (the 1-2.6 s WAKE-STRANDED class; lb13/14/15: strands
        // with h0/h1 incumbents at pc=1/2). The parkwalk's 4 s PARKED-HEAL
        // never matures for them because the park/run bursts reset the
        // window, and the tick-side 2 s PREEMPT-HEAL only bounds the
        // monopoly after it formed. This closes the cycle at the exact
        // right point: the task is being woken RIGHT NOW, so it is about
        // to execute — and a task owned by no CPU slot (verified under
        // this GRQ hold, which excludes every concurrent pick publish)
        // holds no legal guard by construction (guards live on the
        // executing task; the pre-sleep window's task is still in its
        // CPU's current slot and the mid-switch-out shape still carries
        // the R63 marker — both skipped below, mirroring the parkwalk
        // adjudication). sc265 convicted the invariant: ZERO guards are
        // ever booked on a task current on no CPU. A nonzero count here
        // is phantom 100%; zeroing it means the woken task runs
        // preemptible from its first instruction, so the monopoly never
        // forms at all. Full zero (base + stranded hardirq bits), the
        // same medicine PARKED-HEAL administers to the same class.
        //
        // r12 (LP-OUTCOME): set when this wake's lying-slot observation
        // printed (the LP block below); consumed after the enqueue
        // decision so a strand boot says whether the OBSERVED wake
        // finally landed (woken=1) or was silently dropped there
        // (woken=0) — the bit that separates a refused-diagnostic wake
        // (concomitant) from a genuinely lost one.
        let mut lp_observed = false;
        {
            let pc = crate::sched::edges::task_shared(task).preempt_count();
            if pc != 0 {
                let exiting = crate::sched::edges::task_shared(task).ti_flags()
                    & crate::process::task::task_flags::TaskFlags::PF_EXITING
                        .bits()
                    != 0;
                let mut mid_switch = task_is_mid_switch_out(task);
                // Stale-marker confirmation (glib-mutex rewrite,
                // 2026-11-14): a real context switch-out is microseconds
                // and the marker is cleared on the same CPU right after
                // the coroutine switch. A marker whose AGE (from
                // SWITCHING_OUT_JIF, stamped at marker-set time under the
                // GRQ) exceeds SWITCHING_OUT_STALE_JIF is a wedged
                // leftover of an aborted/lost switch — no CPU is saving
                // this context, the task is parked, and treating the
                // marker as live would poison every future wake-heal of
                // this task. Time-confirm by AGE, then ignore.
                //
                // The ORIGINAL form compared SIGHTING-to-SIGHTING
                // distance per task (the WM table): a wake only sees the
                // marker when it lands inside the microseconds-wide
                // switch window, so for fast-cycling tasks (futex/yield
                // workers — the garepro class) sightings are naturally
                // hundreds of jiffies apart and every FRESH marker was
                // declared stale. That disabled the mid-switch guard
                // exactly for the tasks that park/wake hardest, letting
                // the heal zero a task's LIVE in-flight guard count
                // (captured: garepro26 +252.1 s, pid=328, WAKE-HEAL
                // zeroing the GRQ guard booked at sched.rs:2994 on cpu5,
                // then the guard's own release underflowing 5 ms later).
                if mid_switch {
                    let jif = crate::timer::get_jiffies();
                    let mut stale = false;
                    for c in 0..MAX_CPUS {
                        if SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire)
                            == task as u64
                        {
                            let set_jif =
                                SWITCHING_OUT_JIF[c].load(core::sync::atomic::Ordering::Acquire);
                            if jif.wrapping_sub(set_jif) >= SWITCHING_OUT_STALE_JIF {
                                stale = true;
                            }
                            break;
                        }
                    }
                    if stale {
                        mid_switch = false;
                    }
                }
                let mut curr_on = false;
                if !mid_switch {
                    for c in 0..MAX_CPUS {
                        if cpu_state(c).current == task {
                            curr_on = true;
                            break;
                        }
                    }
                }
                if !exiting && !mid_switch && !curr_on {
                    crate::sched::edges::task_shared(task).ti_preempt_count_zero();
                    static WH_N: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    if WH_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 300 {
                        // R34 discipline: GRQ is held — direct console,
                        // no printk/UART lock under GRQ.
                        use crate::debug::taskdump::putc_brief as putchar;
                        fn ph(v: u64) {
                            use crate::debug::taskdump::putc_brief as putchar;
                            for sh in (0..16).rev() {
                                let n = ((v >> (sh * 4)) & 0xF) as u8;
                                putchar(if n < 10 {
                                    b'0' + n
                                } else {
                                    b'a' + n - 10
                                });
                            }
                        }
                        const M: &[u8] = b"\nWAKE-HEAL pid=";
                        for &b in M {
                            putchar(b);
                        }
                        ph(crate::sched::edges::task_shared(task).pid() as u64);
                        const M2: &[u8] = b" was=";
                        for &b in M2 {
                            putchar(b);
                        }
                        ph(pc as u32 as u64);
                        putchar(b'\n');
                        // preempt-hunt (2026-11-01): name the acquire
                        // site whose 1->0 release never ran — the
                        // outermost-acquire tap. This is the producer's
                        // home site, printed at the moment the phantom
                        // count is adjudicated (still the direct-console
                        // discipline: GRQ held).
                        crate::debug::leakorigin::report_outermost(
                            b"WAKE-HEAL-ORIGIN",
                            task,
                        );
                        // wake-fastrepro (leak hunt): the ORIGIN tap is
                        // collision-churned on OH boots (2048 slots vs
                        // thousands of tasks — both control boots printed
                        // WAKE-HEAL with zero ORIGIN lines). The victim's
                        // own last_calls syscall ring (rux:sysforensic (retired debug=sysforensic),
                        // 96 deep, per-task) survives churn by
                        // construction: dump its tail here so the pc=1
                        // face names the syscall path it rides in.
                        {
                            use crate::debug::taskdump::putc_brief as putchar2;
                            fn pd2(mut v: u64) {
                                use crate::debug::taskdump::putc_brief as p;
                                let mut buf = [0u8; 12];
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
                            const M: &[u8] = b"\nWAKE-HEAL-CALLS pid=";
                            for &b in M {
                                putchar2(b);
                            }
                            pd2(crate::sched::edges::task_shared(task).pid() as u64);
                            const M2: &[u8] = b" last12=";
                            for &b in M2 {
                                putchar2(b);
                            }
                            let cur = crate::sched::edges::task_shared(task)
                                .last_calls_cur
                                .load(core::sync::atomic::Ordering::Relaxed);
                            for k in 0..12usize {
                                let idx = (cur + crate::process::task::LAST_CALLS_LEN
                                    - 1
                                    - k)
                                    % crate::process::task::LAST_CALLS_LEN;
                                let nr = crate::sched::edges::task_shared(task).last_calls[idx]
                                    .load(core::sync::atomic::Ordering::Relaxed);
                                if k > 0 {
                                    putchar2(b',');
                                }
                                pd2(nr as u64);
                            }
                            putchar2(b'\n');
                        }
                    }
                } else {
                    // Skip-reason census (budgeted): names which guard
                    // ate the heal — without this the zero-fire state is
                    // indistinguishable between "never reached" and
                    // "predicate always skips" (the lb17 capture: 21
                    // PARKED-LEAK, 0 WAKE-HEAL). 110th: rides rux:schb (retired debug=schb) —
                    // ~56 direct-console lines rode the pre-launcher
                    // window of every perf boot.
                    static WHS_N: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    if WHS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 60
                        && crate::trace::EVENT_SCHB.armed()
                    {
                        use crate::debug::taskdump::putc_brief as putchar;
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
                        const M: &[u8] = b"\nWAKE-HEAL-SKIP pid=";
                        for &b in M {
                            putchar(b);
                        }
                        pdec(crate::sched::edges::task_shared(task).pid() as u64);
                        const R1: &[u8] = b" why=";
                        for &b in R1 {
                            putchar(b);
                        }
                        if exiting {
                            putchar(b'e');
                        }
                        if mid_switch {
                            putchar(b's');
                        }
                        if curr_on {
                            putchar(b'c');
                        }
                        putchar(b'\n');
                        // r10 (lmbench lat_udp/bw_tcp strand handoff): the
                        // why=c refusal is exactly the shape the heal's
                        // origin report exists for — a task still carrying
                        // a nonzero count whose release never ran — but
                        // the heal branch above (which prints ORIGIN) is
                        // SKIPPED for curr_on victims, so the producer was
                        // invisible precisely on the permanent-strand
                        // family. Report the outermost acquire site here
                        // too (rux:leakhunt (retired debug=leakhunt)-gated, own 200-line budget,
                        // still direct-console: GRQ held).
                        if curr_on {
                            crate::debug::leakorigin::report_outermost(
                                b"WAKE-SKIP-ORIGIN",
                                task,
                            );
                        }
                        // Lost-park census tail (rux:lostpark (retired debug=lostpark)): when the
                        // refusal is curr_on, name the lying slot's CPU +
                        // writer tag and the victim's full ownership word —
                        // the wedged-park signature (a satisfiable sleeper
                        // no wake can reach). r10: the rq_load()==0 gate is
                        // dropped — the lmbench lat_udp/bw_tcp strand keeps
                        // its peer looping (rq_load>0) while the UDP server
                        // is refused forever, so the empty-queue-only form
                        // never fired on exactly the family it hunts.
                        if curr_on
                            && crate::trace::EVENT_LOSTPARK.armed()
                        {
                            // Rule 1 (grq-lock-1st): the lying-slot census
                            // tail — event ring, not the console. a packs
                            // (naming_cpu << 32) | slot_writer_tag, b is the
                            // victim's ownership qword; the drain prints it.
                            lp_observed = true;
                            let mut a: u64 = 0xffff_ffff;
                            for c in 0..MAX_CPUS {
                                if cpu_state(c).current == task {
                                    a = ((c as u64) << 32)
                                        | SLOT_WRITER[c]
                                            .load(core::sync::atomic::Ordering::Relaxed)
                                            as u64;
                                    break;
                                }
                            }
                            crate::sched::sched_hooks::sched_ev(
                                crate::sched::sched_hooks::EV_LP_WAKE_SKIP_EMPTY,
                                crate::sched::edges::task_shared(task).pid(),
                                a,
                                crate::sched::edges::task_ownership_qword(task),
                            );
                        }
                    }
                }
            }
        }
        // The result feeds the sched:sched_wakeup tracepoint below, so
        // route the two true-arms through one binding first (the else
        // arm keeps its inline refusal print).
        let woken = if enqueue_task_locked(&mut *grq_guard, crate::sched::edges::task_mut(task)) {
            pick_trace(task, 11);
            true
        } else if ensure_linked_locked(&mut *grq_guard, crate::sched::edges::task_mut(task)) {
            crate::sched::edges::task_shared(task).set_state(TaskState::new(TaskState::RUNNING));
            pick_trace(task, 11);
            true
        } else {
            // W2 twin: both the insert and the heal refused (sc78: the
            // reply-delivery lost-wake hunt). Rule 1 (grq-lock-1st): the
            // event ring carries it; the tick drain prints it.
            crate::sched::sched_hooks::sched_ev(
                crate::sched::sched_hooks::EV_W2_ENQ_HEAL_REFUSED,
                crate::sched::edges::task_shared(task).pid(),
                0,
                0,
            );
            false
        };
        // Wakewatch: the funnel's verdict for this task (W=linked and
        // RUNNING, R=refused) with the jiffies stamp.
        ww_record(task, if woken { WW_EV_WAKE_OK } else { WW_EV_WAKE_REFUSED });
        // r12 (LP-OUTCOME): for every wake whose lying-slot observation
        // printed above, name the FINAL outcome — the enqueue below is
        // the step the LP protocol analysis says cannot drop the wake;
        // woken=0 here convicts it, woken=1 acquits the wake path
        // entirely. Same gate and budget discipline as the LP print
        // (LostParkTrace already checked; 16-line boot budget, GRQ held
        // so direct console).
        if lp_observed {
            // Rule 1 (grq-lock-1st): event ring, not the console (a =
            // woken 0/1 — the LP verdict).
            crate::sched::sched_hooks::sched_ev(
                crate::sched::sched_hooks::EV_LP_WAKE_OUTCOME,
                crate::sched::edges::task_shared(task).pid(),
                woken as u64,
                0,
            );
        }
        // The sched:sched_wakeup tracepoint — the reference's
        // ttwu_do_wakeup record: the task left the wait queue and is
        // enqueued (or was still linked in the R42 race window), state
        // now RUNNING. Both true-arms land here; a refused wake never
        // reaches it, which is the wake→pick census discrimination as
        // event data (a waking with no matching wakeup = refused wake).
        // GRQ is held; record() takes no locks.
        if woken && crate::trace::EVENT_SCHED_WAKEUP.armed() {
            crate::trace::trace_sched_wakeup(
                crate::sched::edges::task_shared(task).comm(),
                crate::sched::edges::task_shared(task).pid(),
                crate::sched::edges::task_shared(task).prio(),
                crate::sched::edges::task_shared(task).ti_cpu(),
            );
        }
        woken
    }
}

/// Dequeue a task from the global RQ.
pub fn dequeue_task(task: &Task) {
    let task_ptr = task as *const Task as *mut Task;

    let mut grq_guard = grq().lock_irqsave_site(grq_site(GRQ_CALLER_DEQUEUE, GRQ_STEP_TRY));

    // R39: read the policy UNDER the GRQ lock. The old read raced
    // change_task_policy (which swaps policy + migrates the class queues
    // under this same lock): a policy captured before the lock could
    // address the WRONG class queue — dequeuing nothing from the new class
    // while believing the compensation succeeded, or worse, no-op'ing the
    // exit-path dequeue of a task that had just been re-linked elsewhere.
    let policy = task.policy();
    let actually_dequeued = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            // R31-5: propagate the real dequeue result — the hardcoded
            // true decremented nr_running on every no-op (every RT/DL
            // exit + wait-recheck), leaking the idle fast path.
            grq_guard.rt_rq.dequeue(task_ptr)
        }
        SchedPolicy::Deadline => {
            grq_guard.dl_rq.dequeue(task_ptr)
        }
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            let dequeued = grq_guard.cfs_rq.dequeue(task_ptr);
            if dequeued {
                crate::sched::sched::ww_record(task_ptr, WW_EV_UNLINK_DEQ);
            }
            // R8-1: clear curr UNCONDITIONALLY when it matches — the R7-2
            // version required `dequeued`, but a RUNNING task was already
            // picked off the tree (dequeue returns false), so the exit path
            // never fired and update_curr kept writing into the FREED Task
            // on every idle tick (the corruption engine behind NEW2's
            // wandering damage).
            if grq_guard.cfs_rq.get_curr() == task_ptr {
                grq_guard.cfs_rq.set_curr(core::ptr::null_mut());
            }
            dequeued
        }
    };

    if actually_dequeued {
        grq_guard.nr_running.fetch_update(
            core::sync::atomic::Ordering::SeqCst,
            core::sync::atomic::Ordering::SeqCst,
            |v| v.checked_sub(1),
        );
    }
}

/// Remove `task` from the global run queue if a racing wake_up() enqueued it
/// while it was transiently marked sleeping — the "prepare-to-wait recheck
/// decided not to sleep" path calls this before continuing to run, so no
/// other CPU can pick a task that is already executing (review NEW-C2).
///
/// CFS-only and idempotent: cfs_rq.dequeue() checks the entity's own on_rq
/// flag and is a safe no-op for a task that is not linked. RT/DL are
/// excluded because their RR rotation legitimately keeps the current task
/// linked, so "on queue" cannot be distinguished from "running" there.
pub fn dequeue_if_enqueued(task: &Task) {
    let task_ptr = task as *const Task as *mut Task;
    let mut grq_guard = grq().lock_irqsave();
    let dequeued = match task.policy() {
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            grq_guard.cfs_rq.dequeue(task_ptr)
        }
        _ => false,
    };
    if dequeued {
        grq_guard.nr_running.fetch_update(
            core::sync::atomic::Ordering::SeqCst,
            core::sync::atomic::Ordering::SeqCst,
            |v| v.checked_sub(1),
        );
    }
}

pub(crate) fn context_switch(prev: &mut Task, next: &mut Task) {
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    pick_trace(next, 5);

    // cctx probe: CW_N counted here; the phase stamps live inside
    // arch::context::context_switch (the phases — FPU save, CR3, MSRs,
    // asm switch, FPU restore — are arch-local).
    crate::debug::cctx::bump(&crate::debug::cctx::CW_N);

    // forkprof (cmdline forkprof=1): consume a pending wake stamp —
    // wake->switch-in is the latency the fork/exit handoff pays.
    if crate::debug::forkprof::on() {
        crate::debug::forkprof::consume_wake(next.pid());
        crate::debug::forkprof::trace(crate::debug::forkprof::T_CTXSW_IN, next.pid());
    }

    // Leak-hunt round 3f: __schedule's discipline refuses to switch out
    // any non-dead task with a nonzero preempt_count, so a prev reaching
    // here with a live count means a DISCIPLINE-BYPASSING switch just
    // happened — the exact event whose guard then drops in another task's
    // context (the leak producer's other half). One compare + rare print;
    // rate-gated direct console.
    {
        let pc = prev.preempt_count();
        if pc != 0 && !prev.state().is_dead() {
            static LAST: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now = crate::timer::get_jiffies();
            let last = LAST.load(core::sync::atomic::Ordering::Acquire);
            if now.wrapping_sub(last) >= 20
                && LAST
                    .compare_exchange(
                        last,
                        now,
                        core::sync::atomic::Ordering::AcqRel,
                        core::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                use crate::debug::taskdump::putc_brief as putchar;
                fn ph(v: u64) {
                    use crate::debug::taskdump::putc_brief as putchar;
                    for sh in (0..16).rev() {
                        let n = ((v >> (sh * 4)) & 0xF) as u8;
                        putchar(if n < 10 {
                            b'0' + n
                        } else {
                            b'a' + n - 10
                        });
                    }
                }
                const M: &[u8] = b"\nSWITCH-COUNT-LEAK prev_pid=";
                for &b in M {
                    putchar(b);
                }
                ph(prev.pid() as u64);
                const M2: &[u8] = b" pc=";
                for &b in M2 {
                    putchar(b);
                }
                ph(pc as u32 as u64);
                const M3: &[u8] = b" next_pid=";
                for &b in M3 {
                    putchar(b);
                }
                ph(next.pid() as u64);
                putchar(b'\n');
            }
        }
    }

    // R65 hunt (garbage-next guard): __switch_to saves callee registers
    // through `prev` and resumes through `next` (thread.sp/ret_addr are
    // read from it), so a non-Task pointer here IS the self-propagating
    // cross-stack engine (x86-scribble3 ring capture: PUB events whose
    // prev/next are kernel-stack addresses, 0x18 apart). Refuse the
    // garbage resume — switch to this CPU's idle task instead — and
    // report; the pick-side seed tripwire names the producer.
    #[cfg(feature = "x86_64")]
    let next: &mut Task = {
        let np = next as *mut Task;
        if crate::trace::EVENT_SCRIBBLE.armed()
            && !crate::debug::scribble::task_plausible(np)
        {
            crate::debug::scribble::bad_switch_arg(1, np as u64, prev as *mut Task as u64);
            let idle = cpu_state(cpu_id).idle;
            if !idle.is_null() {
                // Keep the ownership slots consistent with what actually
                // resumes (__schedule already published the garbage next
                // into the sched slot before calling us).
                this_cpu_mut().current = idle;
                // r11: supersede the garbage pick's generation (probe
                // path; the slot tag stays as the probe left it).
                pick_gen_supersede(cpu_id, idle);
                crate::sched::edges::task_mut(idle)
            } else {
                // No idle pointer (early boot): freeze for gdb.
                crate::debug::scribble::scribble_switch_park(np as u64, 1);
                crate::sched::edges::task_mut(np)
            }
        } else {
            next
        }
    };

    next.set_ti_cpu(cpu_id as i32);

    // /proc/stat "ctxt" + the per-task voluntary/involuntary switch
    // counters (Linux's __schedule accounting). The task being switched
    // OUT is the one that is charged: it blocked (voluntary) or was
    // preempted while still runnable (involuntary).
    crate::fs::perf_ctr::note_switch(prev);

    if next.is_fork_child() {
        next.clear_fork_child();
    }

    crate::sched::edges::arch_context_switch(prev, next);
}

pub fn current() -> Option<&'static mut Task> {
    // INV-SCE-11: the tp-register derivation lives in the edges
    // framework; this is its public face.
    crate::sched::edges::current_task_mut()
}

/// True when the CURRENT task is dying (PF_EXITING or SIGKILL pending).
/// The reference kernel's fatal_signal_pending(current) checks: reclaim
/// and allocation-stall loops bail out early for a dying allocator
/// (mm/page_alloc.c __alloc_pages_slowpath, vmscan's shrink loops) —
/// a task being killed must not crawl through swap I/O it will never
/// use, nor keep other tasks waiting on the reclaim gate (SP-5).
pub fn current_is_dying() -> bool {
    // SAFETY: sched::current() returns the running task's valid pointer
    // (same contract as signal_pending's use).
    match current() {
        Some(t) => task_is_dying(t as *mut Task),
        None => false,
    }
}
// (The policy re-export block is GONE — every lib consumer of the
// policy half routes through sched_hooks rows; the counters read via
// the indexed getters, the consts live sched_hooks-side.)

// ==================== Hook-routed policy surface (S1b) ====================
//
// (sched-seam devirt lane, 2026-11-23): the OBSERVABILITY rows whose
// bodies are plain state touches (class_nonempty, sched_hist_armed,
// the schedstat gate, pick_trace, the pick-generation ledger) are
// framework-native fns now — their storage lives in entities.rs and
// the per-switch/per-enqueue call sites pay a direct call, not a
// SchedHooks fn-pointer dispatch. The remaining shims below still
// route through the table.

/// Set a class's nonempty bit (GRQ-held; call on the 0 -> nonempty
/// transition of that class's counter). Framework-native (devirt lane).
#[inline]
pub fn class_nonempty_set(bit: u32) {
    super::entities::CLASS_NONEMPTY.fetch_or(bit, core::sync::atomic::Ordering::Relaxed);
}
/// Clear a class's nonempty bit (GRQ-held; call on the -> empty
/// transition of that class's counter). Framework-native (devirt lane).
#[inline]
pub fn class_nonempty_clear(bit: u32) {
    super::entities::CLASS_NONEMPTY.fetch_and(!bit, core::sync::atomic::Ordering::Relaxed);
}
/// The class-chain short-circuit mask (pick_next_task loads it once per
/// pick). Framework-native (devirt lane).
#[inline]
pub fn class_nonempty() -> u32 {
    super::entities::CLASS_NONEMPTY.load(core::sync::atomic::Ordering::Relaxed)
}
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn load_balance() { crate::sched::sched_hooks::load_balance() }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn psledger_note_publish(task: *mut Task, cpu: usize, pk: u32) { crate::sched::sched_hooks::psledger_note_publish(task, cpu, pk) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn psledger_peek(task: *mut Task) -> Option<(u32, u32, u64)> { crate::sched::sched_hooks::psledger_peek(task) }
/// 63rd session (strand-wall hunt): gate for the persistent per-task
/// pick/wake history — one relaxed switch-table load pair per event.
/// Framework-native (devirt lane): the gate reads FRAMEWORK trace
/// flags; the old row made the pick walk pay a seam round trip to read
/// its own kernel's flags.
#[inline]
pub fn sched_hist_armed() -> bool {
    crate::trace::EVENT_FUXSTUCK.armed() || crate::trace::EVENT_SCHEDTRACE.armed()
}
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_hist_ring(task: &Task, kind: u64, why: u64, aux: u64) { crate::sched::sched_hooks::sched_hist_ring(task, kind, why, aux) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_hist_refuse(task: *mut Task, skip_reason: u32, cpu: usize) { crate::sched::sched_hooks::sched_hist_refuse(task, skip_reason, cpu) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_hist_link(task: *mut Task) { crate::sched::sched_hooks::sched_hist_link(task) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_hist_pick(task: *mut Task, cpu: usize) { crate::sched::sched_hooks::sched_hist_pick(task, cpu) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_cfs_geometry(task: *mut Task) -> (u64, u64, u64) { crate::sched::sched_hooks::ww_cfs_geometry(task) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_cfs_tie_mass(task: *mut Task) -> u64 { crate::sched::sched_hooks::ww_cfs_tie_mass(task) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_cfs_tie_mass_vr(vr: u64) -> u64 { crate::sched::sched_hooks::ww_cfs_tie_mass_vr(vr) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_tail_fill(ix: usize) -> u64 { crate::sched::sched_hooks::ww_tail_fill(ix) }
/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_append_dump(out: &mut dyn core::fmt::Write, task: *mut Task, now: u64) { crate::sched::sched_hooks::ww_append_dump(out, task, now) }

// These were re-exports of the policy half; they now route through the
// SchedHooks table (registered by the rux-sched-policy crate at boot —
// the NetHooks pattern). Callers' paths are unchanged.

/// Is per-task scheduler statistics accounting on? (the
/// /proc/sys/kernel/sched_schedstats runtime gate — one relaxed load;
/// framework-native since the devirt lane).
#[inline]
pub fn enabled() -> bool {
    super::entities::SCHEDSTATS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Turn per-task scheduler statistics on/off (the sysctl write side;
/// framework-native since the devirt lane).
#[inline]
pub fn set_enabled(on: bool) {
    super::entities::SCHEDSTATS.store(on, core::sync::atomic::Ordering::Release)
}

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn r56_print_pid(prefix: &[u8], pid: u32, suffix: &[u8]) { crate::sched::sched_hooks::r56_print_pid(prefix, pid, suffix) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_clock() -> u64 { crate::sched::sched_hooks::sched_clock() }

/// Nanosleep-loss pick-phase tracer: stamp `task`'s last recorded phase
/// (framework-side since the devirt lane — pure diagnostic state, the
/// table lives in entities.rs next to the other observability counters).
#[inline]
pub fn pick_trace(task: *mut Task, code: u32) {
    let t = task as usize;
    if t == 0 {
        return;
    }
    for s in 0..32 {
        let cur = super::entities::PT_TASK[s].load(core::sync::atomic::Ordering::Relaxed);
        if cur == t {
            super::entities::PT_CODE[s].store(code, core::sync::atomic::Ordering::Relaxed);
            super::entities::PT_JIF[s].store(
                crate::timer::get_jiffies(),
                core::sync::atomic::Ordering::Relaxed,
            );
            return;
        }
    }
    // claim a free slot (racy claim is fine — worst case a lost stamp)
    for s in 0..32 {
        if super::entities::PT_TASK[s]
            .compare_exchange(
                0,
                t,
                core::sync::atomic::Ordering::Relaxed,
                core::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
        {
            super::entities::PT_CODE[s].store(code, core::sync::atomic::Ordering::Relaxed);
            super::entities::PT_JIF[s].store(
                crate::timer::get_jiffies(),
                core::sync::atomic::Ordering::Relaxed,
            );
            return;
        }
    }
}

/// r11: heal-side slot rewrite (R56c/R56d/steal/abort-repub/idle-heal/
/// the R65 garbage-next guard): whatever generation was in flight is
/// superseded — publish and retire the new slot content in one breath
/// so the ledger keeps describing the slot (framework-side since the
/// devirt lane; the statics were always framework-owned).
#[inline]
pub(crate) fn pick_gen_supersede(cpu: usize, task: *mut Task) {
    pick_gen_publish(cpu, task);
    pick_gen_retire(cpu);
}

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn change_task_policy(task: *mut Task, new_policy: crate::process::task::SchedPolicy, rt_prio: u32) { crate::sched::sched_hooks::change_task_policy(task, new_policy, rt_prio) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_record(task: *mut Task, code: u32) { crate::sched::sched_hooks::ww_record(task, code) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn orphan_watch_confirm(task: *mut Task, pid: u32, now: u64, threshold: u64) -> bool { crate::sched::sched_hooks::orphan_watch_confirm(task, pid, now, threshold) }

/// Reader-side: the task's last recorded phase (code, jiffies-ago).
/// Framework-side since the devirt lane (pure diagnostic state).
#[inline]
pub fn pick_trace_peek(task: *mut Task) -> Option<(u32, u64)> {
    let t = task as usize;
    for s in 0..32 {
        if super::entities::PT_TASK[s].load(core::sync::atomic::Ordering::Relaxed) == t {
            return Some((
                super::entities::PT_CODE[s].load(core::sync::atomic::Ordering::Relaxed),
                crate::timer::get_jiffies().saturating_sub(
                    super::entities::PT_JIF[s].load(core::sync::atomic::Ordering::Relaxed),
                ),
            ));
        }
    }
    None
}

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn yield_cpu() { crate::sched::sched_hooks::yield_cpu() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn enqueue_task_locked(grq: &mut GlobalRunQueue, task: &mut Task) -> bool { crate::sched::sched_hooks::enqueue_task_locked(grq, task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_watch_add(task: *mut Task) { crate::sched::sched_hooks::ww_watch_add(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn abort_next_release_orphans(cpu: usize, next: *mut Task) { crate::sched::sched_hooks::abort_next_release_orphans(cpu, next) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn r56_report_switch_abort(cpu: usize, pid: u32) { crate::sched::sched_hooks::r56_report_switch_abort(cpu, pid) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_renice_locked(task: &mut Task, niceval: i32) { crate::sched::sched_hooks::sched_renice_locked(task, niceval) }

/// r11: the picked context is resuming — retire this CPU's CURRENT pick
/// generation (framework-side since the devirt lane; PICK_GEN statics
/// were always framework-owned, the row was pure dispatch overhead).
#[inline]
pub(crate) fn pick_gen_retire(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let g = PICK_GEN[cpu].load(core::sync::atomic::Ordering::Relaxed);
    PICK_GEN_DONE[cpu].store(g, core::sync::atomic::Ordering::Relaxed);
}

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn psledger_consume(task: *mut Task) -> bool { crate::sched::sched_hooks::psledger_consume(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn steer_task_cpu(task: *mut Task, target: i32) { crate::sched::sched_hooks::steer_task_cpu(task, target) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_can_stop_tick() -> bool { crate::sched::sched_hooks::sched_can_stop_tick() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn dl_throttle(task: &mut Task) { crate::sched::sched_hooks::dl_throttle(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn ensure_linked_locked(grq: &mut GlobalRunQueue, task: &mut Task) -> bool { crate::sched::sched_hooks::ensure_linked_locked(grq, task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn check_cfs_preempt(task: *mut Task, cpus_allowed: u32) { crate::sched::sched_hooks::check_cfs_preempt(task, cpus_allowed) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn check_rt_preempt(task: *mut Task, cpus_allowed: u32, prefer: i32) { crate::sched::sched_hooks::check_rt_preempt(task, cpus_allowed, prefer) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn check_dl_preempt(task: *mut Task, cpus_allowed: u32, prefer: i32) { crate::sched::sched_hooks::check_dl_preempt(task, cpus_allowed, prefer) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn orphan_watch_clear(task: *mut Task) { crate::sched::sched_hooks::orphan_watch_clear(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn pick_giveups() -> (u64, u64) { crate::sched::sched_hooks::pick_giveups() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_slot_mismatch_counters() -> [u64; MAX_CPUS] { crate::sched::sched_hooks::ww_slot_mismatch_counters() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_sched_snapshot() -> [u64; 10] { crate::sched::sched_hooks::ww_sched_snapshot() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_stalled_cpu_state(cpu: usize) -> (u64, u64, u64) { crate::sched::sched_hooks::ww_stalled_cpu_state(cpu) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn debug_claim_inject() -> usize { crate::sched::sched_hooks::debug_claim_inject() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn debug_oncpu_inject() -> usize { crate::sched::sched_hooks::debug_oncpu_inject() }

/// s14/60th walk-census getters (framework-native since the devirt
/// lane — plain counter reads).
#[inline]
pub fn ww_pick_walks() -> u64 {
    super::entities::WW_PICK_WALKS.load(core::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub fn ww_pick_walk_inc() {
    super::entities::WW_PICK_WALKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}
#[inline]
pub fn ww_oncpu_heals() -> u64 {
    super::entities::WW_ONCPU_HEALS.load(core::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub fn ww_oncpu_heal_inc() {
    super::entities::WW_ONCPU_HEALS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}
#[inline]
pub fn ww_claim_heals() -> u64 {
    super::entities::WW_CLAIM_HEALS.load(core::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub fn ww_claim_heal_inc() {
    super::entities::WW_CLAIM_HEALS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}
#[inline]
pub fn tl_poison_inserts() -> u64 {
    super::entities::TL_POISON.inserts.load(core::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub fn tl_poison_insert_inc() {
    super::entities::TL_POISON.inserts.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}
#[inline]
pub fn tl_poison_erases() -> u64 {
    super::entities::TL_POISON.erases.load(core::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub fn tl_poison_erase_inc() {
    super::entities::TL_POISON.erases.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

// ---- Per-CPU phase/count/wake-census accessors (devirt lane): plain
// state touches on entities.rs storage; framework-native. ----
#[inline]
pub fn sched_cs_phase(cpu: usize) -> u32 {
    if cpu < MAX_CPUS {
        super::entities::SCHED_CS_PHASE[cpu].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}
#[inline]
pub fn sched_cs_phase_set(cpu: usize, v: u32) {
    if cpu < MAX_CPUS {
        super::entities::SCHED_CS_PHASE[cpu].store(v, core::sync::atomic::Ordering::Release);
    }
}
#[inline]
pub fn sched_calls(cpu: usize) -> u64 {
    if cpu < MAX_CPUS {
        super::entities::SCHED_CALLS[cpu].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}
#[inline]
pub fn sched_calls_inc(cpu: usize) {
    if cpu < MAX_CPUS {
        super::entities::SCHED_CALLS[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}
#[inline]
pub fn ww_idle_phase(cpu: usize) -> u32 {
    if cpu < MAX_CPUS {
        super::entities::WW_IDLE_PHASE[cpu].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}
#[inline]
pub fn ww_idle_phase_set(cpu: usize, v: u32) {
    if cpu < MAX_CPUS {
        super::entities::WW_IDLE_PHASE[cpu].store(v, core::sync::atomic::Ordering::Release);
    }
}
#[inline]
pub fn wake_disp() -> u32 {
    super::entities::WAKE_DISP.load(core::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub fn wake_disp_set(v: u32) {
    super::entities::WAKE_DISP.store(v, core::sync::atomic::Ordering::Relaxed);
}
#[inline]
pub fn wake_idle_mask() -> u32 {
    super::entities::WAKE_IDLE_MASK.load(core::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub fn wake_idle_mask_set(v: u32) {
    super::entities::WAKE_IDLE_MASK.store(v, core::sync::atomic::Ordering::Relaxed);
}

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_heals() -> (u64, u64) { crate::sched::sched_hooks::ww_heals() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_subgate_counts() -> (u64, u64, u64) { crate::sched::sched_hooks::ww_subgate_counts() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_rt_pick_stats() -> (u64, u64, u64) { crate::sched::sched_hooks::ww_rt_pick_stats() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn heartbeat() { crate::sched::sched_hooks::heartbeat() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn stallwatch_tick() { crate::sched::sched_hooks::stallwatch_tick() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn preempt_check_wake(task: *mut Task) { crate::sched::sched_hooks::preempt_check_wake(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn preempt_check_wake_sync(task: *mut Task) { crate::sched::sched_hooks::preempt_check_wake_sync(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn w2_refusal_census(task: *mut Task) -> W2RefusalCensus { crate::sched::sched_hooks::w2_refusal_census(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn task_sched_class(task: &Task) -> &'static dyn SchedClass { crate::sched::sched_hooks::task_sched_class(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ww_note_slot_mismatch(cpu: usize) { crate::sched::sched_hooks::ww_note_slot_mismatch(cpu) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn monopolist_census(now_j: u64) { crate::sched::sched_hooks::monopolist_census(now_j) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn runq_strand_census(now_j: u64) { crate::sched::sched_hooks::runq_strand_census(now_j) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn ffrt_wait_census() { crate::sched::sched_hooks::ffrt_wait_census() }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn rt_wedge_dump_tick(cpu: usize) { crate::sched::sched_hooks::rt_wedge_dump_tick(cpu) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn rt_bandwidth_tick(cpu: usize, current: *mut Task) { crate::sched::sched_hooks::rt_bandwidth_tick(cpu, current) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn emit_grq_reports(reports: &[GrqPendingReport]) { crate::sched::sched_hooks::emit_grq_reports(reports) }

/// r11: open this CPU's pick generation — every publish of a task into
/// a PER_CPU current slot bumps the generation and records
/// {task, pid, jiffies}; PICK_GEN_DONE stays below it while in flight
/// (framework-side since the devirt lane; statics always lived here).
#[inline]
pub(crate) fn pick_gen_publish(cpu: usize, task: *mut Task) {
    if cpu >= MAX_CPUS {
        return;
    }
    let g = PICK_GEN[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    PICK_GEN_TASK[cpu].store(task as u64, core::sync::atomic::Ordering::Relaxed);
    PICK_GEN_PID[cpu].store(
        if task.is_null() { 0 } else { crate::sched::edges::task_shared(task).pid() },
        core::sync::atomic::Ordering::Relaxed,
    );
    PICK_GEN_JIF[cpu].store(
        crate::timer::get_jiffies(),
        core::sync::atomic::Ordering::Relaxed,
    );
    // PICK_GEN_DONE stays below `g`: this generation is in flight.
    let _ = g;
}

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn pick_next_task(grq: &mut GlobalRunQueue, cpu_id: usize, prev: *mut Task) -> *mut Task { crate::sched::sched_hooks::pick_next_task(grq, cpu_id, prev) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn requeue_prev_locked(grq: &mut GlobalRunQueue, prev: &mut Task) { crate::sched::sched_hooks::requeue_prev_locked(grq, prev) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn g_diag_idle_bit(cpu: usize) -> bool { crate::sched::sched_hooks::g_diag_idle_bit(cpu) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn cpu_tick_stale_ms(cpu: usize) -> u64 { crate::sched::sched_hooks::cpu_tick_stale_ms(cpu) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub(crate) fn lp_idle_lie_report(cpu: usize) { crate::sched::sched_hooks::lp_idle_lie_report(cpu) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_slice_to_ms(slice_ns: u64) -> u32 { crate::sched::sched_hooks::sched_slice_to_ms(slice_ns) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn sched_ev_drain(budget: usize) -> usize { crate::sched::sched_hooks::sched_ev_drain(budget) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn note_task_freed(task: *mut crate::process::Task) { crate::sched::sched_hooks::note_task_freed(task) }

/// Hook-routed (S1b): the policy crate's compiled fn via SchedHooks.
#[inline]
pub fn report_poisoned_rq_entry(cpu: usize, raw: usize) { crate::sched::sched_hooks::report_poisoned_rq_entry(cpu, raw) }


use crate::errno;
use crate::process::task::{Task, TaskState, SchedPolicy, Pid, PICK_LIVE, PICK_PICKED};
use crate::arch;
use crate::println;
use crate::config::MAX_CPUS;
use alloc::boxed::Box;
use crate::process::process_hooks::alloc_pid;
use core::arch::asm;
use crate::sync::spinlock::RawSpinlock;

use crate::config::MAX_TASKS;

// ==================== Global RunQueue ====================

/// Global run queue — one instance for the whole system.
///
/// Protected by a single `RawSpinlock`.  All enqueue / dequeue / pick_next
/// operations must hold this lock.  Timer-tick updates (`scheduler_tick`)
/// only touch the per-CPU `current` task and do NOT need the lock.
///
/// NOTE (2b092c49 withdrawal, 2026-10-29; mechanism corrected same day
/// by the refactor lane, 75dbf637): this lock was briefly a
/// `KeyedRawSpinlock<GrqKey>` (§3 typed lock keys pilot). That swap is
/// CONVICTED as the trigger of the d2a58ccb stochastic panic family by
/// a 13-boot paired adjudication (identical tree differing only by the
/// swap): WITH the swap, 6/6 x86 exitwalk boots panic in the CFS
/// timeline btree walk (navigate.rs:552 next_unchecked unwrap, stack:
/// pick_next_cpu → Iter::next) and the riscv64 orphan-storm churn
/// panics on a torn-context wild epc right after an R74-DOUBLE-SEED;
/// WITHOUT it (revert arm + all pre-swap kernels), 7/7 boots run the
/// full churn clean on both arches.
///
/// TRUE ROOT CAUSE (75dbf637 — the initial "latent GRQ race" theory
/// was WRONG): `RawSpinlock::lock()` returns `()` — the statement IS
/// the acquire — while `KeyedRawSpinlock::lock()` returns a guard, and
/// the swap kept the bare `self.lock.lock();` statement: the guard was
/// DISCARDED at the end of the statement and its Drop released the
/// lock immediately, so EVERY GRQ critical section ran unexcluded.
/// The R74-DOUBLE-SEED shape was the expected downstream of a lock
/// that never held. There is NO latent GRQ pick/ownership race; the
/// protocol is exonerated. The guard class is now `#[must_use]` (the
/// discard no longer compiles silently) and protocol-wrapping
/// subsystems use `lock_claim()`; a corrected re-adoption is staged
/// with its own fresh paired adjudication. This struct stays on
/// RawSpinlock until that lands.
pub struct GlobalRunQueue {
    /// Protects dl_rq, rt_rq, cfs_rq, nr_running
    lock: crate::sync::spinlock::RawSpinlock,
    /// Deadline queue (EDF sorted by deadline)
    pub dl_rq: crate::sched::entities::DlRunQueue,
    /// Real-time queue (bitmap + per-priority lists)
    pub rt_rq: crate::sched::entities::RtRunQueue,
    /// CFS queue (BTreeMap sorted by vruntime)
    pub cfs_rq: crate::sched::entities::CfsRunQueue,
    /// Total runnable task count (read atomically for idle checks)
    pub nr_running: core::sync::atomic::AtomicUsize,
    /// Bitmap of idle CPUs: bit N = 1 means CPU N is idle
    pub idle_cpus: core::sync::atomic::AtomicU32,
}

/// Rotating scan start for find_idle_cpu (review batch 8 — idle spreading).
static LAST_IDLE_HINT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

unsafe impl Sync for GlobalRunQueue {}

impl GlobalRunQueue {
    /// Create a new GlobalRunQueue (NOT const — BTreeMap::new() is not const).
    pub     fn new() -> Self {
        Self {
            lock: crate::sync::spinlock::RawSpinlock::new(),
            dl_rq: crate::sched::entities::DlRunQueue::new(),
            rt_rq: {
                let mut rt = crate::sched::entities::RtRunQueue::new();
                rt.init();
                rt
            },
            cfs_rq: crate::sched::entities::CfsRunQueue::new(),
            nr_running: core::sync::atomic::AtomicUsize::new(0),
            idle_cpus: core::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Lock the global RQ (disable interrupts + preempt + lock).
    #[inline]
    #[track_caller]
    pub fn lock_irqsave(&self) -> GrqGuard<'_> { // track_caller: syscost GRQ site attribution
        self.lock_irqsave_site(grq_site(GRQ_CALLER_OTHER, GRQ_STEP_TRY))
    }

    /// Lock the global RQ, publishing the caller's site id for the
    /// holder-naming rig (grq-lock-1st).
    ///
    /// The site is stamped BEFORE the lock attempt, so a cpu that spins
    /// forever on a held GRQ shows up as "trying-to-acquire" with its
    /// own frame pointer — that is the signature of the self-deadlock
    /// (a GRQ re-acquire from inside a GRQ section) and of the
    /// cross-CPU ABAA, and it names the CALLER that reached the lock.
    #[inline]
    #[track_caller]
    pub fn lock_irqsave_site(&self, site: u32) -> GrqGuard<'_> {
        let flags = crate::arch::cpu::save_and_disable_irq();
        crate::interrupt::preempt::preempt_count_add(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        let cpu = crate::arch::cpu_id() as usize;
        // Publish "trying" BEFORE the spin (the frame pointer is the
        // caller's, so the dump names who wanted the lock).
        grq_site_mark_fp_at(cpu, site);
        self.lock.lock();
        // Acquired: the section is now entered.
        grq_site_mark_at(cpu, site | GRQ_STEP_HELD);
        GRQ_ACQ_JIF[cpu].store(crate::timer::get_jiffies(), core::sync::atomic::Ordering::Relaxed);
        // grq-source-fix: no separate "held" tracker is written here.
        // The lock word itself names this cpu as the owner (owner-in-
        // word, RawSpinlock::held_word) — one authority, published by
        // the acquire's single cmpxchg. The round-6 GRQ_HELD bool made
        // the release two steps (tracker-clear, then word-store) whose
        // gap produced the gl3 orphaned-word wedge; with the owner in
        // the word there is nothing left to disagree with it.
        GrqGuard { grq: self as *const Self as *mut Self, flags, _marker: core::marker::PhantomData }
    }

    /// Nested variant of [`lock_irqsave`]: acquires with the same
    /// protocol, but ONLY callable while an `Outer`-class keyed lock is
    /// held (the proof is minted by that outer guard) and only when
    /// `GrqKey: CanNestInside<Outer>` — INV-LOCK-5's forward direction
    /// as a compile fact. The proof is zero-sized; the runtime protocol
    /// is identical to lock_irqsave.
    #[inline]
    #[track_caller]
    pub fn lock_irqsave_nested<'a, Outer: crate::sync::lockkey::NestKey>(
        &self,
        _proof: &crate::sync::lockkey::NestProof<'a, Outer>,
    ) -> GrqGuard<'_>
    where
        crate::sync::lockkey::GrqKey: crate::sync::lockkey::CanNestInside<Outer>,
    {
        self.lock_irqsave()
    }

    /// Nested variant that also publishes the caller's site id for the
    /// holder-naming rig (grq-lock-1st) — see [`lock_irqsave_site`].
    #[inline]
    #[track_caller]
    pub fn lock_irqsave_nested_site<'a, Outer: crate::sync::lockkey::NestKey>(
        &self,
        _proof: &crate::sync::lockkey::NestProof<'a, Outer>,
        site: u32,
    ) -> GrqGuard<'_>
    where
        crate::sync::lockkey::GrqKey: crate::sync::lockkey::CanNestInside<Outer>,
    {
        self.lock_irqsave_site(site)
    }

    /// Lock without IRQ save (for non-interrupt contexts like init).
    #[inline]
    pub fn lock_plain(&self) -> GrqPlainGuard<'_> {
        crate::interrupt::preempt::preempt_count_add(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        self.lock.lock();
        GrqPlainGuard { grq: self as *const Self as *mut Self, _marker: core::marker::PhantomData }
    }

    /// Try-lock with the full lock_irqsave protocol (IRQ save + preempt
    /// count + lock), returning None instead of spinning when the GRQ is
    /// held. For diagnostic paths that must never block on a possibly
    /// wedged holder (the DEBUG taskdump runs from the spinlock deadlock
    /// watchdog with IRQs off) but still need the lock to legally read the
    /// per-class queue trees.
    #[inline]
    pub fn try_lock_irqsave(&self) -> Option<GrqGuard<'_>> {
        let flags = crate::arch::cpu::save_and_disable_irq();
        crate::interrupt::preempt::preempt_count_add(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        if self.lock.try_lock() {
            Some(GrqGuard {
                grq: self as *const Self as *mut Self,
                flags,
                _marker: core::marker::PhantomData,
            })
        } else {
            crate::interrupt::preempt::preempt_count_sub(
                crate::interrupt::preempt::PREEMPT_OFFSET,
            );
            crate::arch::cpu::restore_irq(flags);
            None
        }
    }

    // ---- idle CPU bitmap ----

    /// Mark a CPU as idle.
    pub fn mark_idle(&self, cpu: usize) {
        if cpu < MAX_CPUS {
            self.idle_cpus.fetch_or(1u32 << cpu, core::sync::atomic::Ordering::Release);
        }
    }

    /// Mark a CPU as busy (no longer idle).  Returns true if it was idle.
    pub fn clear_idle(&self, cpu: usize) -> bool {
        if cpu < MAX_CPUS {
            let mask = 1u32 << cpu;
            let prev = self.idle_cpus.fetch_and(!mask, core::sync::atomic::Ordering::AcqRel);
            (prev & mask) != 0
        } else {
            false
        }
    }

    /// Find an idle CPU in the given affinity mask.
    ///
    /// Review batch 8: the scan start rotates (last chosen + 1) so wakeups
    /// spread across idle CPUs instead of piling every task onto the
    /// lowest-numbered one.
    pub fn find_idle_cpu(&self, affinity: u32) -> Option<usize> {
        let idle = self.idle_cpus.load(core::sync::atomic::Ordering::Acquire);
        let candidates = idle & affinity;
        if candidates == 0 {
            return None;
        }
        // Rotate so the scan starts just past the CPU chosen last time.
        let start = LAST_IDLE_HINT.load(core::sync::atomic::Ordering::Relaxed) % MAX_CPUS;
        let rotated = candidates.rotate_right(start as u32);
        let pick = rotated.trailing_zeros() as usize;
        let cpu = (pick + start) % MAX_CPUS;
        LAST_IDLE_HINT.store(cpu + 1, core::sync::atomic::Ordering::Relaxed);
        Some(cpu)
    }

    /// Claim an idle CPU in `affinity` for a wake dispatch: find one and
    /// atomically clear its idle bit, so two racing wakes cannot both
    /// target it. Returns the claimed CPU, or None when no idle CPU is
    /// available (or the racing claim lost).
    pub fn claim_idle_cpu(&self, affinity: u32) -> Option<usize> {
        let cpu = self.find_idle_cpu(affinity)?;
        if self.clear_idle(cpu) {
            Some(cpu)
        } else {
            None
        }
    }

    /// R3 (rt-r3 lane): claim with a PLACEMENT preference — try `prefer`'s
    /// idle bit first (clear_idle only succeeds when the bit was set, so
    /// this is the same atomic claim as the rotating path), then fall
    /// back to the rotating scan. RT/DL timer wakes fire the hardirq on
    /// the sleeper's own cpu: claiming the placement turns the dispatch
    /// into a self-poke the idle loop consumes at the IRQ exit, instead
    /// of steering to a rotated idle cpu that needs an IPI + halt-exit.
    pub fn claim_idle_cpu_prefer(&self, affinity: u32, prefer: i32) -> Option<usize> {
        if prefer >= 0 && (prefer as usize) < MAX_CPUS && (affinity & (1u32 << prefer)) != 0 {
            if self.clear_idle(prefer as usize) {
                return Some(prefer as usize);
            }
        }
        self.claim_idle_cpu(affinity)
    }

    /// Total load across all classes (for informational purposes).
    /// Expose total nr_running for diagnostic use.
    pub fn grq_nr_running() -> usize {
        grq().rq_load()
    }

    /// Total load across all classes (for informational purposes).
    pub fn rq_load(&self) -> usize {
        let cfs = self.cfs_rq.nr_running() as usize;
        let rt = self.rt_rq.nr_running() as usize;
        let dl = self.dl_rq.nr_running() as usize;
        cfs + rt + dl
    }
}

// ==================== SCHED_DEADLINE CBS throttling (batch 8 HIGH) ====================
//
// CBS was completely defeated: every enqueue (including __schedule's
// requeue of prev) called replenish_runtime(), so a single DL task could
// burn 100% of every CPU forever. The fix: a task whose runtime budget is
// exhausted is THROTTLED — it is moved off the runqueue (state left
// INTERRUPTIBLE so __schedule does not demand a requeue link) and a kernel
// timer is armed at its deadline; the timer's wake re-enqueues it with a
// refilled budget (replenishment now happens exactly once per period).


// ==================== RT bandwidth throttling (batch 8 MED) ====================
//
// SCHED_FIFO/RR had no throttling: a CAP_SYS_NICE-holding RT task could
// starve every CPU indefinitely. Mirror Linux's sched_rt_runtime_us /
// sched_rt_period_us defaults (950ms per 1s). 70th (ohw84-87, the OH
// boot's FIFO evt_listen spinner): Linux accounts the runtime PER-CPU
// (each root rt_rq carries its own runtime; exhausting it throttles RT
// on THAT cpu, leaving the 5% CFS rescue slice alive) — the earlier
// GLOBAL budget (950ms * MAX_CPUS) let ONE spinning FIFO thread on one
// cpu burn only half the pool and NEVER throttle, stranding every CFS
// task on that cpu indefinitely (ohw86 WAKE-STRANDED: 4.8-6.4 s strands
// behind render_service's FIFO listener). Per-CPU accounting now.


// ==================== GRQ Guards ====================

/// Guard for `lock_irqsave()` — unlock + preempt enable + IRQ restore on drop.
pub struct GrqGuard<'a> {
    grq: *mut GlobalRunQueue,
    flags: bool,
    _marker: core::marker::PhantomData<&'a GlobalRunQueue>,
}

impl<'a> GrqGuard<'a> {
    /// Release the spinlock but keep interrupts disabled.
    /// Returns the saved IRQ flags. Caller must call restore_irq() later.
    ///
    /// ORDER FIX (glib-mutex lane, 2026-11-14): the preempt_count sub runs
    /// BEFORE the raw unlock. The old order (unlock, then sub) left a
    /// two-instruction window in which the GRQ was observable-free while
    /// the switching task still carried its +1 — and in __schedule that
    /// window sits AFTER the sched-slot flip to `next` (sched.rs:3113), so
    /// a concurrent waker's WAKE-HEAL adjudication saw the task as
    /// "count=1, owned by no slot" and zeroed the LIVE guard count. Under
    /// KVM with a loaded host the vCPU can be descheduled anywhere,
    /// stretching the window to milliseconds — captured live (garepro26,
    /// +252.1 s): WAKE-HEAL-ORIGIN pid=328 osite=sched.rs:2994 ocpu=5
    /// oage_jif=0 (the heal zeroing the in-flight guard), then 5 ms later
    /// PREEMPT-UNDERFLOW "count_was=0 sub=1" at sched.rs:397 (this
    /// function's own sub arriving late) with UNDERFLOW-ORIGIN naming the
    /// same acquire. With the sub first, a waker entering after the raw
    /// unlock always observes count==0 and the heal predicate never fires
    /// on a live guard. Safe: preempt_enable here is a bare sub (no
    /// resched), IF stays 0 across the whole bracket.
    pub fn unlock_irqretain(self) -> bool {
        let flags = self.flags;
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        let cpu = crate::arch::cpu_id() as usize;
        // grq-source-fix: the release is site-clear + the single
        // word store below. No tracker step exists that could clear
        // ahead of the word (the orphaned-word producer).
        grq_site_mark_at(cpu, 0);
        // SAFETY: we hold the GRQ lock via this guard, so the unlock is valid.
        crate::sched::edges::grq_ptr_mut(self.grq).lock.unlock();
        core::mem::forget(self);
        flags
    }
}

impl core::ops::Deref for GrqGuard<'_> {
    type Target = GlobalRunQueue;
    fn deref(&self) -> &Self::Target {
        // SAFETY: we hold the GRQ lock via this guard, so shared access is valid.
        crate::sched::edges::grq_ptr_shared(self.grq)
    }
}

impl core::ops::DerefMut for GrqGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: we hold the GRQ lock via this guard, so exclusive access is guaranteed.
        crate::sched::edges::grq_ptr_mut(self.grq)
    }
}

impl Drop for GrqGuard<'_> {
    fn drop(&mut self) {
        // ORDER FIX (glib-mutex lane): sub before unlock — see
        // GrqGuard::unlock_irqretain; same rationale for the plain drop
        // path (the [unlock..sub] gap must never be observable with the
        // count still elevated).
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        let cpu = crate::arch::cpu_id() as usize;
        // grq-source-fix: site-clear + the single word store below —
        // no separate tracker step (see lock_irqsave_site).
        grq_site_mark_at(cpu, 0);
        crate::sched::edges::grq_ptr_mut(self.grq).lock.unlock();
        crate::arch::cpu::restore_irq(self.flags);
    }
}

/// Guard for plain `lock_plain()` (no IRQ save).
pub struct GrqPlainGuard<'a> {
    grq: *mut GlobalRunQueue,
    _marker: core::marker::PhantomData<&'a GlobalRunQueue>,
}

impl core::ops::Deref for GrqPlainGuard<'_> {
    type Target = GlobalRunQueue;
    fn deref(&self) -> &Self::Target {
        // SAFETY: we hold the GRQ lock via this guard, so shared access is valid.
        crate::sched::edges::grq_ptr_shared(self.grq)
    }
}

impl core::ops::DerefMut for GrqPlainGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: we hold the GRQ lock via this guard, so exclusive access is guaranteed.
        crate::sched::edges::grq_ptr_mut(self.grq)
    }
}

impl Drop for GrqPlainGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: we hold the GRQ lock via this guard; Drop runs exactly once.
        crate::sched::edges::grq_ptr_mut(self.grq).lock.unlock();
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
    }
}

// ==================== Per-CPU State ====================

/// Minimal per-CPU state — no queues, just task pointers.
pub struct PerCpuState {
    /// Currently running task
    pub current: *mut Task,
    /// Per-CPU idle task (PID 0)
    pub idle: *mut Task,
    /// Per-CPU stop task (for hotplug)
    pub stop: *mut Task,
}

impl PerCpuState {
    pub(crate) const fn new() -> Self {
        Self {
            current: core::ptr::null_mut(),
            idle: core::ptr::null_mut(),
            stop: core::ptr::null_mut(),
        }
    }
}

// ==================== Static Instances ====================

// GRQ and GRQ_READY moved into the framework edges module (unsafe-slim
// L1): the MaybeUninit boot-provenance accesses live in
// sched::edges under INV-SCE-3; this file reads them through
// edges::grq_ref()/edges::grq_mut() and initializes through the
// claim/stage/publish protocol.

/// gw1 (oh-launch round 6) introduced GRQ_HELD — a per-CPU positive-
/// knowledge bool written at acquire/release — so the death path could
/// force-release a strand the dying CPU provably held. DELETED by the
/// grq-source-fix (2026-11-19): making the tracker a SECOND authority
/// turned the release into two steps (tracker-clear, then word-store),
/// and a fault landing in that gap stranded a word no tracker claimed
/// — the gl3/gl4 orphaned-word shape (seven waiters, no holder), which
/// then needed a self-invented heal (951d9ec33) the reference kernel
/// has no counterpart for. The construction now carries the owner IN
/// the lock word itself (RawSpinlock: 0 = free, cpu+1 = held by cpu;
/// acquire = one cmpxchg, release = one ordered store) — a single
/// authority that cannot disagree with itself. Every former tracker
/// consumer is derived: `grq_held_here` (printk deferral gate), the
/// crime-scene dump's holder/waiter roles, and the terminal release's
/// ownership proof. The unreachable-state detector lives in
/// `RawSpinlock::holder_cpu` (debug_assert, hot-path rules).

/// A diagnostic report CAPTURED under the GRQ and EMITTED after the lock
/// is released (grq-lock-1st).
///
/// printk is not lock-free: its ring-buffer write ends in
/// `KMSG_READ_WQ.wake_up_all()`, whose waitqueue lock is held across
/// `wake_up_process` -> `wake_up_enqueue_opt` -> `grq().lock_irqsave()`.
/// A report issued while the GRQ is held therefore re-acquires the GRQ
/// on the same cpu — the non-reentrant spinlock then spins forever with
/// IRQs off (the "IRQs-dead GRQ holder" wedge) — and against another cpu
/// it is the textbook ABAA (this cpu: GRQ -> WQ; the other: WQ -> GRQ).
/// The capture-then-emit shape keeps every field's value exact (they are
/// read under the lock) while moving the report itself out of the
/// critical section.
#[derive(Clone, Copy)]
pub struct GrqPendingReport {
    /// Which report this is (see the `GRQ_RPT_*` constants).
    pub kind: u8,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub d: u64,
    pub e: u64,
    pub comm: [u8; 16],
}

/// Report kinds for [`GrqPendingReport`].
pub const GRQ_RPT_COUNTER_RESYNC: u8 = 1;
pub const GRQ_RPT_SHOUT_RETIRED: u8 = 2;
pub const GRQ_RPT_STRAND_ID: u8 = 3;
pub const GRQ_RPT_STRAND_ID_NOTRACE: u8 = 4;


// ==================== GRQ SITE STAMP — the holder-naming rig (grq-lock-1st) =
//
// THE PROBLEM THIS SOLVES. The generic deadlock detector names the lock
// and the holder CPU, but it cannot name the HOLDER'S STACK: the capture
// is an IPI (REMOTE-BT), and a GRQ holder runs with IRQs OFF (the lock is
// `spin_lock_irqsave`), so the IPI is never serviced and the capture
// times out — "REMOTE-BT TIMEOUT (not servicing IPIs)". That is why
// every GRQ wedge so far ended with the holder UNNAMED: the only
// instrument that could have named it is the one instrument a GRQ holder
// cannot answer.
//
// THE SHAPE. The holder publishes its own progress. Every GRQ critical
// section stamps a per-CPU slot with the id of the operation it is
// performing, plus the frame pointer of the code performing it. A
// wedged system then reads those slots from ANY cpu — no IPI, no
// interrupt, no lock — and walks the holder's own stack (its kernel
// stack is mapped, and for a wedged holder it is frozen).
//
// The same slots also serve the SPINNERS: a cpu that is waiting for the
// lock stamps "trying" before it spins, so the dump separates the
// holder from the waiters without any cross-cpu handshake.
//
// COST. One relaxed store per checkpoint on a per-CPU word (the existing
// `SCHED_CS_PHASE` shape) plus one frame-pointer read at the
// checkpoints where the holder can actually stall. Measured against the
// cctx `SCH_HOLD_NS` bracket in run/GRQ-LOCK-1ST.md.

/// Caller classes (site id bits [8:15]).
pub const GRQ_CALLER_SCHED: u32 = 1;
pub const GRQ_CALLER_WAKE: u32 = 2;
pub const GRQ_CALLER_DEQUEUE: u32 = 3;
pub const GRQ_CALLER_ENQUEUE: u32 = 4;
pub const GRQ_CALLER_TICK: u32 = 5;
pub const GRQ_CALLER_OTHER: u32 = 6;

/// Steps within a section (site id bits [0:7]).
pub const GRQ_STEP_TRY: u32 = 0;
pub const GRQ_STEP_HELD: u32 = 1;
pub const GRQ_STEP_NESTED_LOCK: u32 = 2;
pub const GRQ_STEP_CLASS_OP: u32 = 3;
pub const GRQ_STEP_PICK: u32 = 4;
pub const GRQ_STEP_DISPATCH: u32 = 5;
pub const GRQ_STEP_PUBLISH: u32 = 6;

/// Build a site id from a caller class and a step.
#[inline(always)]
pub const fn grq_site(caller: u32, step: u32) -> u32 {
    (caller << 8) | step
}

/// The last site each cpu reached (0 = not in a GRQ section).
pub static GRQ_SITE: [core::sync::atomic::AtomicU32; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_CPUS];

/// The frame pointer of the code that reached that site. Published by
/// the holder itself, so the wedge dump can walk the holder's stack
/// WITHOUT an IPI (which a GRQ holder, IRQs off, can never service).
pub static GRQ_SITE_FP: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

/// Jiffies at the section's acquire — the hold's age.
pub static GRQ_ACQ_JIF: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

/// Read the current frame pointer (rbp on x86_64, s0 on riscv64,
/// x29 on aarch64).
///
/// IRREDUCIBLE F-ROW (F-REG-1): there is no safe shape for reading a
/// CPU register — Rust exposes no intrinsic for rbp/s0/x29 and the
/// register file is outside the language's memory model. The reference
/// kernel reaches the same value in C via
/// `__builtin_frame_address(0)` (gcc lowers it to the identical
/// `mov rbp` / `mv s0` / `mov x29`), used by its frame-pointer
/// stackwalker (`arch/arm64/kernel/stacktrace.c`, x86
/// `dumpstack.c`'s fp path); this read feeds the GRQ crime-scene
/// backtrace, the same consumer. Contract: read-only, `nomem` +
/// `nostack` + `preserves_flags` — no memory, stack or flag effect,
/// so the only soundness requirement is that the output register is
/// written, which the operand enforces.
#[inline(always)]
fn grq_current_fp() -> u64 {
    let fp: u64;
    #[cfg(feature = "x86_64")]
    // F-REG-1 (x86_64): mov rbp — the reference's __builtin_frame_address(0).
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack, preserves_flags));
    }
    #[cfg(feature = "riscv64")]
    // F-REG-1 (riscv64): mv s0 — same lowered builtin.
    unsafe {
        core::arch::asm!("mv {}, s0", out(reg) fp, options(nomem, nostack, preserves_flags));
    }
    #[cfg(feature = "aarch64")]
    // F-REG-1 (aarch64): mov x29 — same lowered builtin.
    unsafe {
        core::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack, preserves_flags));
    }
    fp
}

/// Stamp the site on `cpu` (one relaxed store — the hot-path shape).
#[inline(always)]
pub(crate) fn grq_site_mark_at(cpu: usize, site: u32) {
    if cpu < MAX_CPUS {
        GRQ_SITE[cpu].store(site, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Stamp the site AND the caller's frame pointer (two stores) — used at
/// the checkpoints where the holder can actually stall, so the dump can
/// walk the holder's stack.
#[inline(always)]
fn grq_site_mark_fp_at(cpu: usize, site: u32) {
    if cpu < MAX_CPUS {
        GRQ_SITE[cpu].store(site, core::sync::atomic::Ordering::Relaxed);
        GRQ_SITE_FP[cpu].store(grq_current_fp(), core::sync::atomic::Ordering::Relaxed);
    }
}

/// Name a site id for the dump.
pub fn grq_site_name(site: u32) -> &'static str {
    let caller = site >> 8;
    let step = site & 0xFF;
    match (caller, step) {
        (GRQ_CALLER_SCHED, GRQ_STEP_TRY) => "schedule:trying-to-acquire",
        (GRQ_CALLER_SCHED, GRQ_STEP_HELD) => "schedule:acquired",
        (GRQ_CALLER_SCHED, GRQ_STEP_NESTED_LOCK) => "schedule:nested-lock",
        (GRQ_CALLER_SCHED, GRQ_STEP_CLASS_OP) => "schedule:class-op",
        (GRQ_CALLER_SCHED, GRQ_STEP_PICK) => "schedule:pick",
        (GRQ_CALLER_SCHED, GRQ_STEP_PUBLISH) => "schedule:publish",
        (GRQ_CALLER_WAKE, GRQ_STEP_TRY) => "wake:trying-to-acquire",
        (GRQ_CALLER_WAKE, GRQ_STEP_HELD) => "wake:acquired",
        (GRQ_CALLER_WAKE, GRQ_STEP_NESTED_LOCK) => "wake:nested-lock",
        (GRQ_CALLER_WAKE, GRQ_STEP_CLASS_OP) => "wake:class-op",
        (GRQ_CALLER_WAKE, GRQ_STEP_PICK) => "wake:pick",
        (GRQ_CALLER_WAKE, GRQ_STEP_DISPATCH) => "wake:dispatch",
        (GRQ_CALLER_SCHED, GRQ_STEP_DISPATCH) => "schedule:dispatch",
        (GRQ_CALLER_DEQUEUE, GRQ_STEP_TRY) => "dequeue_task:trying-to-acquire",
        (GRQ_CALLER_DEQUEUE, GRQ_STEP_HELD) => "dequeue_task:acquired",
        (GRQ_CALLER_ENQUEUE, GRQ_STEP_TRY) => "enqueue_task:trying-to-acquire",
        (GRQ_CALLER_ENQUEUE, GRQ_STEP_HELD) => "enqueue_task:acquired",
        (GRQ_CALLER_TICK, GRQ_STEP_TRY) => "tick:trying-to-acquire",
        (GRQ_CALLER_TICK, GRQ_STEP_HELD) => "tick:acquired",
        (GRQ_CALLER_OTHER, GRQ_STEP_TRY) => "grq:trying-to-acquire",
        (GRQ_CALLER_OTHER, GRQ_STEP_HELD) => "grq:acquired",
        _ => "grq:other",
    }
}

/// The GRQ crime-scene dump (grq-lock-1st).
///
/// Prints, for every cpu that is inside (or trying to enter) a GRQ
/// section: the cpu, its task, the site (named), the age of the hold,
/// and — for the HOLDER — the holder's own stack, walked from the frame
/// pointer the holder published at that site.
///
/// The walk is IPI-FREE ON PURPOSE. A GRQ holder runs with IRQs off, so
/// the REMOTE-BT capture (an IPI) always times out on a GRQ wedge —
/// which is exactly why every GRQ wedge so far ended with the holder
/// UNNAMED. The holder's kernel stack is mapped and (for a wedged
/// holder) frozen, so another cpu can read it directly.
///
/// Allocation-free and lock-free: it runs in the fragile states the
/// deadlock detector and the oops policy report from. Output goes to
/// the raw serial port (`uart_putc_raw`) — printk may be the very thing
/// that is wedged.
pub fn grq_dump_holder(why: &str) {
    // Nothing to report unless some cpu is inside (or trying to enter) a
    // GRQ section — the dump is called from generic deadlock paths.
    let mut any = false;
    for cpu in 0..MAX_CPUS {
        if GRQ_SITE[cpu].load(core::sync::atomic::Ordering::Relaxed) != 0 {
            any = true;
            break;
        }
    }
    if !any {
        return;
    }
    let now = crate::timer::get_jiffies();
    // grq-source-fix: the holder is DERIVED from the lock word (the
    // single authority — owner-in-word). Before sched init there is no
    // word to read; the site stamps alone still name the participants.
    let holder = if crate::sched::edges::grq_is_ready() {
        // SAFETY: GRQ_READY checked; read-only lock word access.
        grq().lock.holder_cpu()
    } else {
        None
    };
    grq_raw_puts(b"\nGRQ-CRIME-SCENE ");
    grq_raw_puts(why.as_bytes());
    grq_raw_puts(b"\n");
    // R11 (deadlock-layer unblinding): the GRQ lock WORD value — the one
    // datum that discriminates "a real holder the site stamps missed"
    // (word == 1) from "the word itself was scribbled" (word ∉ {0,1}).
    // fr4/fr5's wedges printed all-waiting rows and no holder on standard
    // builds; this line closes that blind spot.
    grq_raw_puts(b"GRQ-LOCK locked=0x");
    {
        let mut v = grq().lock.locked_word() as u64;
        let mut sh: i32 = 8;
        while sh > 0 {
            sh -= 4;
            let nb = ((v >> sh) & 0xF) as u8;
            grq_raw_puts(&[(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 })]);
        }
    }
    grq_raw_puts(b"\n");
    for cpu in 0..MAX_CPUS {
        let site = GRQ_SITE[cpu].load(core::sync::atomic::Ordering::Relaxed);
        if site == 0 {
            continue;
        }
        let holds = holder == Some(cpu);
        let acq = GRQ_ACQ_JIF[cpu].load(core::sync::atomic::Ordering::Relaxed);
        let age = now.wrapping_sub(acq);
        let cur = crate::sched::sched::cpu_state(cpu).current;
        grq_raw_puts(b"GRQ-HOLDER cpu=");
        grq_raw_putdec(cpu as u64);
        grq_raw_puts(b" role=");
        grq_raw_puts(if holds { b"holder" } else { b"waiting" });
        grq_raw_puts(b" pid=");
        if cur.is_null() {
            grq_raw_puts(b"none");
        } else {
            grq_raw_putdec(crate::sched::edges::task_shared(cur).pid() as u64);
        }
        grq_raw_puts(b" site=");
        grq_raw_puts(grq_site_name(site).as_bytes());
        grq_raw_puts(b" age_jif=");
        grq_raw_putdec(age);
        grq_raw_puts(b"\n");
        if !holds {
            continue;
        }
        // The holder's own stack: walk the frame-pointer chain the
        // holder published. The frame words are read from the holder's
        // kernel stack; a wedged holder does not move, so the walk is
        // consistent. Every hop is validated (alignment, kernel range,
        // strictly ascending, sane gap) exactly like the REMOTE-BT
        // walker.
        let mut fp = GRQ_SITE_FP[cpu].load(core::sync::atomic::Ordering::Relaxed);
        grq_raw_puts(b"GRQ-HOLDER-BT cpu=");
        grq_raw_putdec(cpu as u64);
        grq_raw_puts(b"\n");
        let mut n = 0u32;
        while n < 20 {
            if fp == 0 || fp % 8 != 0 || fp < 0x8000_0000 {
                break;
            }
            // SAFETY: fp is validated (aligned, in the kernel range); the
            // two frame-word reads are the standard unwind operation on a
            // live kernel stack.
            let (ret, next) =
                { (
                crate::sync::edges::read_volatile_u64((fp + 8) as usize),
                crate::sync::edges::read_volatile_u64(fp as usize),
            ) };
            if ret == 0 {
                break;
            }
            grq_raw_puts(b"GRQ-BT  #");
            grq_raw_putdec(n as u64);
            grq_raw_puts(b" ra=0x");
            grq_raw_puthex(ret);
            grq_raw_puts(b"\n");
            n += 1;
            if next <= fp || next - fp > 0x4000 {
                break;
            }
            fp = next;
        }
        if n == 0 {
            grq_raw_puts(b"GRQ-BT  (no frames)");
            // R13 (fr12's unnameable inner-spin holder): when the frame
            // chain is unwalkable (fp stamp 0/invalid or the chain
            // corrupt), dump the holder's stack TOP raw — the deepest
            // 16 qwords below the top carry the most recent context
            // (an IRQ-interrupted frame's pt_regs + the interrupted
            // return addresses), symbolizable offline. The holder is
            // wedged, so the reads are consistent.
            let cur = crate::sched::sched::cpu_state(cpu).current;
            if !cur.is_null() {
                let top = crate::sched::edges::task_shared(cur).kernel_stack_top();
                if top != 0 {
                    grq_raw_puts(b" top-raw:\n");
                    let mut off = 168 + 16 * 8; // below the pt_regs anchor
                    while off >= 8 * 8 {
                        off -= 8;
                        // SAFETY: the holder's own kernel stack, in-bounds
                        // by construction (top - off within the 16K stack).
                        let v = crate::sync::edges::read_volatile_u64(top - off);
                        grq_raw_puts(b"GRQ-BT   @-");
                        grq_raw_putdec(off as u64);
                        grq_raw_puts(b" 0x");
                        grq_raw_puthex(v);
                        grq_raw_puts(b"\n");
                    }
                }
            } else {
                grq_raw_puts(b"\n");
            }
        }
    }
}

// ==================== GRQ WEDGE INJECTION (debug-grq-wedge) ====================
//
// The holder-naming rig needs a REPRODUCIBLE wedge to prove it names the
// holder. This hook creates one on demand: with `grqwedge=1` on the
// cmdline, ONE cpu takes the GRQ and spins inside the schedule()'s
// critical section forever. Every other cpu then spins on the GRQ,
// the deadlock detector fires, and the GRQ crime-scene dump prints the
// holder's site + its own backtrace.
//
// It is a COMPILE-TIME-gated diagnostic (the `debug-grq-wedge` feature),
// so the shipping build carries none of it — the rig it verifies (the
// site stamp and the dump) is always on.

/// Armed by `grqwedge=1` on the cmdline (one-shot, at sched init).
#[cfg(feature = "debug-grq-wedge")]
static GRQ_WEDGE_ARMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// The wedge has already fired (one-shot).
#[cfg(feature = "debug-grq-wedge")]
static GRQ_WEDGE_FIRED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Arm the injection (called once from `sched::init`).
#[cfg(feature = "debug-grq-wedge")]
pub fn grq_wedge_arm() {
    GRQ_WEDGE_ARMED.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// Should THIS cpu inject the wedge now? One-shot.
#[cfg(feature = "debug-grq-wedge")]
#[inline]
pub fn grq_wedge_inject() -> bool {
    if !GRQ_WEDGE_ARMED.load(core::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    !GRQ_WEDGE_FIRED.swap(true, core::sync::atomic::Ordering::Relaxed)
}

/// Does THIS cpu currently hold the GRQ lock?
///
/// grq-lock-1st: the printk path needs this to decide whether it may
/// touch a waitqueue lock. printk's ring-buffer write ends in
/// `KMSG_READ_WQ.wake_up_all()`, and `WaitQueueHead::wake_up` holds the
/// queue's list lock ACROSS `wake_up_process` -> `wake_up_enqueue_opt`
/// -> `grq().lock_irqsave()`. A printk issued while this cpu holds the
/// GRQ therefore re-acquires the GRQ on the SAME cpu (self-deadlock on
/// a non-reentrant spinlock, IRQs off forever) — and against another
/// cpu it is the textbook ABAA (this cpu: GRQ -> WQ; the other: WQ ->
/// GRQ).
///
/// grq-source-fix: the answer is DERIVED from the lock word (the
/// single authority — the word names its owner), not from a second
/// tracker. Same-cpu query on a word our own acquire wrote: a false
/// positive is impossible and a false negative only costs one
/// deferred wake.
#[inline]
pub fn grq_held_here() -> bool {
    if !crate::sched::edges::grq_is_ready() {
        return false;
    }
    let cpu = crate::arch::cpu_id() as usize;
    // SAFETY: GRQ_READY checked; read-only lock word access.
    grq().lock.held_by(cpu)
}

/// Initialize the global run queue (called once during boot).
fn grq_init() {
    // INV-SCE-3 (edges): the claim CAS takes the single-initializer
    // role; the staged value publishes with the Release every
    // edges::grq_ref()/grq_mut() reader gates on.
    if crate::sched::edges::grq_claim_init() {
        // Another CPU is initializing; spin until done
        while !crate::sched::edges::grq_is_ready() {
            core::hint::spin_loop();
        }
        return;
    }
    // RT-HEADS FIX (oh-display b61-b66 root cause): the 100 RT priority
    // list heads are SELF-REFERENTIAL ListHeads. GlobalRunQueue::new()
    // builds rt_rq as a stack temporary and calls RtRunQueue::init() ON
    // THE TEMPORARY, so every head's next/prev hold the temporary's
    // address; the move into the GRQ static copies those VALUES, leaving
    // all 100 heads pointing into a dead early-boot stack frame
    // (~0x10a1xx-0x10a7xx on the BSP boot stack). Every RT list was then
    // threaded through a phantom node in boot-stack memory: the first
    // add_tail at a priority scribbled the stale slot to reach the real
    // node, and the lists stayed functional only while that stack region
    // retained the scribble. The first sufficiently deep BSP excursion
    // overwrote it and every subsequent walk dereferenced boot-stack
    // garbage (POISONED rq entry reports, KERNPANICs in the pick path at
    // T+15-19min, byte-identical every boot). Nothing is enqueued yet
    // (GRQ_READY gates all enqueue paths), so re-self-linking the heads
    // at their final address here is safe and complete.
    crate::sched::edges::grq_stage(GlobalRunQueue::new());
    // RT-HEADS FIX tail (see above): re-self-link the 100 priority
    // heads at their final address. Still on the claim-winning path
    // (INV-SCE-3 writer side, pre-publish).
    crate::sched::edges::grq_mut().rt_rq.init();
    crate::sched::edges::grq_publish_init();
}

/// Get a shared reference to GRQ.
pub fn grq() -> &'static GlobalRunQueue {
    if !crate::sched::edges::grq_is_ready() {
        panic!("GRQ accessed before initialization");
    }
    crate::sched::edges::grq_ref()
}

/// b50 #GP guard (x86-scribble family): 48-bit canonical-address check
/// for pointers pulled out of runqueue structures. Both supported
/// arches (x86_64, riscv64 sv48) use 48-bit canonical addresses — bits
/// 63:47 must all replicate bit 47 — and every kernel pointer the
/// scheduler stores (heap Task structs, embedded ListHeads) is qword
/// aligned. A wild kernel store landing on a timeline/list slot turns
/// the stored pointer into garbage whose first deref raises #GP(0)
/// (boot b50: rip inside ownership_pickable's opening qword load,
/// rdi non-canonical, kernel fatal at ~20 min uptime). Testing the
/// shape before the deref converts that into a dropped/skipped entry.
pub fn kernel_ptr_canonical(p: *const u8) -> bool {
    let a = p as usize as u64;
    if a % 8 != 0 {
        return false;
    }
    let hi = a >> 47;
    hi == 0 || hi == 0x1FFFF
}

/// Kernel text window [lo, hi) taken from the arch linker script (x86_64:
/// __stext/__etext; riscv64: _stext/_etext). Forensic stack-word filters
/// must use this instead of hardcoded VAs: the old window
/// [0xffffffff80000000, 0xffffffff80200000) sat below the actual x86_64
/// text and covered only the first 2 MiB of the (up to 12 MiB) riscv64
/// image, so no real return address ever matched.
pub fn kernel_text_window() -> (u64, u64) {
    #[cfg(feature = "x86_64")]
    extern "C" {
        static __stext: u8;
        static __etext: u8;
    }
    #[cfg(not(feature = "x86_64"))]
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    #[cfg(feature = "x86_64")]
    let lo = core::ptr::addr_of!(__stext) as u64;
    #[cfg(feature = "x86_64")]
    let hi = core::ptr::addr_of!(__etext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let lo = core::ptr::addr_of!(_stext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let hi = core::ptr::addr_of!(_etext) as u64;
    (lo, hi)
}

/// Stricter variant for runqueue-walk safety: the pointer must lie in the
/// kernel PHYSMAP (direct map), where every Task allocation lives. Plain
/// canonical also accepts kernel-NEGATIVE addresses (0xfffffffffffffeXX —
/// poison like -1/-0x158 left in a freed task's list links), which then
/// fault inside the pick path when fields are dereferenced through them
/// (b61/b62 KERNPANICs in RtRunQueue::pick_next_cpu: the cgroup chain
/// walk and ownership_pickable faulting on garbage derived from
/// freed-then-reused queue entries).
pub fn kernel_ptr_in_physmap(p: *const u8) -> bool {
    let a = p as usize as u64;
    if a % 8 != 0 {
        return false;
    }
    #[cfg(feature = "x86_64")]
    {
        // 0xffff8880_00000000 ..< 0xffff9000_0000_0000 (64 GB of direct map).
        (0xffff_8880_0000_0000..0xffff_9000_0000_0000).contains(&a)
    }
    // RUX-RISCV-POISON (sched-riscv): the physmap screen was hardcoded to
    // the x86_64 direct-map window. On riscv64 (Sv39) every kernel heap
    // pointer lives in the linear map at 0xffffffd6_00000000..+128GB
    // (arch/riscv64/mm/memory_layout.rs: PAGE_OFFSET/KERN_VIRT_SIZE), so
    // the screen rejected EVERY legitimately allocated Task and CgroupNode:
    // the CFS pick dropped each one at its first pick ("POISONED rq entry
    // raw=0xffffffd6xxxxxxxx", rate-limited print hiding a systematic
    // drop), and any fork()ed child was silently lost at its first
    // schedule — the riscv64 wedge-at-first-ls repro. Only the static
    // storages (init task, per-CPU idles) survived via the extra legs of
    // kernel_task_ptr_range_ok, which is why the boot reached the shell
    // (PID 1 is the static init task) and then froze forever on the first
    // fork. Gate on the arch's actual linear-map window.
    #[cfg(not(feature = "x86_64"))]
    {
        let po = crate::arch::mm::PAGE_OFFSET as u64;
        let end = po + crate::arch::mm::KERN_VIRT_SIZE as u64;
        (po..end).contains(&a)
    }
}

/// Range-only Task-pointer screen for runqueue walks: the kernel linear
/// map (where every heap-allocated Task lives) PLUS the static Task
/// storages that legally sit outside it — the boot init task
/// (init::INIT_TASK_STORAGE, the ONE non-heap Task — b69: a physmap-only
/// screen dropped PID 1 from the CFS timeline at its first pick and
/// wedged both CPUs in WFI with nr_running==0) and the per-CPU idle
/// tasks (IDLE_TASK_STORAGES). Pure address arithmetic, no dereference —
/// safe to call on not-yet-validated memory. CgroupNode walks must NOT
/// use this (nodes are heap-only; keep kernel_ptr_in_physmap there).
pub fn kernel_task_ptr_range_ok(p: *const u8) -> bool {
    if kernel_ptr_in_physmap(p) {
        return true;
    }
    let a = p as usize;
    if a % 8 != 0 {
        return false;
    }
    let init_lo = crate::init::init_task_storage_addr();
    if a >= init_lo && a < init_lo + core::mem::size_of::<crate::process::task::Task>() {
        return true;
    }
    let idle_lo = crate::sched::edges::idle_task_storage_base();
    let idle_span = core::mem::size_of::<crate::process::task::Task>() * MAX_CPUS;
    a >= idle_lo && a < idle_lo + idle_span
}


/// Scribble hunter (rux:scribble (retired debug=scribble)): trylock the GRQ for the diagnostic
/// consistency scan. None when uninitialized or currently held.
pub fn try_grq_lock() -> Option<GrqGuard<'static>> {
    if !crate::sched::edges::grq_is_ready() {
        return None;
    }
    // SAFETY: guarded by GRQ_READY above.
    crate::sched::edges::grq_ref().try_lock_irqsave()
}

/// Per-CPU state array (indexed by cpu_id)
// PER_CPU moved into crate::sched::edges (INV-SCE-10): the per-CPU
// scheduler slots, indexed by the hardware cpu id, written by this
// cpu (init) and read cross-cpu under the GRQ / RCU disciplines the
// accessors document.

/// Per-CPU RQ initialization flags
static RQ_INITIALIZED: [core::sync::atomic::AtomicBool; MAX_CPUS] = [const { core::sync::atomic::AtomicBool::new(false) }; MAX_CPUS];

// R63 (byte-scribble root cause — the R49 heal's live-window blind spot):
// between `__schedule`'s current-slot update (this_cpu_mut().current =
// next, under the GRQ lock) and the register-save publish inside
// context_switch (x86: __switch_to's callee-saved save, then
// x86_switch_publish's prev->on_cpu = false; riscv64: the full
// ra/sp/s0-s11 save, then the post-fence on_cpu clear), a task being
// switched OUT reads as RUNNING ∧ unlinked ∧ owned-by-no-current-slot —
// the exact shape the R46/R49 phantom heal treats as an orphaned mark
// and clears. Clearing ti_on_cpu in that window lets any CPU's pick
// pass the on_cpu skip and resume a HALF-SAVED context (stale
// thread.sp / callee.ret_addr, torn fxsave) while the outgoing CPU is
// still writing the same task's thread struct and kernel stack — two
// CPUs executing one task on one stack. That is the engine behind the
// x86 fork/exec-storm scribble family (torn ret_addr bytes, gs_base <-
// 1033 from a torn pt_regs syscall-arg fetch, 0xf000ff53 IVT-pattern
// fills through a wild per-CPU base, R55 slot divergence, wedged
// spinlocks; caught live as DOUBLE-RUN pid=320 cpu2-vs-cpu3 with the
// R49-ONCPU-ORPHAN print in the same capture). This marker names the
// task each CPU is actively switching out, from before the GRQ release
// until after context_switch returns, so the heal can tell a LIVE mark
// from an orphaned one. Written only from __schedule on the local CPU
// (set under the GRQ lock, cleared on the same hardware CPU after the
// coroutine switch), so the waker's under-lock read is race-free.
pub static SWITCHING_OUT: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];


/// glib-mutex lane (2026-11-14): jiffies at the moment SWITCHING_OUT[c]
/// was LAST SET (stored together with the marker, under the GRQ). The
/// WAKE-HEAL's stale-marker adjudication uses MARKER AGE from this
/// stamp — the previous sighting-to-sighting test could not tell a
/// wedged leftover (marker set long ago, its switch never completing)
/// from a FRESH marker on a fast-cycling task whose previous mid-switch
/// sighting was naturally >50 jiffies earlier, and so ignored fresh
/// markers exactly for the futex/yield worker class — the garepro26
/// capture (WAKE-HEAL zeroing pid=328's in-flight GRQ guard count, then
/// the PREEMPT-UNDERFLOW of the same guard 5 ms later). A marker whose
/// age exceeds the threshold is a wedged leftover and may be ignored;
/// a fresh one is honored unconditionally.
pub(crate) static SWITCHING_OUT_JIF: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

/// Age (jiffies) beyond which a SWITCHING_OUT marker is adjudicated a
/// wedged leftover of an aborted/lost switch (the original WM constant;
/// a legitimate switch completes in microseconds, so 50 jiffies is
/// generous by three orders of magnitude).
pub(crate) const SWITCHING_OUT_STALE_JIF: u64 = 50;

/// R63: whether `task` is the prev of a context switch in flight on any
/// CPU. Called with the GRQ lock held (same discipline as the R49
/// current-slot scan).
///
/// r-strand (lat_fs conviction, 2026-11-19): only a YOUNG mark is an
/// in-flight switch. The mark's window is the __schedule frame —
/// microseconds — while the birth stamp (SWITCHING_OUT_JIF, set under
/// the same GRQ hold as the mark) bounds it; the 50-jiffy stale bar is
/// five-plus orders of margin. A mark aging past the bar means its
/// retire was lost (the captured face: a worker switched out mid-creat
/// with the retire skipped — RUNNING+unlinked+stale-marked, reached by
/// no pick and no heal, machine idle around it). Aging marks read as
/// NOT in flight so the phantom sweep relinks the task and the pick's
/// stale-mark heal can fire instead of every scanner deferring to a
/// switch that finished ages ago.
pub fn task_is_mid_switch_out(task: *mut Task) -> bool {
    let now = crate::timer::get_jiffies();
    (0..MAX_CPUS).any(|c| {
        if SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire) != task as u64 {
            return false;
        }
        let born = SWITCHING_OUT_JIF[c].load(core::sync::atomic::Ordering::Acquire);
        born != 0
            && now
                .wrapping_sub(born)
                < SWITCHING_OUT_STALE_JIF
    })
}

/// R85 (tail-phase PICK-SKIP wedge): does any CPU still OWN `task`?
///
/// Called from the class pick screens (fair.rs decision legs) with the GRQ
/// held, where it is the AUTHORITATIVE ownership adjudication. A task is
/// live-owned iff some CPU's current slot names it, or some CPU is
/// switching it out right now:
///
/// - A running task's slot names it from its pick until its next
///   __schedule, and a completed pick publishes the ownership marks AND
///   the slot inside one continuous GRQ hold — a marked pick can never be
///   observed slotless under the lock.
/// - A mid-switch-out task is named by SWITCHING_OUT[cpu], published under
///   the GRQ BEFORE the lock is dropped and cleared only AFTER
///   __switch_to's release store has landed (same CPU, program order).
///
/// A task that still shows ownership marks ({on_cpu=1, claim=X} or a
/// foreign claim) but is named by NO slot and NO in-flight switch is not
/// executing anywhere, no picker is resuming it, and no code path will
/// ever release its marks — the tail-phase PICK-SKIP wedge shape (pid 1
/// linked, {oncpu=1, claim=1, pk=1}, every CPU's pick skipping it with
/// why=1 forever while the box idles). The advisory pick_state word alone
/// cannot make this call: LIVE legitimately persists after switch-outs
/// and a scribble can leave it set forever (the R80 lesson) — the slots
/// and SWITCHING_OUT are scheduler-truth, written only under the GRQ.
pub fn pick_owner_live(task: *mut Task) -> bool {
    if task.is_null() {
        return false;
    }
    // r-strand (lat_fs conviction, 2026-11-19): an idle-marked cpu runs
    // only its idle task — its slot cannot legitimately name a real
    // task. The idle-loop entry heal rewrites the slot BEFORE
    // mark_idle, so a name surviving on an idle-marked cpu is the
    // STALE-slot face: the strand capture (PICK-SKIP-ALL why=1
    // oncpu=1 claim=1 slot=2 swout=0) held a linked task refused by
    // every pick for the whole window while the naming cpu sat
    // halted-idle. Discount idle-marked names so the why=1
    // stale-mark heal (stale_mark_confirm + release_pick_ownership)
    // can fire; SWITCHING_OUT stays authoritative — a cpu switching
    // TO idle keeps its mark until the release store lands.
    let idle_mask = grq().idle_cpus.load(core::sync::atomic::Ordering::Acquire);
    let now = crate::timer::get_jiffies();
    (0..MAX_CPUS).any(|c| {
        // Same aging discipline as task_is_mid_switch_out: only a YOUNG
        // switch-out mark names a live owner; an aged mark is the
        // lost-retire face and must let the stale-mark heal fire.
        let sw = {
            let m = SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire)
                == task as u64;
            let born = SWITCHING_OUT_JIF[c].load(core::sync::atomic::Ordering::Acquire);
            m && born != 0
                && now.wrapping_sub(born) < SWITCHING_OUT_STALE_JIF
        };
        if sw {
            return true;
        }
        if c < 32 && idle_mask & (1u32 << c) != 0 {
            return false;
        }
        cpu_state(c).current == task
    })
}

/// R85 (diag): bitmask of CPUs whose current slot names `task` — for the
/// PICK-SKIP-ALL report block, so a capture names whether a refusal was
/// backed by a live owner or a stale mark nobody will release.
pub fn owner_slot_mask(task: *mut Task) -> u32 {
    if task.is_null() {
        return 0;
    }
    let mut m = 0u32;
    for c in 0..MAX_CPUS {
        if cpu_state(c).current == task {
            m |= 1 << c;
        }
    }
    m
}

/// R85 (diag): bitmask of CPUs whose in-flight switch-out is `task`.
pub fn switching_out_mask(task: *mut Task) -> u32 {
    if task.is_null() {
        return 0;
    }
    let mut m = 0u32;
    for c in 0..MAX_CPUS {
        if SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire) == task as u64 {
            m |= 1 << c;
        }
    }
    m
}

/// R86 (exit-side lost-release adjudication): the exit-path twin of the
/// R85 pick-side heal. free_dead_member's `while member.on_cpu()` wait
/// assumes the member's final __switch_to release store will land —
/// true for every legitimate in-flight switch-out (the member is named
/// by its CPU's slot until the pick completes, and by SWITCHING_OUT
/// until the release store has landed, both published under the GRQ).
/// But a DEAD, unlinked member showing {on_cpu=1, claim=X} while named
/// by NO slot and NO in-flight switch is the R85 lost-release terminal
/// microstate striking the EXIT path: no picker ever scans it again
/// (it is not linked), so the pick-side heal cannot reach it, and the
/// reaper spins here forever — one CPU lost per wedged sweep, and the
/// GNOME no-light class when it lands during shell init (r16type/r20type:
/// both SMP2 CPUs spinning in this loop behind lost releases, shell
/// wakeups WAKE-STRANDED, screen dark for the whole 2700 s run while
/// the kernel stayed panic-free).
///
/// Adjudicate under the GRQ (the same discipline as the pick-side heal:
/// the slot flip and SWITCHING_OUT publish share one GRQ hold, so a
/// GRQ-held reader can never observe the publish half-done and free a
/// task whose switch-out is still writing its saved context). When no
/// owner is found, release the marks with the exact store the switch
/// protocol would have made — idempotent if the real release ever
/// lands — and report loudly (first 8 per boot).
///
/// R87 (oh-launch round 10, the scribbler producer): the adjudication is
/// EXIT-STRICT — stricter than pick_owner_live in exactly one dimension:
/// a SWITCHING_OUT marker naming the member counts as a LIVE in-flight
/// switch REGARDLESS OF AGE. The mark's window is not just the register
/// swap: between the mark's publish (under the GRQ, before the slot
/// flip) and the asm release store, the switching CPU still executes the
/// whole publish chain — x86_switch_publish, the rsp0 install, every
/// armed diagnostic — ON THE MEMBER'S OWN KERNEL STACK, with IRQs off.
/// Under a monopolist window, a KVM host deschedule, or a long armed-boot
/// ring dump, that window can exceed any fixed age bar; the 50-jiffy
/// discount then read a still-live switch as stale, this heal
/// force-cleared the ownership qword, the FDM wait exited, and
/// free_kernel_stack freed pages the switching CPU was still executing
/// on — the producer behind the string-byte execution hijacks
/// (rip='s.boot.s', rsp='os.requi'), the 0xb9 page-table face, and the
/// R83 ti_kernel_sp family. The pick side keeps its age discount (a
/// wrong call there relinks a task); the exit side may not (a wrong call
/// here frees a live stack). An aged mark is AMBIGUOUS and ambiguity
/// defers: free_dead_member re-parks the member for the next sweep
/// instead of proceeding.
pub fn member_release_stale_ownership(member: *mut Task) -> bool {
    if member.is_null() {
        return false;
    }
    let guard = grq().lock_irqsave();
    // Exit-strict liveness (R87): any SWITCHING_OUT marker (ANY age) or
    // any non-idle-discounted current slot names a live owner. The idle
    // discount keeps pick_owner_live's sound half: an idle-marked cpu
    // runs only its idle task (its idle bit is cleared under the same
    // GRQ hold as every real-task pick), so a member's name surviving in
    // an idle-marked slot is the lost-slot-flip face — and the idle task
    // never touches the member's kernel stack.
    let idle_mask = grq().idle_cpus.load(core::sync::atomic::Ordering::Acquire);
    let mut live = false;
    for c in 0..MAX_CPUS {
        if SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire) == member as u64 {
            live = true; // in-flight switch-out, any age (R87)
            break;
        }
        if c < 32 && idle_mask & (1u32 << c) != 0 {
            continue;
        }
        if cpu_state(c).current == member {
            live = true;
            break;
        }
    }
    if live {
        return false; // guard drops here — owned or ambiguous, keep waiting
    }
    // SAFETY: the member is DEAD and unlinked (the caller's dead-list
    // invariant); no picker can touch it, and the GRQ hold above excludes
    // every in-flight switch-out, so the qword write races nothing.
    let (pid, claim, pk, st, tcpu, slot, swout, n) = {
        // INV-SCE-2: the caller's dead-list invariant pins the member.
        let member = crate::sched::edges::task_shared(member);
        let pid = member.pid();
        let claim = member.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
        let pk = member.ti_pick_state.load(core::sync::atomic::Ordering::Acquire);
        let st = member.state().bits();
        let tcpu = member.ti_cpu() as u32;
        let slot = owner_slot_mask(core::ptr::from_ref(member) as *mut _);
        let swout = switching_out_mask(core::ptr::from_ref(member) as *mut _);
        member.release_pick_ownership();
        static FDM_STALE_SEEN: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = FDM_STALE_SEEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        (pid, claim, pk, st, tcpu, slot, swout, n)
    };
    // PROBE-LOCK DISCIPLINE (grq-lock-1st): report AFTER the guard drop.
    // printk's ring-buffer write ends in `KMSG_READ_WQ.wake_up_all()`, whose
    // waitqueue lock is held across `wake_up_process` -> the GRQ acquire.
    // A printk issued under the GRQ therefore re-acquires the GRQ on this
    // cpu (self-deadlock with IRQs off) or forms the GRQ->WQ / WQ->GRQ
    // ABAA with another cpu. Every field the report needs was captured
    // above, so release-then-report cannot change the outcome.
    drop(guard);
    if n < 8 {
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
        crate::pr_warn!(
            "FDM-STALE-RELEASE pid={} claim={} pk={} st={:#x} tcpu={} slot={:#x} swout={:#x}",
            pid,
            claim,
            pk,
            st,
            tcpu,
            slot,
            swout
        );
    }
    true
}

/// R87 (stack-free tail window, oh-launch round 10): does any CPU's
/// SWITCHING_OUT marker name `task`, at ANY age? The marker brackets the
/// switch path's ENTIRE set of references to prev's kernel stack: it is
/// published under the GRQ BEFORE the per-CPU slot flips (so before any
/// GRQ-serialized observer can see the switch starting) and cleared only
/// in the incoming frame's post-switch tail — AFTER __switch_to's rsp
/// swap abandoned prev's stack for good (both the x86_64 and riscv64
/// tails clear it after context_switch returns).
///
/// on_cpu alone does NOT cover the switch tail. The asm release store
/// (x86_64 context.rs, the R65 order) lands BEFORE the register restore:
/// with {on_cpu=false, claim=-1} already published, the switching CPU
/// still executes its final loads, the resume push and the ret ON PREV'S
/// OWN STACK. Under KVM the host can deschedule the switcher for
/// milliseconds right after the release store while the freeing vCPU
/// races ahead — wait-exit, RCU poll, free, slab recycle — and the
/// switcher's ret then lands in reallocated memory (the recycled-frame
/// scribbler's tail face; fr1: the scribble reporter's own ret popping a
/// foreign return address 1.29 s into the boot, no heal and no deadline
/// involved). Every kernel-stack free must therefore treat a live marker
/// as "do not free", the exact discipline free_task_slot (FTS) already
/// applies to the Task page.
pub fn switching_out_names(task: *mut Task) -> bool {
    if task.is_null() {
        return false;
    }
    for c in 0..MAX_CPUS {
        if SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire) == task as u64 {
            return true;
        }
    }
    false
}

/// R85: pick-side time confirmation for a stale-ownership adjudication.
/// Wraps the harvester's orphan watch (same table, same exec-frozen
/// refresh discipline — R81): a mid-switch-out transient or a briefly
/// descheduled picker advances its exec clock and re-records, never
/// maturing; a shape whose exec clock stays frozen for `threshold`
/// jiffies is by definition stale. The owner-based pick_owner_live
/// check already excludes every legitimate owner, so this is defense in
/// depth against an out-of-tree divergence window.
pub fn stale_mark_confirm(task: *mut Task, pid: u32, threshold: u64) -> bool {
    orphan_watch_confirm(task, pid, crate::timer::get_jiffies(), threshold)
}

/// Per-CPU reschedule flags
pub static NEED_RESCHED: [core::sync::atomic::AtomicBool; MAX_CPUS] = [const { core::sync::atomic::AtomicBool::new(false) }; MAX_CPUS];

/// Per-CPU polling-idle flags (closegap lane): set while the idle loop
/// is inside its pre-halt poll window. `resched_cpu` reads this and
/// skips the resched IPI when set — the polling CPU observes
/// NEED_RESCHED/nr_running on its own within tens of ns. The idle loop
/// clears the flag BEFORE its final recheck, so a dispatch racing the
/// clear either takes the no-IPI path with the recheck catching the
/// work, or takes the IPI path against a halting CPU (see cpu_idle_loop).
pub(crate) static POLLING_IDLE: [core::sync::atomic::AtomicBool; MAX_CPUS] = [const { core::sync::atomic::AtomicBool::new(false) }; MAX_CPUS];


#[inline]
fn sched_cs_stamp(cpu: usize, phase: u32) {
    if cpu < MAX_CPUS {
        super::entities::SCHED_CS_PHASE[cpu].store(phase, core::sync::atomic::Ordering::Release);
    }
}


/// r24 (kill10 wedge): idle-loop own-slot heal — the executing idle task
/// found its own slot naming a real task (the lying-busy-slot shape that
/// parks wake victims forever on x86_64, where the pid-hash harvester is
/// gated off).
pub const SLOT_W_IDLE_HEAL: u32 = 6;
pub static SLOT_WRITER: [core::sync::atomic::AtomicU32; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_CPUS];
pub static SLOT_WRITER_TASK: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

#[inline]
pub(crate) fn slot_tag_write(cpu: usize, tag: u32, task: *mut Task) {
    if cpu < MAX_CPUS {
        SLOT_WRITER[cpu].store(tag, core::sync::atomic::Ordering::Relaxed);
        SLOT_WRITER_TASK[cpu].store(task as u64, core::sync::atomic::Ordering::Relaxed);
    }
}

// r11 (R49 pick-published-never-switched cascade — the lying-slot producer
// hunt): per-CPU pick-generation ledger. Every publish of a task into a
// PER_CPU current slot (the tag-1 pick publish is the principal one) bumps
// that CPU's generation and records {task, pid, jiffies}; the
// materialization of the generation — the picked context actually resuming
// on that CPU (the post-context_switch point, the repick fast path, the
// newborn's schedule_tail, or the abort republish whose prev keeps
// running) — retires the CURRENT generation. A generation left unretired
// while its slot still names its task and its CPU keeps advancing is the
// losing switch: the pick published the slot but no context switch ever
// materialized into the picked task — the exact shape the round-10 strand
// fingerprinted (LP-WAKE-SKIP-EMPTY pid=<udp-server> naming_cpu=0 slotw=1
// idlebit=0, naming CPU busy elsewhere). All Relaxed, same-CPU ordered by
// construction (publish holds the GRQ; retire runs on the publishing CPU
// with IRQs off); readers are diagnostic prints that pin coherence by
// requiring PICK_GEN_TASK == the slot's task.
pub static PICK_GEN: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];
pub static PICK_GEN_DONE: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];
pub static PICK_GEN_TASK: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];
pub static PICK_GEN_PID: [core::sync::atomic::AtomicU32; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_CPUS];
pub static PICK_GEN_JIF: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];
/// Census: generations convicted unretired at a lying-slot observation
/// (slot still names the published task, tag-1 writer, age past the
/// in-flight floor). Zero on healthy boots.
pub static PICKGEN_LOST: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];


// ==================== Deferred Exit Notification ====================
//
// When a task exits (do_exit), it must notify its parent (SIGCHLD +
// wake_up).  However, waking the parent BEFORE schedule() creates a
// race: the parent can reap (free_task_slot) the exiting task before
// schedule() switches it away, causing use-after-free corruption.
//
// Solution: store the parent PID in a per-CPU slot; __schedule
// processes it AFTER the context switch, when the exiting task is
// guaranteed to no longer run on any CPU.

/// Per-CPU deferred exit-notification slot. Packs (parent_pid << 32 |
/// child_pid) into one atomic so the parent/child PAIR is never torn
/// (0 = no pending notification).
static DEFERRED_EXIT_NOTIFY: [core::sync::atomic::AtomicI64; MAX_CPUS] = [const { const { core::sync::atomic::AtomicI64::new(0) } }; MAX_CPUS];

#[inline]
fn pack_notify(parent_pid: u32, child_pid: u32) -> i64 {
    ((parent_pid as i64) << 32) | (child_pid as i64 & 0xFFFF_FFFF)
}

#[inline]
fn unpack_notify(v: i64) -> (u32, u32) {
    (((v >> 32) as u32) & 0xFFFF_FFFF, (v & 0xFFFF_FFFF) as u32)
}

/// Defer sending SIGCHLD to `parent_pid` until after the next context switch.
///
/// Called from `do_exit` *before* `schedule()`.  The notification is
/// delivered by `__schedule` once the exiting task has been switched away.
/// `child_pid` rides along so the SIGCHLD siginfo can carry the child's
/// exit status (SA_SIGINFO handlers — toybox timeout, glib, systemd —
/// decode the outcome from si_code/si_status).
pub fn defer_exit_notify(parent_pid: u32, child_pid: u32) {
    let cpu = arch::cpu_id() as usize;
    if cpu < MAX_CPUS {
        // R11-4: single-slot overwrite lost notifications when two tasks
        // exited on the same CPU before a context switch (echo + cat in a
        // pipeline) — the parent's SIGCHLD vanished and mrsh's wait hung.
        // Fire any pending predecessor INLINE (we are in syscall context,
        // post-preempt-enable; send_signal + wake are safe here) before
        // taking the slot.
        let new = pack_notify(parent_pid, child_pid);
        let old = DEFERRED_EXIT_NOTIFY[cpu].swap(new, core::sync::atomic::Ordering::Relaxed);
        if old != 0 && old != new {
            process_deferred_exit_pid(old);
        }
    }
}

/// 99th (§87.8 wait4/SIGCHLD wakeup-loss hunt): read-only snapshot of
/// the per-CPU deferred exit-notification slots for the wait4-park
/// census (a nonempty slot naming a parked waiter as the parent = a
/// child's exit notify was stored but never delivered — the deferred
/// protocol's loss arm, see exit.rs wait4_park_audit). Fills at most
/// `out.len()` entries; 0 = empty slot.
pub fn deferred_exit_notify_slots(out: &mut [i64]) {
    let n = out.len().min(MAX_CPUS);
    for c in 0..n {
        out[c] = DEFERRED_EXIT_NOTIFY[c].load(core::sync::atomic::Ordering::Relaxed);
    }
}

/// Build the Linux do_notify_parent siginfo from the (zombie) child task:
/// si_code = CLD_EXITED/CLD_KILLED/CLD_DUMPED and si_status in waitpid
/// WSTATUS encoding. Falls back to None when the child is already gone
/// from the pid hash (reaped by a racing WNOHANG loop). Returns the
/// siginfo AND the child's clone exit_signal (do_notify_parent sends
/// THAT signal, not always SIGCHLD — LTP clone301 exit_signal=SIGUSR2).
fn build_sigchld_info(child_pid: u32) -> Option<(crate::signal::SigInfo, u8)> {
    let child = crate::process::pid_hash::pid_hash_lookup_pinned(child_pid);
    if child.is_null() {
        return None;
    }
    // Child is pinned (refcount held); INV-SCE-2 reads.
    let (info, exit_sig) = {
        let child = crate::sched::edges::task_shared(child);
        let i = crate::signal::SigInfo::child_exit(
            child.pid(),
            child.cred().uid,
            child.exit_code(),
            child.core_dumped(),
        );
        (i, child.get_exit_signal())
    };
    crate::process::task::Task::task_put(child);
    Some((info, exit_sig))
}

/// Deliver one deferred exit notification: the child's clone exit_signal
/// (SIGCHLD unless the clone asked otherwise) with the exit siginfo to the
/// parent, plus the group-wide wait_chldexit wake.
fn process_deferred_exit_pid(packed: i64) {
    use crate::signal::Signal;
    // forkprof (cmdline forkprof=1): the notify+wake cost that runs in
    // the switcher's context after the exiting task is off-CPU.
    let fp = crate::debug::forkprof::on();
    let fp_t0 = if fp { crate::debug::forkprof::now() } else { 0 };
    let (parent_pid, child_pid) = unpack_notify(packed);
    // EXITNOTIFY (dbus-activation hunt): activation stalls were traced to
    // the parent never learning its spawned child exited. Log each
    // delivered exit notification (parent, child, signal rc).
    // serial-clean: default off — available only behind rux:schedtrace (retired debug=schedtrace)
    // (it fired on every child exit and polluted every boot log).
    if crate::trace::EVENT_SCHEDTRACE.armed() {
        static EN_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if EN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 150 {
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
            crate::pr_info!(
                "EXITNOTIFY: child={} parent={}",
                child_pid, parent_pid
            );
            // debug-migrate: the row also records into the unified
            // framework (rux:schedtrace kind=EXITNOTIFY — the event is
            // also the family's arm switch).
            crate::trace::trace_schedtrace(
                0,
                child_pid as u32,
                parent_pid as u64,
                0,
            );
        }
    }
    match build_sigchld_info(child_pid) {
        Some((mut info, exit_sig)) => {
            // Rewrite si_signo: the siginfo builder hardcodes SIGCHLD,
            // do_notify_parent delivers p->exit_signal (0 = no signal).
            if exit_sig != 0 && exit_sig != Signal::SIGCHLD as u8 {
                info.si_signo = exit_sig as i32;
            }
            if exit_sig != 0 {
                let _ = crate::signal::send_signal_with_info(parent_pid, info);
            }
        }
        None => {
            // Child already reaped — the bare wake still unblocks a
            // racing wait4 (it re-checks its children).
            let _ = crate::signal::send_signal(parent_pid, Signal::SIGCHLD as i32);
        }
    }
    let parent = crate::process::pid_hash::pid_hash_lookup_pinned(parent_pid);
    if !parent.is_null() {
        // Group-wide wake: Linux's wait_chldexit queue is shared by the
        // whole thread group — a sibling blocked in wait4/waitid (e.g.
        // glib's gmain worker) must also observe the child exit.
        crate::process::exit::wake_group_chldexit(crate::process::unsafe_edges::borrow_task(parent));
        crate::process::task::Task::task_put(parent);
    }
    if fp {
        crate::debug::forkprof::add(crate::debug::forkprof::P_SCHED_NOTIFY, fp_t0);
        crate::debug::forkprof::trace(crate::debug::forkprof::T_SNDNOTIFY_DONE, child_pid);
    }
}

/// Process the deferred exit notification (if any) for the current CPU.
///
/// Must be called AFTER `context_switch` so the exiting task is no longer
/// running on any CPU when we wake the parent.
/// Process deferred exit notification for a specific CPU.
/// Called with the CPU that the exiting task was running on (captured
/// before context_switch), because cpu_id() returns the new task's
/// CPU after the switch.
// (delivery helper lives above as process_deferred_exit_pid(packed))

fn process_deferred_exit_notify_cpu(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let packed = DEFERRED_EXIT_NOTIFY[cpu].load(core::sync::atomic::Ordering::Relaxed);
    if packed == 0 {
        return;
    }
    // Clear the slot (consume the notification).
    DEFERRED_EXIT_NOTIFY[cpu].store(0, core::sync::atomic::Ordering::Relaxed);

    let (pid, _child) = unpack_notify(packed);
    let parent = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
    if !parent.is_null() {
        // SAFETY: parent was obtained from pid_hash_lookup and is a valid Task
        // pointer (PID hash table entries are not freed until release_task).
        {
            // Migrate a SLEEPING parent to the current CPU before waking it:
            // wake_up → select_task_rq reads ti_cpu, so steering it here
            // lets this CPU's idle task pick it up immediately.
            // NEVER touch ti_cpu of a task that is still running or already
            // queued on another CPU — cpu_id() reads tp→ti_cpu on that CPU,
            // and hijacking it corrupts the victim's per-CPU view
            // (review 2R.15).
            // R13-1 (root cause of S-A AND S-B): is_sleeping() is TRUE
            // during the whole prepare_to_wait -> schedule() window while
            // the parent is STILL EXECUTING on another CPU. Steering its
            // ti_cpu then poisons cpu_id() (a tp→ti_cpu FIELD read) for
            // the rest of its kernel path: the next __schedule resolves
            // prev from the WRONG per-CPU slot and switches context
            // against a different task — two tasks, one kernel stack;
            // zeroed stack locals (the NULL+0x30 CAS) and mixed trap
            // frames (the jump-to-user-address) both fall out. on_cpu()
            // is exactly "picked, context not yet saved": true inside
            // that window (skip), false once genuinely switched out
            // (safe to steer).
            // R49: the on_cpu gate must be evaluated ATOMOMICALLY with the
            // pick path — the old unlocked check-then-write raced
            // mark_picked_on_cpu + context_switch's ti_cpu stamp (see
            // steer_task_cpu). The is_sleeping() filter stays unlocked
            // (cheap pre-filter); the on_cpu re-check moved under the
            // GRQ lock inside steer_task_cpu.
            if crate::sched::edges::task_shared(parent).state().is_sleeping() {
                steer_task_cpu(parent, cpu as i32);
            }
        }
        crate::process::task::Task::task_put(parent);
    }

    process_deferred_exit_pid(packed);
}

/// Per-CPU idle task storage
// IDLE_TASK_STORAGES moved into sched::edges (INV-SCE-4).

/// Per-CPU idle task pointers, accessed by boot.S to set tp on secondary CPUs.
/// Written once by sched::init() on the boot CPU, read by secondary_start in
/// assembly before any C code runs.  0 means not yet initialized.
#[no_mangle]
// __secondary_idle_tasks moved into crate::sched::edges (INV-SCE-4
// companion): published once by the BSP init loop, consumed read-only
// by boot.S on secondary bring-up.

// ==================== Reschedule Flags ====================


/// Voluntary preemption point (launcher perf gate round 3, 2026-10-20).
///
/// With no kernel preemption, a task inside a long kernel-side loop
/// (task-teardown sweeps, resource drops) monopolizes its CPU until the
/// loop ends even with need_resched set by every tick — the steady
/// WAKE-STRANDED family under the OH thread-churn load. Unlike
/// IRQ-exit preemption (tried in 2cf4bdc9, reverted in 0ced11da: it
/// poisoned preempt_counts by interrupting code that was never written
/// to be switched mid-flight), this yields ONLY at points the caller
/// KNOWS are switch-safe: task context, no locks, no bh, no RCU read
/// side (all enforced by the preemptible() check), no per-CPU state
/// straddling the call.
pub fn cond_resched() {
    if need_resched() && crate::interrupt::preempt::preemptible() {
        schedule();
    }
}

#[no_mangle]
pub extern "C" fn asm_need_resched() -> i64 {
    if need_resched() { 1 } else { 0 }
}


/// Claim an idle CPU (within `affinity`) for a wake dispatch (perf-ipc):
/// the wake path's public entry for idle steering (see
/// `Task::wake_up_impl`). The claim atomically clears the idle bit so two
/// racing wakes cannot dispatch to the same CPU.
pub fn claim_idle_cpu_for(affinity: u32) -> Option<usize> {
    // RT-R4 (R2 residual, pick side): prefer the LOCAL cpu when it is
    // idle and allowed, before the rotating spread. The spread 50/50'd
    // dispatches to the REMOTE idle cpu on an all-idle box — a resched
    // IPI into a HALTED vCPU, the rt_pick_avg 19-21 us class (vs 3 us
    // when no halt is crossed; smp2 interval sweep, run/RT-R4.md) —
    // while the LOCAL cpu, often executing this very wake in its timer
    // hardirq, would pick the task at its idle-loop schedule() with
    // zero IPI and zero halt-exit (resched_cpu(self) only sets the
    // NEED_RESCHED flag). Same shape as the reference's wake-affine
    // this_cpu preference in select_task_rq. clear_idle is an atomic
    // CAS, so racing wakes still cannot double-target a cpu.
    let this = crate::arch::cpu_id() as usize;
    if this < MAX_CPUS && affinity & (1u32 << this) != 0 && grq().clear_idle(this) {
        return Some(this);
    }
    grq().claim_idle_cpu(affinity)
}

/// R3 (rt-r3 lane): placement-preferring idle claim — see
/// `GlobalRunQueue::claim_idle_cpu_prefer`.
///
/// RT-R4 unification note (P0 hunt, 2026-11-18): rt-r3's rewiring moved
/// the wake dispatch here, and the CFS leg (prefer < 0) fell through to
/// the raw rotating claim — silently bypassing claim_idle_cpu_for's
/// LOCAL-idle preference (rt-r4), which had been the dispatch entry.
/// The two targeting decisions are ONE decision again: placement first
/// (RT/DL), then LOCAL-when-idle (all classes — the waker executing
/// this wake picks it at its own idle-loop schedule() with zero IPI),
/// then the rotating spread. One function, one order, no disagreeing
/// paths.
pub fn claim_idle_cpu_for_prefer(affinity: u32, prefer: i32) -> Option<usize> {
    // 1. Placement (RT/DL): the reference's check_preempt_curr target
    //    rq; a timer hardirq fires on the sleeper's own cpu, so claiming
    //    the placement is a zero-IPI self-poke (rt-r3).
    if prefer >= 0 && (prefer as usize) < MAX_CPUS && (affinity & (1u32 << prefer)) != 0 {
        if grq().clear_idle(prefer as usize) {
            return Some(prefer as usize);
        }
    }
    // 2. LOCAL-when-idle (all classes, rt-r4): the waker executing this
    //    very wake picks the task at its own idle-loop schedule() with
    //    zero IPI and zero halt-exit.
    let this = crate::arch::cpu_id() as usize;
    if this < MAX_CPUS && affinity & (1u32 << this) != 0 && grq().clear_idle(this) {
        return Some(this);
    }
    // 3. The rotating spread (idle spreading, review batch 8).
    grq().claim_idle_cpu(affinity)
}


/// Idle-task pointer for a CPU (pre-created by `init`; used by the SMP
/// bring-up to publish `current` on a secondary before its first tick).
pub fn idle_task_ptr(cpu: usize) -> usize {
    if cpu < MAX_CPUS {
        // SAFETY: written once by init() before secondaries start; read
        // only by the matching CPU's bring-up.
        crate::sched::edges::secondary_idle_task(cpu)
    } else {
        0
    }
}


// ==================== Per-CPU Accessors ====================

/// Get per-CPU state for the current CPU.
#[inline]
fn this_cpu() -> &'static PerCpuState {
    // SAFETY: cpu_id is bounds-checked against MAX_CPUS; each CPU only reads its own slot.
    {
        let id = crate::arch::cpu_id() as u64 as usize;
        if id >= MAX_CPUS {
            panic!("this_cpu: cpu_id {} >= MAX_CPUS {}", id, MAX_CPUS);
        }
        crate::sched::edges::per_cpu_shared(id)
    }
}

/// Get per-CPU state for the current CPU (mutable).
#[inline]
pub(crate) fn this_cpu_mut() -> &'static mut PerCpuState {
    // INV-SCE-10: this CPU mutates its own slot.
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    crate::sched::edges::per_cpu_mut(cpu_id)
}

/// Get per-CPU state for a specific CPU.
#[inline]
pub fn cpu_state(cpu_id: usize) -> &'static PerCpuState {
    // SAFETY: cpu_id is clamped to [0, MAX_CPUS-1]; PER_CPU is a static array.
    crate::sched::edges::per_cpu_shared(cpu_id)
}

/// Get per-CPU state for a specific CPU (mutable).
#[inline]
pub(crate) fn cpu_state_mut(cpu_id: usize) -> &'static mut PerCpuState {
    // SAFETY: cpu_id is clamped to [0, MAX_CPUS-1]; caller must ensure no aliasing.
    crate::sched::edges::per_cpu_mut(cpu_id)
}

/// Check if a CPU is online (has been assigned an idle task).
#[inline]
pub fn cpu_online(cpu: usize) -> bool {
    cpu < MAX_CPUS && !cpu_state(cpu).idle.is_null()
}

/// Whether CPU `cpu_id` is currently executing its idle task (including
/// while halted). Read-only remote query, used by the RCU grace-period
/// check: a CPU inside the idle loop executes no fd-table reader
/// windows (get_file runs only in task/IRQ context over a non-idle
/// `current`), so it is quiescent regardless of when it last managed to
/// store a QS report — a halted idle CPU's stale report must not hold
/// up a grace period (r26: close-completion waits stalled ~1 jiffy per
/// close otherwise). The pointer load is atomic and `current` only
/// transitions idle→task at switch-in (after the reporting site in
/// `__schedule`), so a `true` result always means genuinely-idle-loop.
#[inline]
pub fn cpu_running_idle(cpu_id: usize) -> bool {
    let s = cpu_state(cpu_id);
    !s.idle.is_null() && s.current == s.idle
}

// ==================== Dummy RunQueue (compatibility) ====================

/// Dummy RunQueue for compatibility with SchedClass trait / procfs output.
pub struct RunQueue;

// ==================== Initialization ====================

pub fn init_per_cpu_rq(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }
    if RQ_INITIALIZED[cpu_id].load(core::sync::atomic::Ordering::Acquire) {
        return;
    }

    // Initialize the global RQ once
    grq_init(); // idempotent (edges::grq_claim_init gates the work)

    RQ_INITIALIZED[cpu_id].store(true, core::sync::atomic::Ordering::Release);
}

pub fn init_secondary(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    // tp was already set to the idle task pointer by boot.S (loaded from
    // __secondary_idle_tasks).  Just register with the per-CPU state.
    {
        let idle_ptr = crate::sched::edges::idle_task_ptr(cpu_id);

        // Verify the current-task pointer matches the pre-created idle task
        let current_tp = crate::arch::cpu::get_thread_id() as usize;
        #[cfg(not(feature = "aarch64"))]
        debug_assert_eq!(current_tp, idle_ptr as usize,
            "init_secondary: current-task ptr mismatch! tp={:#x}, expected={:#x}", current_tp, idle_ptr as usize);
        // aarch64 secondary bring-up parks the cpu INDEX in TPIDR_EL1
        // (boot.S secondary_start has no __secondary_idle_tasks
        // protocol); the idle task pointer is installed HERE — the
        // moment the per-task protocol begins. Until this port's IRQ
        // bring-up, secondaries never got past their masked-wfi park,
        // so the riscv64-shaped assert above never fired here.
        #[cfg(feature = "aarch64")]
        if current_tp != idle_ptr as usize {
            crate::arch::cpu::set_thread_id(idle_ptr as u64);
        }

        clear_trap_scratch();

        let pcpu = cpu_state_mut(cpu_id);
        pcpu.idle = idle_ptr;
        pcpu.current = idle_ptr;
    }
}

/// Clear the trap-entry scratch register (riscv64 `sscratch`; no-op
/// elsewhere — x86_64 keeps per-CPU state in memory, not a scratch reg).
///
/// IRREDUCIBLE F-ROW (F-REG-3): CSR state has no safe Rust access —
/// the reference's `arch/riscv/kernel/entry.S` writes the same
/// `csrw sscratch, zero` on the usermode-entry paths (the scratch
/// register holds the kernel tp across user execution; zeroing here
/// establishes the boot-time convention before the first userspace
/// entry). Contract: a CSR write with no memory effect; the one-shot
/// boot ordering is guaranteed by the caller (init, pre-scheduler).
#[inline]
fn clear_trap_scratch() {
    #[cfg(feature = "riscv64")]
    // F-REG-3 (riscv64): csrw sscratch, zero — entry.S usermode-entry setup.
    unsafe {
        core::arch::asm!("csrw sscratch, zero");
    }
}

pub fn init() {
    let boot_cpu = crate::arch::cpu_id() as u64 as usize;

    // grq-lock-1st: arm the wedge injection when the diagnostic build is
    // asked for it (`grqwedge=1`). The injection itself is compile-time
    // gated; the arming is a one-shot cmdline read at init.
    #[cfg(feature = "debug-grq-wedge")]
    if crate::cmdline::get_param("grqwedge").as_deref() == Some("1") {
        grq_wedge_arm();
    }

    if boot_cpu >= MAX_CPUS {
        // HOTPATH-PRINT: boot-time one-shot (init error path, fires before the scheduler runs).
        println!("sched: init: invalid cpu_id {}", boot_cpu);
        return;
    }

    // Pre-create idle tasks for ALL CPUs (Linux model).
    // This must happen before start_secondaries() so that boot.S can
    // load tp from __secondary_idle_tasks before any C code runs on
    // secondary harts, allowing safe early interrupt enable.
    for cpu in 0..MAX_CPUS {
        init_per_cpu_rq(cpu);

        // SAFETY: INV-SCE-4 writer side — the BSP init loop writes each
        // cpu's idle slot exactly once, before any secondary schedules;
        // __secondary_idle_tasks is consumed read-only by boot.S after
        // this loop publishes it.
        unsafe {
            let idle_ptr = crate::sched::edges::idle_task_ptr(cpu);
            Task::new_idle_at(idle_ptr);

            crate::process::pid_hash::pid_hash_insert(idle_ptr);

            if let Some(stack_top) = (*idle_ptr).alloc_kernel_stack() {
                (*idle_ptr).thread_mut().sp = stack_top as u64;
            } else {
                // HOTPATH-PRINT: boot-time one-shot (init allocation failure).
            println!("sched: failed to allocate kernel stack for idle task cpu {}", cpu);
            }

            (*idle_ptr).set_ti_cpu(cpu as i32);

            // Store pointer for assembly secondary_start
            crate::sched::edges::secondary_idle_publish(cpu, idle_ptr as usize);

            if cpu == boot_cpu {
                clear_trap_scratch();
                crate::arch::cpu::set_thread_id(idle_ptr as u64);

                let pcpu = cpu_state_mut(cpu);
                pcpu.idle = idle_ptr;
                pcpu.current = idle_ptr;
            }
        }
    }
}

// ==================== Task Allocation ====================

/// R17-B: live-Task-page ownership bitmap — any heap free covering a page
/// that carries a live Task (marked at alloc, unmarked at free) is refused
/// and reported. Proved the free side clean in round 17; kept as the
/// permanent tripwire for the alloc-side handoff hunt.
///
/// Batch 8: FULL coverage of physical RAM. The old 128-word table indexed
/// with `(page >> 6) & 127` aliased every 8192 pages — a page 512MB away
/// could read another page's ownership bit (false tripwire hits / silent
/// misses). One bit per 4K page of PHYS_MEMORY_SIZE, keyed by PHYSICAL page
/// number; Task allocations come from the buddy heap, which lives in the
/// high linear map (HEAP_START = 0x80A0_0000 + VA_PA_OFFSET), so both the
/// low identity window and the linear-map window are translated to phys.
const TASK_PAGE_PAGES: usize = crate::config::PHYS_MEMORY_SIZE / crate::config::PAGE_SIZE;
const TASK_PAGE_WORDS: usize = (TASK_PAGE_PAGES + 63) / 64;
static TASK_PAGE_OWNED: [core::sync::atomic::AtomicU64; TASK_PAGE_WORDS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; TASK_PAGE_WORDS];

/// Physical page index (0 = RAM base 0x8000_0000) for the page carrying
/// `ptr`, or None when the pointer is outside mapped RAM (nothing to track).
#[inline]
fn task_page_index(ptr: *const u8) -> Option<usize> {
    const RAM_BASE: usize = 0x8000_0000;
    let p = ptr as usize;
    let page_offset = crate::arch::mm::memory_layout::PAGE_OFFSET;
    if p >= page_offset {
        // Linear map: virt = PAGE_OFFSET + (phys - RAM_BASE)
        let off = p - page_offset;
        if off < crate::config::PHYS_MEMORY_SIZE {
            return Some(off >> 12);
        }
    } else if p >= RAM_BASE && p - RAM_BASE < crate::config::PHYS_MEMORY_SIZE {
        // Low identity window (kernel image, early-boot allocations)
        return Some((p - RAM_BASE) >> 12);
    }
    None
}

#[inline]
fn task_page_bit(ptr: *const u8) -> Option<(usize, u64)> {
    let page = task_page_index(ptr)?;
    Some((page >> 6, 1u64 << (page & 63)))
}

#[inline]
fn task_page_mark(ptr: *mut u8) {
    if let Some((w, bit)) = task_page_bit(ptr) {
        TASK_PAGE_OWNED[w].fetch_or(bit, core::sync::atomic::Ordering::AcqRel);
    }
}
#[inline]
fn task_page_unmark(ptr: *mut u8) {
    if let Some((w, bit)) = task_page_bit(ptr) {
        TASK_PAGE_OWNED[w].fetch_and(!bit, core::sync::atomic::Ordering::AcqRel);
    }
}
/// Set while alloc_task_slot itself is allocating, so the heap's
/// alloc-side probe (R18-1) does not flag the legitimate first handoff.
pub static IN_TASK_ALLOC: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// stress-port r3 parkwalk census (rux:leakhunt (retired debug=leakhunt) only): [0] walks run,
/// [1] task observations, [2] parked-with-nonzero-count observations.
/// The hunt2 postmortem needs this: a 100 s strand produced ZERO
/// PARKED-LEAK lines, and the counters distinguish "walk blind" from
/// "victim parked clean, the phantom arose at resume".
/// nanosleep-loss: parks refused because the sleeper's one-shot wheel
/// entry was already consumed (the lost-wake rescue counter).
pub static NSLPARK_RESCUED: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

pub static PARKWALK_CENSUS: [core::sync::atomic::AtomicU64; 3] =
    [
        core::sync::atomic::AtomicU64::new(0),
        core::sync::atomic::AtomicU64::new(0),
        core::sync::atomic::AtomicU64::new(0),
    ];

/// Set while free_task_slot is releasing a Task page, so the heap's
/// free-side probe (R69) does not flag the legitimate ownership release.
pub static IN_TASK_FREE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// True if the page carrying `ptr` is marked as a live Task page.
///
/// R69: the old `task_page_word` helper indexed with the pre-Batch-8
/// aliasing hash — `(page >> 6) & 127` against the riscv RAM base —
/// while `task_page_mark`/`unmark` index by full physical page number
/// (Batch 8). `task_page_is_owned` therefore read bits nobody ever
/// wrote: the bitmap always read as zero and both probes built on it
/// (R17-B free refusal, R18-1 alloc-side handoff taint) were inert.
/// Index exactly like the mark side.

/// True if the page carrying `ptr` is marked as a live Task page.
#[inline]
pub fn task_page_is_owned(ptr: *const u8) -> bool {
    match task_page_index(ptr) {
        Some(page) => TASK_PAGE_OWNED[page >> 6]
            .load(core::sync::atomic::Ordering::Acquire)
            & (1u64 << (page & 63))
            != 0,
        None => false,
    }
}

/// Slot take (INV-TL-1): the ownership-take half of the task-page
/// lifecycle. Pairing audit — the page becomes a Task only through
/// this sequence, all inside one function with no yield point:
/// alloc (raw bytes, uninitialized) -> pid take (its failure deallocs
/// the SAME layout back — pre-init, no observer can hold the pointer)
/// -> new_task_at in-place init (its failure routes through
/// free_task_slot so the page leaves POISONED, never raw — the R48
/// note below) -> pid_hash_insert (the first publish; before it, no
/// other CPU can name this page). IN_TASK_ALLOC is a diagnostic flag
/// for the buddy's free-side audit, NOT an ownership tracker (no
/// second authority — the poison words themselves are the only
/// freed-state truth).
pub fn alloc_task_slot() -> Option<*mut Task> {
    let layout = core::alloc::Layout::new::<Task>();
    IN_TASK_ALLOC.store(true, core::sync::atomic::Ordering::Release);
    // SAFETY (INV-TL-1): Layout is non-zero (Task is sized); the raw
    // allocator has no safe wrapper that preserves the Task slot's
    // raw-pointer identity (Round-B typed-handle conversion pending);
    // null check follows immediately.
    let task_ptr = unsafe { alloc::alloc::alloc(layout) } as *mut Task;
    IN_TASK_ALLOC.store(false, core::sync::atomic::Ordering::Release);
    if task_ptr.is_null() {
        return None;
    }

    let pid = match alloc_pid() {
        Some(p) => p,
        None => {
            // SAFETY: task_ptr was allocated above with the same Layout; not yet initialized.
            unsafe { alloc::alloc::dealloc(task_ptr as *mut u8, core::alloc::Layout::new::<Task>()); }
            return None;
        }
    };

    // SAFETY: task_ptr was freshly allocated with Layout::new::<Task>() and is non-null;
    // new_task_at initializes it in-place before use.
    unsafe {
        if !Task::new_task_at(task_ptr, pid, SchedPolicy::Normal) {
            // R10-9: stack allocation failed — discard the slot cleanly.
            crate::process::process_hooks::free_pid(pid);
            // R48: new_task_at already wrote state=TASK_NEW (R52; was
            // RUNNING) and pid=<pid> over any poison left by the page's
            // previous life BEFORE it failed — a raw dealloc here would
            // return an UNPOISONED,
            // valid-looking Task page to the buddy, disarming every
            // R15-6/R33/R47 freed-page guard (a stale wake would then
            // sail through the pid check, read a runnable-looking state,
            // and reach the wake path — pre-R52 it read RUNNING and the
            // R46 heal re-enqueued the half-initialized task: on_cpu=false,
            // on_rq=false, state=RUNNING was exactly the phantom shape;
            // TASK_NEW now refuses every wake route, and the poison keeps
            // the pid check as the first-line drop).
            // Route through free_task_slot so the page leaves POISONED at
            // the compile-time offsets and the slot hits FREED_TASK_RING.
            free_task_slot(task_ptr);
            return None;
        }
        crate::process::pid_hash::pid_hash_insert(task_ptr);
    }

    // R69: the ownership bitmap was never populated — task_page_mark had
    // no callers, so every probe reading TASK_PAGE_OWNED (the R18-1
    // alloc-side handoff taint in the buddy, the R69 free-side audit)
    // saw a permanently-zero table. Mark live Task pages from the day
    // one place that creates them.
    task_page_mark(task_ptr as *mut u8);

    Some(task_ptr)
}

/// R12-4: recently-freed Task slots (pid at free time) for post-mortems.
pub static FREED_TASK_RING: [core::sync::atomic::AtomicU32; 64] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 64];
static FREED_RING_HEAD: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// R12-4: poison marker written into a freed Task's state/pid words.
pub const TASK_POISON: u32 = 0xDEAD_BEEF;

/// Slot release (INV-TL-2): the terminal-release half of the
/// lifecycle — the family the sc191 v2 fix and the R63
/// terminal_switch_done protocol hang off. The single-atomic-window
/// discipline: between the last observer's release (the callers'
/// task_put / terminal_switch_done handshake proves no CPU still
/// runs or can still reach the task) and the dealloc below there is
/// ONE window with ONE writer (this CPU); the steps inside it are
/// strictly ordered — wakewatch stamp invalidation (phantom-strand
/// guard), drop_in_place (field destructors; the Arc fields were
/// already released to None by the exit paths), the R12-4 ring
/// record, the R47 positional poison of state+pid (the ONLY
/// freed-state authority — R15-6/R33/R41/R47 guards read it), the
/// R83 header paint, task_page_unmark, and the dealloc. NO second
/// tracker: IN_TASK_FREE is the buddy audit's diagnostic flag, not
/// ownership. No safe shape exists for this sequence: it writes
/// through a pointer that is logically dead (the poison/paint) —
/// by definition outside the language's model — and runs the
/// destructor for the last reference the type system cannot see.
pub fn free_task_slot(task_ptr: *mut Task) {
    if task_ptr.is_null() {
        return;
    }
    // freed-task lane (2026-11-15, live capture in the dio_read storm):
    // a CPU whose PER_CPU current slot still names this task keeps
    // dereferencing — and MUTATING — the page after the free. Captured
    // face: scheduler_tick resolved current from the stale slot, ran
    // Task::rlimit() (a Spinlock INSIDE the Task page), whose bracket
    // fetched +1/-1 on the painted ti_preempt_count
    // (PREEMPT-UNDERFLOW count_was=0xc3c3c3c4 at spinlock.rs:758,
    // FREED-TOUCH cpu-cur!0, chain scheduler_tick->Task::rlimit), and
    // once the page is recycled the same stale slot books phantom
    // counts onto the new tenant (the WAKE-HEAL was=1 class) and walks
    // VMAs through freed address spaces (the do_page_fault NULL+8
    // face). The reference's task_free rule: a task struct outlives
    // every CPU that still names it (finish_task_switch drops the last
    // ref; the free never races a stale current). on_cpu (cleared by
    // __switch_to) is NOT sufficient — the R9 gate upstream proves the
    // mark and the slot can diverge (lying slot). Gate the free on the
    // scheduler's own truth: no PER_CPU current slot and no in-flight
    // SWITCHING_OUT marker may name the task. Bounded, yielding (same
    // discipline as free_dead_member's bounded spin batches — the exit
    // tails this waits on make progress only if this CPU can schedule);
    // past the deadline the page LEAKS loudly instead of dangling —
    // one page per wedged CPU, never a use-after-free.
    {
        const FTS_SLOT_WAIT_JIFFIES: u64 = 300; // 3 s, R86/FDM budget
        const FTS_SPIN_BATCH: u32 = 100_000;
        let mut spins: u32 = 0;
        let armed_at = crate::timer::get_jiffies();
        let mut printed = false;
        crate::arch::cpu::restore_irq(true);
        loop {
            let mut owner: usize = usize::MAX;
            for c in 0..crate::config::MAX_CPUS {
                if cpu_state(c).current == task_ptr {
                    owner = c;
                    break;
                }
            }
            if owner == usize::MAX
                && (0..crate::config::MAX_CPUS).all(|c| {
                    SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire)
                        != task_ptr as u64
                })
            {
                break;
            }
            if !printed {
                printed = true;
                use crate::debug::taskdump::putc_brief as putchar;
                fn pdec_fts(mut v: u64) {
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
                    while n > 0 {
                        n -= 1;
                        p(buf[n]);
                    }
                }
                const M: &[u8] = b"\nFTS-SLOT-WAIT pid=";
                for &b in M {
                    putchar(b);
                }
                pdec_fts(crate::sched::edges::task_shared(task_ptr).pid() as u64);
                const M2: &[u8] = b" cpu=";
                for &b in M2 {
                    putchar(b);
                }
                if owner != usize::MAX {
                    pdec_fts(owner as u64);
                } else {
                    const SW: &[u8] = b"switching-out";
                    for &b in SW {
                        putchar(b);
                    }
                }
                putchar(b'\n');
            }
            if crate::timer::get_jiffies().wrapping_sub(armed_at)
                >= FTS_SLOT_WAIT_JIFFIES
            {
                // Leak, don't dangle: the owning CPU is wedged with a
                // stale slot. One page per such CPU; the alternative is
                // every later dereference of the recycled page
                // (phantom counts, wrong-task wakes, freed-mm walks).
                use crate::debug::taskdump::putc_brief as putchar;
                const M: &[u8] = b"\nFTS-SLOT-LEAK (page withheld) pid=";
                for &b in M {
                    putchar(b);
                }
                let mut v = crate::sched::edges::task_shared(task_ptr).pid() as u64;
                {
                    let mut buf = [0u8; 10];
                    let mut n = 0;
                    if v == 0 {
                        n = 1;
                        buf[0] = b'0';
                    }
                    while v > 0 && n < buf.len() {
                        buf[n] = b'0' + (v % 10) as u8;
                        v /= 10;
                        n += 1;
                    }
                    let mut i = n;
                    while i > 0 {
                        i -= 1;
                        putchar(buf[i]);
                    }
                }
                putchar(b'\n');
                // Keep the pid-hash state consistent: the caller-side
                // bookkeeping already ran; nothing else to do — the
                // page simply never returns to the allocator.
                return;
            }
            spins = spins.wrapping_add(1);
            if spins % FTS_SPIN_BATCH == 0 {
                crate::sched::schedule();
            } else {
                core::hint::spin_loop();
            }
        }
    }
    // ORPHAN-MARK residue check (wake-fastrepro final-boss): a Task page
    // entering the free pool with a dirty ownership qword hands its
    // residue to whatever recycles the page (the constructors reset the
    // qword, so this is belt-and-braces — but a set mark here names a
    // lifecycle that never released, upstream of every later
    // STALE-CLAIM-RESET on the recycled page). Budgeted, direct console.
    {
        // SAFETY: pre-poison header reads; the caller owns the final
        // reference.
        {
            let tp = crate::sched::edges::task_shared(task_ptr);
            let oncpu = tp.on_cpu(); let claim = tp
                .running_on_cpu
                .load(core::sync::atomic::Ordering::Acquire);
            if (oncpu || claim != -1) && tp.pid() != 0 {
                static FREEC_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if FREEC_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn pdec3(mut v: u64) {
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
                    const M: &[u8] = b"\nFREE-CLAIMED pid=";
                    for &b in M {
                        putchar(b);
                    }
                    pdec3(tp.pid() as u64);
                    const M2: &[u8] = b" oncpu=";
                    for &b in M2 {
                        putchar(b);
                    }
                    pdec3(oncpu as u64);
                    const M3: &[u8] = b" claim=";
                    for &b in M3 {
                        putchar(b);
                    }
                    pdec3(claim.unsigned_abs() as u64);
                    putchar(b'\n');
                }
            }
        }
    }
    // The sched:sched_process_free tracepoint — the reference records
    // it at the final put (delayed_put_task_struct → trace →
    // put_task_struct), i.e. right here at the last-reference free,
    // before the page is poisoned/unlinked. Closes the
    // fork→exit→free lifecycle: exit-without-free is the leak
    // signature. Marshals only behind armed().
    if crate::trace::EVENT_SCHED_PROCESS_FREE.armed() {
        // SAFETY: task_ptr is non-null and still a valid Task — the
        // caller's last task_put guarantees this call owns the final
        // reference, so no concurrent reader can be freeing it too.
        {
            let words = crate::trace::comm_to_words(crate::sched::edges::task_shared(task_ptr).comm());
            crate::trace::record(
                &crate::trace::EVENT_SCHED_PROCESS_FREE,
                [
                    words[0],
                    words[1],
                    crate::sched::edges::task_shared(task_ptr).pid() as u64,
                    crate::sched::edges::task_shared(task_ptr).prio() as u64,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
            );
        }
    }

        // FREED-WHILE-LINKED UNLINK (oh-display b61-b66 family): a Task
        // page returned to the heap while still linked on a class queue
        // gets recycled (new Tasks, BTreeMap nodes, kernel stacks), and
        // every later walk of that queue dereferences the recycled page —
        // the "POISONED rq entry" / pick-path KERNPANIC family at
        // T+15-19min. The old check only REPORTED and only under
        // rux:scribble (retired debug=scribble); unlink for real, unconditionally, while the page
        // is still valid so del()/BTreeMap-removal write to the real
        // neighbors. RT uses force_unlink (does not trust the possibly
        // stale on_rq flag); CFS/DL dequeue scan by pointer and are
        // flag-independent already. grq.nr_running is paired per actual
        // removal.
        {
            let mut guard = grq().lock_irqsave();
            let mut unlinked = 0usize;
            // SAFETY: task_ptr is non-null and still a valid Task (poison
            // and dealloc happen below); the GRQ lock is held.
            {
                if guard.rt_rq.force_unlink(task_ptr) {
                    unlinked += 1;
                }
                if guard.cfs_rq.is_linked(task_ptr)
                    && guard.cfs_rq.dequeue(task_ptr)
                {
                    unlinked += 1;
                }
                if guard.dl_rq.is_linked(task_ptr)
                    && guard.dl_rq.dequeue(task_ptr)
                {
                    unlinked += 1;
                }
            }
            for _ in 0..unlinked {
                guard.nr_running.fetch_update(
                    core::sync::atomic::Ordering::SeqCst,
                    core::sync::atomic::Ordering::SeqCst,
                    |v| v.checked_sub(1),
                );
            }
            if unlinked > 0 {
                // Rule 1 (grq-lock-1st): event ring, not the console
                // (pid = the dying task's last pid, a = the task page,
                // b = how many class queues it was unlinked from).
                crate::sched::sched_hooks::sched_ev(
                    crate::sched::sched_hooks::EV_FREED_WHILE_LINKED,
                    crate::sched::edges::task_shared(task_ptr).pid(),
                    task_ptr as usize as u64,
                    unlinked as u64,
                );
            }
        }
        // Scribble hunter (rux:scribble (retired debug=scribble)): stop tracking before the page
        // is dropped/deallocated (it may be recycled immediately).
        #[cfg(feature = "x86_64")]
        crate::debug::scribble::unregister(task_ptr);
        // Scribble hunter: freed-while-linked UAF check — a Task page
        // returned to the heap while still linked in its class tree
        // gets recycled as BTreeMap nodes / new Tasks, and subsequent
        // tree ops write node pointers into the recycled page (the
        // Task-pointer-fragment scribble family). Loud if it happens.
        #[cfg(feature = "x86_64")]
        if crate::trace::EVENT_SCRIBBLE.armed() {
            if let Some(grq_guard) = try_grq_lock() {
                use core::sync::atomic::Ordering as O;
                let policy = crate::sched::edges::task_shared(task_ptr).policy();
                let linked = match policy {
                    SchedPolicy::Fifo | SchedPolicy::Rr => grq_guard.rt_rq.is_linked(task_ptr),
                    SchedPolicy::Deadline => grq_guard.dl_rq.is_linked(task_ptr),
                    _ => grq_guard.cfs_rq.is_linked(task_ptr),
                };
                let flagged = crate::sched::edges::task_shared(task_ptr).pid() == TASK_POISON;
                if linked || flagged {
                    use crate::debug::taskdump::putc_brief as putchar;
                    const MSG: &[u8] = b"\nFREED-WHILE-LINKED task=0x";
                    for &b in MSG {
                        putchar(b);
                    }
                    let mut v = task_ptr as usize;
                    for _ in 0..16 {
                        let n = (v >> 60) as u8;
                        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                        v <<= 4;
                    }
                    const M2: &[u8] = b" linked=0x";
                    for &b in M2 {
                        putchar(b);
                    }
                    putchar(b'0' + linked as u8);
                    const M3: &[u8] = b" poison=0x";
                    for &b in M3 {
                        putchar(b);
                    }
                    let p = crate::sched::edges::task_shared(task_ptr).pid() as usize;
                    for sh in (0..8u32).rev() {
                        let n = ((p >> (sh * 4)) & 0xF) as u8;
                        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                    }
                    putchar(b'\n');
                }
                drop(grq_guard);
                let _ = O::Relaxed;
            }
        }
        // Heap-v3 leak fix: run the Task field destructors BEFORE returning
        // the page. The Task owns heap memory in plain (non-Arc) fields —
        // exe_path: Box<[u8]> (re-set on every execve by set_exe_path),
        // sem_undo / dead_threads / wait_chldexit Vecs, queued siginfo,
        // seccomp_filter — and dealloc() below releases only the Task's
        // own bytes, so every one of those allocations used to leak.
        // Observed signature (rux:memwatch (retired debug=memwatch)): MEMLIVE site
        // Task::set_exe_path b=3 sz=9 live=603 a=603 f=0 under a pure
        // exec loop — one 9-byte Box per exec pinning a whole 4kB buddy
        // page (+4kB/exec, +1.2MB/min steady-state HeapUsed growth).
        //
        // drop_in_place is safe at every call site: new_task_at's ONLY
        // failure return sits AFTER all fields are initialized (the
        // kernel-stack alloc is its last step), and the fork/kthread
        // unwind paths plus release_task/free_dead_member all operate on
        // fully initialized tasks. The Arc fields (address_space,
        // fdtable, signal, fs) are already explicitly released to None on
        // the exit paths, so their drops here are no-ops there; on the
        // fork unwind paths dropping the cloned Arcs is exactly the
        // correct refcount decrement.
        // Wakewatch: invalidate the wake stamp keyed by this address
        // BEFORE the page is recycled — a new Task at the same address
        // must not inherit the old occupant's stamp (phantom-strand
        // guard, see wakewatch::note_task_freed).
        crate::sched::sched_hooks::note_task_freed(task_ptr);
        // SAFETY (INV-TL-2): drop_in_place runs the field destructors
        // for the page's last type-system-invisible reference; the
        // call-site audit (the comment block above) proves every path
        // here operates on a FULLY initialized task (new_task_at's only
        // failure return precedes any Drop-bearing field).
        unsafe {
            core::ptr::drop_in_place(task_ptr);
        }
        // R12-4: poison + record before the free — the next "zeroed/garbage
        // linked child" or wild-pointer wake then shows this exact marker
        // instead of anonymous zeros, proving (or ruling out) the
        // freed-while-referenced family at first sight.
        // SAFETY (INV-TL-2, the poison window): every write below goes
        // through a pointer the type system already considers dead —
        // that is the POINT of the poison discipline (the R47/R12-4
        // guards read these words to refuse stale wakes). Volatile
        // stores at the compile-time offsets; the paint loop bounds
        // itself to 0x100 bytes; the dealloc closes the window.
        unsafe {
            let slot = task_ptr as *mut u8;
            let h = FREED_RING_HEAD.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % 64;
            FREED_TASK_RING[h].store((*task_ptr).pid(), core::sync::atomic::Ordering::Relaxed);
            // R47: poison state and pid at their COMPILE-TIME offsets
            // (task_offsets::TASK_STATE/TASK_PID; currently 0x50/0x54). The
            // previous hardcoded `slot.add(0x48)` u64 write predated the
            // journal_handle/ti_on_cpu/task_refcnt insertion before `state`
            // (the same +8 drift trap.rs R20 fixed for its own walk) and
            // landed entirely inside ti_a2 — so every poison reader
            // (Task::wake_up R15-6, enqueue_task_locked R15-6,
            // ensure_linked_locked R41, trap.rs panic walk) checked fields
            // that were never poisoned. Freed Task pages were
            // indistinguishable from live ones, and the stale wakes those
            // guards exist to drop proceeded to set_state(RUNNING == 0)
            // into freed-then-reused pages — the positional 4-byte zero at
            // the state word behind the phantom-RUNNING / recurrence-at-
            // same-address signature (rounds 39-47 form-B). Two separate
            // u32 writes (not one u64) so a future field inserted between
            // state and pid cannot silently miss pid again.
            core::ptr::write_volatile(
                slot.add(crate::process::task::TASK_STATE) as *mut u32,
                TASK_POISON,
            );
            core::ptr::write_volatile(
                slot.add(crate::process::task::TASK_PID) as *mut u32,
                TASK_POISON,
            );
            // (2026-11-23: the R83 freed-header paint loop that lived here
            // retired with debug/r83hunt.rs — its only reader was the hunt's
            // audit/dump; the R47/R12-4 state/pid poison above is what the
            // stale-wake guards read and it is unchanged.)
            // R69: release the ownership bit and tell the heap's free-side
            // audit that THIS free is the legitimate one (anything else
            // freeing a live Task page is the double-handoff engine).
            task_page_unmark(slot);
            IN_TASK_FREE.store(true, core::sync::atomic::Ordering::Release);
            alloc::alloc::dealloc(slot, core::alloc::Layout::new::<Task>());
            IN_TASK_FREE.store(false, core::sync::atomic::Ordering::Release);
        }
}

// ==================== Core Scheduling ====================


#[inline(never)]
#[track_caller]
pub fn schedule() {
    // __schedule_bug parity (the reference's kernel/sched/core.c
    // discipline): schedule() from interrupt context is forbidden — in
    // the reference it splats __schedule_bug(prev) and every sleeping
    // primitive BUGs first via might_sleep(), so the invariant
    // "SOFTIRQ/HARDIRQ_OFFSET exists only inside its own bracket and a
    // switch can never carry it to another task" holds structurally.
    // This tree lacked the check: a softirq-context sleeper's schedule
    // proceeded silently, the bracket's add/sub pair straddled two
    // task contexts, and the migrated +SOFTIRQ_OFFSET stranded on a
    // worker forever (the lat_fs strand family, 2026-11-19 conviction:
    // PREEMPT-HEAL-REFUSED pc=0x2 with 377 fresh observations). The
    // switch is REFUSED here (the count cannot migrate); the sleeping
    // primitive's wait fails/spins and the offender is named.
    if crate::interrupt::preempt::in_interrupt() {
        static ATOMIC_N: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let n = ATOMIC_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 24 {
            let loc = core::panic::Location::caller();
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
            crate::pr_err!(
                "SCHEDULE-ATOMIC (reference __schedule_bug parity) caller={}:{} pid={} pc={:#x} in_softirq={} in_irq={}",
                loc.file(),
                loc.line(),
                crate::process::current_pid(),
                crate::interrupt::preempt::preempt_count(),
                crate::interrupt::preempt::in_softirq(),
                crate::interrupt::preempt::in_irq(),
            );
        }
        return;
    }
    // SP-5 r5 frozen-tick trap: schedule() entered with IF=0. The tail's
    // per-caller restore_irq(flags) then returns with IF STILL 0, and the
    // caller keeps running with IRQs off until some other sti — the
    // observed 300 ms both-CPU tick-death windows open exactly at the GRQ
    // acquire inside this call (TICKTRAP cli_open + syscost max segments).
    // The reference's only legitimate IF=0 caller is the IRQ-exit
    // preemption path (preempt_schedule_irq shape); anything else is a
    // sleeping-with-IRQs-off bug. Rate-limited one-liner naming the caller.
    if !crate::arch::cpu::get_interrupts_state() {
        static IRQOFF_N: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        if IRQOFF_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 12 {
            let loc = core::panic::Location::caller();
            crate::pr_debug!(
                "SCHED-IRQOFF caller={}:{} pid={} pc={:#x}",
                loc.file(),
                loc.line(),
                crate::process::current_pid(),
                crate::sched::current()
                    .map(|t| t.preempt_count())
                    .unwrap_or(0)
            );
        }
    }
    // Recursion guard (kernel-preemption round 2, 2026-10-20). While a
    // schedule() call is in progress on this task, the IRQ-exit preemption
    // hook must not re-enter it: __schedule's tail restores the incoming
    // task's saved IRQ state, and with IF re-enabled a timer tick can land
    // inside the unwinding frame — re-entering schedule() on the same
    // half-unwound stack (the reverted 2cf4bdc9 failure mode).
    //
    // Round 1 used a PREEMPT_ACTIVE bit inside preempt_count (the Linux
    // CONFIG_PREEMPT design). sc65/sc66 falsified it for THIS tree: the
    // count is per-task and there are residual cross-task imbalance
    // producers in the pick/heal machinery (sc66 strand incumbents showed
    // init carrying 0x04000001 and even 0x08000001 — leaked ACT bits plus
    // a leaked PREEMPT_OFFSET), and every leaked count bit now has teeth:
    // it makes the task permanently unpreemptable AND un-sleepable (the
    // __schedule discipline refuses to switch out a task whose count is
    // nonzero — the 160-291 s one-activation monopolies). A leaked TIF
    // bit instead only disables preemption for that task; sleeping and
    // the discipline semantics stay exact.
    //
    // TIF_IN_SCHED is a bit in the existing ti_flags word (no Task layout
    // change — the scribble machinery is offset-sensitive). Set on entry,
    // cleared on return from __schedule in the same frame (per-task, so
    // coroutine resume/migration pair it correctly, unlike a per-CPU
    // flag).
    use crate::process::task::task_flags::TaskFlags;
    const IN_SCHED: u32 = TaskFlags::TIF_IN_SCHED.bits();
    if let Some(cur) = current() {
        cur.set_ti_flag(IN_SCHED);
    }
    // SAFETY: __schedule() manipulates raw task pointers and calls context_switch;
    // must be called from kernel context with valid current task pointer.
    __schedule();
    if let Some(cur) = current() {
        cur.clear_ti_flag(IN_SCHED);
    }
}

#[no_mangle]
pub extern "C" fn asm_schedule() {
    schedule();
}

/// Terminal deschedule for a DEAD task — do_exit's "final schedule, never
/// returns". sc186/sc189 (session R eighth leg): a racing wake/heal can
/// re-link the exiting task between its dequeue and the final pick, and
/// __schedule's same-task repick fast path then RETURNS without a context
/// switch — do_exit's schedule() returns, the wfi park loop after it runs,
/// and the CPU slot keeps naming the dead task forever while its leaked
/// preempt bits (0x10000 hardirq observed) sit uncleared: the cpu is lost
/// for the rest of the boot (PREEMPT-LEAK cpu=0 pc=0x10000 rip=wfi+5 held
/// 10+ min; sa_main OS_IPC threads right after fatal USEGV at rip=0,
/// twice reproduced — each sa_main null-jump halved the machine).
///
/// Discipline here: re-assert the terminal state each round (a task that
/// will never run userspace again holds no critical section worth
/// stranding a CPU for), clear anything that can make the discipline or
/// the recursion guard refuse the switch, and retry until a REAL context
/// switch happens. Never returns to the caller on success.
/// Death-path GRQ release (oh-launch round 6; reworked grq-source-fix):
/// a task/CPU that dies or parks forever inside a GRQ section strands
/// the lock WORD (its paired unlock will never run) — every later GRQ
/// acquirer spins permanently. gw1 (gdbstub): 4 CPUs at
/// RawSpinlock::lock+1359 behind a task whose terminal switch had
/// cleared only the preempt count; gw2: 7 CPUs behind a CPU parked in
/// the KERNPANIC halt loop with the GRQ held.
///
/// grq-source-fix: the ownership proof is the LOCK WORD itself (0 =
/// free, cpu+1 = held by cpu — the single authority). The round-6
/// per-CPU tracker made this a two-step release whose gap the gl3
/// orphaned-word class lived in, and its heal branch (951d9ec33)
/// force-released an UNCLAIMED word — self-invented remediation with
/// no reference counterpart, the lying-slot-heal mistake shape. Both
/// are gone: a release here happens only when the word POSITIVELY
/// names this cpu, so it can never touch another cpu's live section,
/// and an interruption anywhere else in the protocol leaves a word
/// that still names its owner for a later terminal pass to release.
/// Callers: the terminal switch (schedule_dead_never_returns), the
/// oops policy (before every arm), and the KERNPANIC re-entry park
/// arms (x86_64 + riscv64 twins).
pub fn terminal_release_grq_if_held() {
    let cpu = crate::arch::cpu_id() as usize;
    if !crate::sched::edges::grq_is_ready() {
        return;
    }
    // SAFETY: GRQ_READY checked. held_by reads the word read-only; only
    // this cpu's own acquire/release writes this cpu's ownership into
    // it, so a `true` here is stable until our own release below.
    if !grq().lock.held_by(cpu) {
        return;
    }
    static GRQFORCE_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = GRQFORCE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // RELEASE THE LOCK FIRST, REPORT AFTER (grq-lock-1st).
    //
    // The report used to go through printk, and printk is NOT
    // lock-free: pr_warn! -> printk_ln -> write_to_ring_buffer ->
    // KMSG_READ_WQ.wake_up_all() -> WaitQueueHead::wake_up ->
    // wake_up_process -> wake_up_enqueue_opt -> grq().lock_irqsave().
    // A /dev/kmsg reader parked on KMSG_READ_WQ therefore turns a
    // printk issued while THIS CPU holds the GRQ into a
    // SELF-DEADLOCK: the wake re-acquires the very lock this CPU
    // already owns, and the spin runs with IRQs off forever — the
    // exact "IRQs-dead GRQ holder" signature the OH campaign kept
    // capturing (four CPUs at RawSpinlock::lock+1359 behind one
    // holder). The cross-CPU form is the textbook ABAA: this CPU
    // holds the GRQ and wants the waitqueue lock; the other holds
    // the waitqueue lock (inside wake_up) and wants the GRQ.
    //
    // The release therefore comes FIRST, and the report goes out
    // through the LOCK-FREE raw console (uart_putc_raw — the same
    // path the KERNPANIC park uses), never through printk.
    //
    // SAFETY: grq() returns the initialized GRQ (GRQ_READY checked
    // above); the word names THIS cpu as the holder (held_by).
    grq().lock.force_unlock_terminal();
    if n < 8 {
        let pid = crate::sched::current()
            .map(|t| crate::sched::edges::task_shared(t).pid())
            .unwrap_or(0);
        grq_raw_puts(b"TERM-GRQ-FORCE pid=");
        grq_raw_putdec(pid as u64);
        grq_raw_puts(b" cpu=");
        grq_raw_putdec(cpu as u64);
        grq_raw_puts(b" released the runqueue lock stranded by the dying section\n");
    }
}

/// Lock-free raw console writers for the GRQ death path (grq-lock-1st).
///
/// These exist because printk is NOT lock-free: its ring-buffer write
/// ends in `KMSG_READ_WQ.wake_up_all()`, whose waitqueue lock is held
/// across `wake_up_process` — which takes the GRQ. A printk issued
/// under the GRQ therefore self-deadlocks (see
/// `terminal_release_grq_if_held`). `uart_putc_raw` writes the serial
/// port directly: no printk ring, no waitqueue, no scheduler lock.
fn grq_raw_puts(bytes: &[u8]) {
    for &b in bytes {
        crate::console::uart_putc_raw(b);
    }
}

fn grq_raw_putdec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    loop {
        buf[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        crate::console::uart_putc_raw(buf[n]);
    }
}

fn grq_raw_puthex(v: u64) {
    let mut shift: i32 = 64;
    while shift > 0 {
        shift -= 4;
        let nibble = ((v >> shift) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        crate::console::uart_putc_raw(c);
    }
}

pub fn schedule_dead_never_returns() {
    use crate::process::task::task_flags::TaskFlags;
    const IN_SCHED: u32 = TaskFlags::TIF_IN_SCHED.bits();
    // boot-63 residue: the pacing flag — positive knowledge that this
    // task still executes here. Cleared only at the real switch-out
    // retire sites (beside terminal_switch_done); the reap drainer's
    // sustained arm waits on it instead of a timing bet.
    if let Some(cur0) = current() {
        cur0.terminal_pacing.store(true, core::sync::atomic::Ordering::Release);
    }
    static TERM_RETRY_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    static TERM_ENGAGE_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let mut rounds = 0u32;
    loop {
        if let Some(cur) = current() {
            // SAFETY: cur is the running task (this very context).
            {
            // INV-SCE-2: cur is the dying task this CPU is parking.
            let cur_t = crate::sched::edges::task_shared(cur);

                // sc192 (v3): print ENGAGEMENT (once, budgeted) so the
                // next strand can distinguish "helper never ran" from
                // "helper ran silently with pc==0" — sc191/sc192 both
                // stranded with ZERO TERM-DESCHED lines and the two were
                // indistinguishable in the log. Serial-cleanliness:
                // census boots only (rux:wwcensus (retired debug=wwcensus)); ordinary boots fire
                // this on every exiting child.
                if rounds == 0
                    && crate::trace::EVENT_WWCENSUS.armed()
                {
                    let n = TERM_ENGAGE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 8 {
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
                        crate::pr_warn!(
                            "TERM-DESCHED engaged pid={} pc={:#x}",
                            crate::sched::edges::task_shared(cur).pid(),
                            crate::sched::edges::task_shared(cur).preempt_count()
                        );
                    }
                }
                // A racing wake/heal may have re-linked us or rewritten
                // our state: re-assert DEAD and unlink before every
                // attempt so the picker can never hand the CPU back.
                if !cur_t.state().is_dead() {
                    cur_t.set_state(TaskState::new(TaskState::DEAD));
                }
                dequeue_task(&*cur);
                cur.clear_ti_flag(IN_SCHED);
                // sc191 (v2): a CLEAN thread exit (libusb_event tid 649,
                // code=0) still stranded in the v1 fallback — the pick
                // machinery kept resolving the DEAD task as its own pick
                // because its pick-ownership pair ({on_cpu, claim}, the
                // R63 mid-switch-out shape) was never released: the real
                // switch-out asm does this store, but the repick loop
                // never reaches a switch. Release it explicitly — the
                // same store the R56 heal paths use — so this CPU's pick
                // can move to anything else, including idle.
                // R11 (the terminal-release window): the release above is
                // exactly what makes the free-side waits alone UNSAFE —
                // from here until the SWITCHING_OUT publish inside the
                // schedule() below, the task reads {on_cpu=false, no
                // marker} while still executing on its own stack. Re-arm
                // the positive terminal flag each round: only the REAL
                // final switch-out (the retire site, in the incoming
                // context) sets it again — the free side waits on it, so
                // the window closes for every retry of this loop.
                cur_t.terminal_switch_done.store(
                    false,
                    core::sync::atomic::Ordering::Release,
                );
                cur_t.release_pick_ownership();
                let pc = crate::sched::edges::task_shared(cur).preempt_count();
                if pc != 0 {
                    // Leaked bits protect nothing on a task that will
                    // never run again. Name them once per boot for the
                    // count-producer hunt (F1 lane), then clear.
                    let n = TERM_RETRY_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 4 {
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
                        crate::pr_warn!(
                            "TERM-DESCHED pid={} leaked preempt_count={:#x} cleared for final switch (round {})",
                            crate::sched::edges::task_shared(cur).pid(),
                            pc,
                            rounds
                        );
                    }
                    crate::interrupt::preempt::preempt_count_reset();
                }
                // gw1 (oh-launch round 6): the count clear above is only
                // half the terminal cleanup — see terminal_release_grq_
                // if_held (the shared death-path helper).
                terminal_release_grq_if_held();
            }
        }
        schedule();
        rounds += 1;
        // schedule() returning means it refused/repicked us once more;
        // loop and re-assert. sc192 (v3): the old "park in wfi like the
        // pre-v1 code did" fallback after 8 rounds is a PERMANENT CPU
        // loss whenever the re-link race outlasts the budget — observed
        // on a SIGKILL teardown of an on-CPU FFRT worker (launcher tgid
        // 5111, tid 5129, killed mid-futex-wait): the bare wfi loop never
        // re-enters schedule(), so no later interrupt or wake can hand
        // this CPU to another task; the slot named the dead task for 60+
        // s (rip=wfi+5, preempt_count froze at 0x10000 after an IRQ hit
        // the park). A DEAD task holds no critical section worth
        // stranding a CPU for: instead pace the retry with wfi — each
        // interrupt (the timer tick at minimum) is another chance for
        // the picker, now seeing {DEAD, dequeued, unowned}, to switch us
        // out for good. wfi needs IF=1; force it on (a dead task holds
        // no critical section to protect).
        if rounds >= 8 {
            // v4 forensics: name the stuck shape at power-of-two rounds so
            // the next strand says WHY schedule() keeps returning — state
            // rewritten by a racer (not DEAD), leaked count back (0x10000),
            // or a pick that keeps resolving us. Budget: log2 rounds.
            if rounds.is_power_of_two() && rounds <= 4096 {
                if let Some(cur) = current() {
                    let st = crate::sched::edges::task_shared(cur).state();
                    let pc = crate::sched::edges::task_shared(cur).preempt_count();
                    let claim = crate::sched::edges::task_shared(cur).running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
                    // R13 (the liveness stall discriminator): is the pacing
                    // DEAD task still LINKED in its class tree? linked=true
                    // on a DEAD+dequeued task means a RE-LINKER exists (a
                    // wake/heal path resurrecting it — the pick keeps
                    // finding it); linked=false with the loop still pacing
                    // means the pick/refuse decision resolves prev some
                    // other way (the same-task repick fast path). One read
                    // under the GRQ at power-of-two rounds, cold path.
                    let linked = {
                        let g = grq().lock_irqsave();
                        let l = {
                            let cur_t = crate::sched::edges::task_shared(cur);
                            match cur_t.policy() {
                                crate::process::task::SchedPolicy::Fifo
                                | crate::process::task::SchedPolicy::Rr => {
                                    g.rt_rq.is_linked(cur)
                                }
                                crate::process::task::SchedPolicy::Deadline => {
                                    g.dl_rq.is_linked(cur)
                                }
                                _ => g.cfs_rq.is_linked(cur),
                            }
                        };
                        let nrr = g.nr_running.load(core::sync::atomic::Ordering::Relaxed);
                        drop(g);
                        (l, nrr)
                    };
        // HOTPATH-PRINT: hunt-era diagnostic on a cold/rare path (wedge adjudication); rate-bounded by its trigger condition.
                    crate::pr_warn!(
                        "TERM-DESCHED stuck pid={} rounds={} state={:#x} pc={:#x} claim={} linked={} nr_running={} rip_paced",
                        crate::sched::edges::task_shared(cur).pid(),
                        rounds,
                        st.bits(),
                        pc,
                        claim,
                        linked.0,
                        linked.1
                    );
                }
            }
            crate::arch::cpu::enable_irq();
            // SAFETY: plain hint instruction, no side effects.
            crate::arch::cpu::wfi();
        }
    }
}

/// The idle task's comm, standard shape: "swapper/<cpu>" (used by
/// the sched:sched_switch tracepoint; Rux's idle Task keeps an empty
/// comm).
fn swapper_comm(cpu: usize) -> [u8; 16] {
    let mut out = [0u8; 16];
    let name = alloc::format!("swapper/{}", cpu);
    let bytes = name.as_bytes();
    let n = bytes.len().min(15);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

fn __schedule() {
    // cctx probe (closegap lane): phase NS sums for the whole schedule
    // body. t0 at entry; the phases add below bracket guards, GRQ wait,
    // accounting, prev dequeue/requeue, pick, publish. Off = zero cost
    // (now() short-circuits to 0, add() to a no-op).
    let cc_t0 = crate::debug::cctx::now();
    crate::debug::cctx::bump(&crate::debug::cctx::SCH_N);
    // runq-strand lane: per-CPU schedule-entry count — the monopolist
    // census reads its per-burst delta (frozen = a CPU that stopped
    // scheduling; the strand producer's signature).
    {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id < MAX_CPUS {
            sched_calls_inc(cpu_id);
        }
    }
    clear_need_resched();

    // Context switch is an RCU quiescent state.
    crate::sync::rcu::rcu_note_context_switch();

    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    // PER_CPU scribble heal (b81 GDB capture): the wild-write engine that
    // hit hpet::BASE (b77) also zeroes/corrupts PER_CPU slots — cpu1 ran
    // with current=nil and idle=<random heap task>, so every "nothing
    // runnable" pick context-switched into an arbitrary task's stale
    // context (the b66 resume-family symptom, now from a different
    // producer) and the machine crawled on one vCPU. The idle tasks are
    // STATIC (IDLE_TASK_STORAGES, written once by init) and current is
    // mirrored in the arch tp register: both slots are therefore
    // RE-DERIVABLE, and two compares per schedule are cheap. Heal
    // unconditionally; report the first hit per CPU.
    if cpu_id < MAX_CPUS {
        let static_idle = crate::sched::edges::idle_task_ptr(cpu_id);
        let pcpu = cpu_state_mut(cpu_id);
        if pcpu.idle != static_idle {
            static PCDATA_HIT: [core::sync::atomic::AtomicBool; MAX_CPUS] =
                [const { core::sync::atomic::AtomicBool::new(false) }; MAX_CPUS];
            if !PCDATA_HIT[cpu_id].swap(true, core::sync::atomic::Ordering::AcqRel) {
                r56_print_pid(
                    b"PERCPU-SCRIBBLE-HEALED cpu=",
                    cpu_id as u32,
                    b" idle_slot=",
                );
                r56_print_pid(b"", pcpu.idle as usize as u32, b"\n");
            }
            pcpu.idle = static_idle;
        }
        if pcpu.current.is_null() {
            let tp0 = crate::arch::cpu::get_thread_id() as *mut Task;
            if !tp0.is_null() {
                pcpu.current = tp0;
            }
        }
    }
    let mut prev = this_cpu().current;

    // R65 hunt (garbage-prev guard): the slot-resolved prev is dereferenced
    // below (pid, state, policy) and handed to the switch path, which
    // saves callee registers THROUGH it. A slot holding a non-Task pointer
    // (the x86-scribble3 cascade: publish with garbage next -> current slot
    // poisoned -> next __schedule runs on the poison) must be healed here,
    // before any dereference: fall back to the hardware truth (tp, the
    // arch per-CPU current_task) and resync the slot.
    #[cfg(feature = "x86_64")]
    if crate::trace::EVENT_SCRIBBLE.armed()
        && !crate::debug::scribble::task_plausible(prev)
    {
        let tp0: *mut Task = crate::arch::cpu::get_thread_id() as *mut Task;
        crate::debug::scribble::bad_switch_arg(0, prev as u64, tp0 as u64);
        if crate::debug::scribble::task_plausible(tp0) {
            this_cpu_mut().current = tp0;
            prev = tp0;
        } else {
            // No sane task identity left on this CPU — freeze for gdb
            // instead of switching through garbage.
            crate::debug::scribble::scribble_switch_park(prev as u64, 0);
        }
    }

    // R55 (multi-mode form-B): the slot-resolved prev must be the task the
    // hardware is actually executing (tp). A mismatch means ti_cpu was
    // steered mid-execution (every in-tree steer is gated, but out-of-tree
    // writes and residual windows remain — the multi-thread gate caught
    // pid 323 frozen at trap_exit with RUNNING+on_cpu+unlinked and the
    // slot still naming it). Healing here is one compare on the happy
    // path; on mismatch we find tp's true home slot, resync ti_cpu, and
    // proceed with the TRUE prev — so its dequeue/requeue bookkeeping
    // lands on the task that is actually leaving the CPU (the misdirect
    // shape that produced every phantom capture).
    let tp: *mut Task = crate::arch::cpu::get_thread_id() as *mut Task;
    if !tp.is_null() && tp != prev {
        // Scribble hunter tripwire: name the divergence the moment the
        // scheduler sees it (slot task vs the task the hardware runs).
        #[cfg(feature = "x86_64")]
        if crate::trace::EVENT_SCRIBBLE.armed() {
            crate::debug::scribble::slot_divergence(cpu_id, tp, prev);
        }
        // Locate the slot that still accounts tp (it was scheduled SOMEWHERE).
        let home = {
            let mut found = usize::MAX;
            for c in 0..crate::config::MAX_CPUS {
                if cpu_state(c).current == tp as *const Task as *mut Task {
                    found = c;
                    break;
                }
            }
            found
        };
        if home != usize::MAX {
            // Resync the poisoned identity: tp executes HERE. R85: stamp
            // the HOME slot's index, NOT cpu_id — cpu_id was derived from
            // the very ti_cpu we are healing (clamped to 0 when stale), so
            // stamping it back entrenches the divergence and books this
            // hart's schedules on another CPU's PER_CPU slot forever (two
            // harts fighting over slot 0: the tail-phase heal-storm shape).
            // The slot that names tp is the last pick's booking — the
            // physical hart tp executes on. When ti_cpu was already right,
            // home == cpu_id and this is a no-op.
            if home != cpu_id {
                // Rate-limited (a divergence fight can pass here hot).
                static LAST_RESYNC: core::sync::atomic::AtomicU64 =
                    core::sync::atomic::AtomicU64::new(0);
                let now = crate::timer::get_jiffies();
                let last = LAST_RESYNC.load(core::sync::atomic::Ordering::Acquire);
                if now.wrapping_sub(last) >= 2
                    && LAST_RESYNC
                        .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
                        .is_ok()
                {
                    r56_print_pid(
                        b"R55-RESYNC-STALE tcpu_was=",
                        crate::sched::edges::task_shared(tp).ti_cpu() as u32,
                        b" home=",
                    );
                    r56_print_pid(b"", home as u32, b"\n");
                }
            }
            // The sched:sched_migrate_task tracepoint for the heal path —
            // the stamp is about to move an existing task to its home CPU
            // (the reference records every set_task_cpu move).
            if crate::trace::EVENT_SCHED_MIGRATE_TASK.armed() {
                let tpr = crate::sched::edges::task_shared(tp);
                crate::trace::trace_sched_migrate_task(
                    tpr.comm(),
                    tpr.pid() as u32,
                    tpr.prio(),
                    tpr.ti_cpu(),
                    home as i32,
                );
            }
            crate::sched::edges::task_shared(tp).set_ti_cpu(home as i32);
            prev = tp;
        } else if (tp as usize) >= 0x80000000
            && crate::sched::edges::task_shared(tp).pid() == 0
            && !prev.is_null()
            && crate::sched::edges::task_shared(prev).pid() != 0
        {
            // R56d (slot-capture self-heal): tp is the idle task but this
            // CPU's slot names a REAL task — the pick that published the
            // slot never reached its context switch, and control fell back
            // into the idle loop. Every pass of that loop re-enters
            // __schedule here, resolves prev from the LYING slot, requeues
            // the orphan and re-picks it via the next == prev fast path
            // (which never restores its context) — a permanent busy wedge
            // in which the orphan is linked only inside GRQ-locked windows
            // and therefore invisible to every other CPU's pick. The slot
            // is this CPU's own (nobody else writes it under the GRQ except
            // the lying-slot heal, which stores this same idle task), so
            // resyncing it here is safe: prev becomes the idle task, the
            // idle loop marks itself idle, and the orphan — now slotless —
            // is harvestable through wake_up_enqueue's phantom path.
            this_cpu_mut().current = tp;
            prev = tp;
            slot_tag_write(cpu_id, SLOT_W_R56D, tp);
            // r11: the lying name's generation (if any was in flight) is
            // superseded by this resync.
            pick_gen_supersede(cpu_id, tp);
            const MSG: &[u8] = b"R56-SLOT-SELF-HEALED\n";
            for &b in MSG {
                crate::debug::taskdump::putc_brief(b);
            }
        }
        // home == MAX otherwise: tp has no slot at all.
        //
        // R85 (tail-phase PICK-SKIP wedge, the producer): a REAL, running
        // tp-task with no slot must be BOOKED as prev — the code below
        // would otherwise book the slot's task, switching out the WRONG
        // context while the physically-executing task keeps its on_cpu
        // mark, gets requeued by no one, and is skipped by every future
        // pick (the pid-1 tail wedge: linked ∧ {on_cpu=1, claim=X,
        // pk=1} forever, box idle). Linux parity: the current task is
        // what the hardware executes — its enqueue/dequeue bookkeeping
        // belongs to IT, and the slot is only a mirror that can lie (the
        // clamped-cpu_id divergence books PER_CPU[0] from a foreign
        // hart; heals and steals rewrite slots under us). Booking tp
        // makes the switch save/release the RIGHT context even when the
        // slot identity is poisoned; the misbooked slot heals through
        // the existing R56c/R56d/R85 machinery. The exit path's genuine
        // case (tp == the dying task mid-do_exit) has PF_EXITING set
        // and keeps the old behavior.
        //
        // R74 tripwire: name the shape (it is the orphan producer)
        // rate-limited.
        if home == usize::MAX && (tp as usize) >= 0x8000_0000_0000_0000 {
            use crate::process::task::task_flags::TaskFlags;
            let tpr = crate::sched::edges::task_shared(tp);
            let exiting =
                tpr.ti_flags() & TaskFlags::PF_EXITING.bits() != 0;
            let running = tpr.state().is_running();
            if !exiting && running {
                // R76: name the writer that last touched this CPU's slot
                // (tags: 1=pick 2=R56d 3=R56c 4=steal 5=abort-republish)
                // plus tp's own ownership word — the R74 family needs a
                // producer, not another symptom line.
                r56_print_pid(
                    b"R74-SLOTLESS-TP cpu=",
                    cpu_id as u32,
                    b" tp_pid=",
                );
                r56_print_pid(b"", tpr.pid(), b" slot_pid=");
                r56_print_pid(b"", crate::sched::edges::task_shared(prev).pid(), b" slotw=");
                r56_print_pid(
                    b"",
                    SLOT_WRITER[cpu_id].load(core::sync::atomic::Ordering::Relaxed),
                    b" tp_oncpu=",
                );
                let tpclaim = tpr.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
                r56_print_pid(b"", tpr.on_cpu() as u32, b" tp_claim=");
                if (tpclaim as i64) < 0 {
                    r56_print_pid(b"", (-tpclaim) as u32, b" tp_pk=");
                } else {
                    r56_print_pid(b"", tpclaim as u32, b" tp_pk=");
                }
                r56_print_pid(b"", tpr.pick_state(), b" ksp=0x");
                // R87 note: print the halves with a ':' separator. The old
                // separator-less concatenation of the two decimal prints
                // ("4294967254" + "31981568") reads as one number whose
                // natural 9/9-digit split decodes to hi32=0x19999995 —
                // the "ksp smear" ghost family (R83/R84 hunted a
                // corruption that never existed; every captured value,
                // re-split at the correct 10/8 digits, is a legal kernel
                // stack address).
                r56_print_pid(b"", (tpr.ti_kernel_sp() >> 32) as u32, b":");
                r56_print_pid(b"", (tpr.ti_kernel_sp() & 0xFFFF_FFFF) as u32, b"\n");
                // R85: book the physical truth — unconditional within this
                // block (tp != prev by the enclosing guard, tp is running,
                // canonical, and not exiting). The slot's task may be
                // legitimately running on ANOTHER hart; booking it here as
                // prev is exactly the double-book that strands tp. The
                // preempt-count guard below still refuses mid-critical-
                // section switches for tp.
                tpr.set_ti_cpu(cpu_id as i32);
                prev = tp;
            }
        }
    }

    if prev.is_null() {
        return;
    }

    // Prev is STABLE from here to the context switch (all rebooking
    // healed above). One INV-SCE-2 derivation serves every read below;
    // the raw `prev` stays the identity currency (comparisons, helpers).
    let prev_t = crate::sched::edges::task_shared(prev);
    let prev_pid = prev_t.pid();

    // Preempt discipline enforcement: a task holding preempt-disable state
    // (spinlock guards add PREEMPT_OFFSET) must never be switched out —
    // parking it mid-critical-section strands every lock it holds (the
    // jchurn wedge: an O_CREAT holder slept in the virtio used-ring wait
    // with VFS_MUTATION_LOCK + EXT4_BIG_LOCK held, and the CPUs spinning
    // on the mutation lock with preemption disabled could never
    // reschedule it back). Refuse the switch: restore RUNNING, unwind any
    // enqueue a racing wake may have done while we were in prepare_to_wait,
    // and return to the caller, whose wait loop re-checks its condition —
    // it degenerates into a poll loop, which is the correct behavior for a
    // lock holder. Terminal states (ZOMBIE/DEAD) are exempt: the exit
    // path's final schedule() must always switch away.
    //
    // LOST-PARK FIX (fix-lostpark lane, 2026-11-01): the refusal used to
    // trigger on ANY nonzero count — including counts whose ONLY set bits
    // are the interrupt classes (HARDIRQ/SOFTIRQ/NMI offsets). Those bits
    // are phantom on a task attempting a VOLUNTARY sleep: no hardirq,
    // softirq or NMI handler can legally call schedule(), so a sleeper
    // presenting them carries a leaked bracket (the x86 tick path's
    // entry/exit imbalance producers; the same class WAKE-HEAL and
    // PARKED-HEAL already zero on woken/parked targets). Refusing the
    // park anyway turned the phantom into a permanent unpreemptible
    // kernel poll loop: with kernel-preemption off, nothing evicts the
    // spinner, and because the spinner never completes another wait-queue
    // event, its peer's one-shot wake tokens (returned by W1 when the
    // wake hit the spinner's RUNNING phase of the oscillation) are never
    // re-issued — the lmbench lat_sem / ping-pong SMP wedge (satisfiable
    // parked waiter + peer spinning with both "idle" counters frozen;
    // repro: run/lostpark-notes.md). The reference kernel treats this
    // shape as a BUG print ("scheduling while atomic") and SLEEPS THE
    // TASK ANYWAY — an IRQ-class bit must never prevent a voluntary park.
    // Adjudicate: on a voluntary sleep (INTERRUPTIBLE/UNINTERRUPTIBLE/
    // STOPPED — states only the task's own wait path writes), with the
    // spinlock nest (PREEMPT_MASK) and PREEMPT_ACTIVE both clean, zero
    // the phantom interrupt bits and allow the park. Every other shape
    // keeps the original refusal exactly as-is (lock holders, involuntary
    // switches, terminal states).
    //
    // NOTE (kernel-preemption round 2): the schedule() recursion guard is
    // TIF_IN_SCHED, a ti_flags bit — NOT a preempt_count bit — so this
    // check needs no masking. (Round 1 used a PREEMPT_ACTIVE count bit;
    // leaked count bits here would make sleepers unswitchable — see the
    // sc65/sc66 one-activation monopolies.)
    {
        let prev_state = prev_t.state();
        if !prev_state.is_dead() && prev_t.preempt_count() != 0 {
            // Adjudicate under a brief IRQ-off window so a tick landing
            // between the read and the zero cannot erase an in-flight
            // balanced bracket (its un-paired sub would underflow the
            // count negative — a new phantom of the opposite sign).
            let flags = crate::arch::cpu::save_and_disable_irq();
            let pc = prev_t.preempt_count();
            let irq_class = pc
                & (crate::interrupt::preempt::HARDIRQ_MASK
                    | crate::interrupt::preempt::SOFTIRQ_MASK
                    | crate::interrupt::preempt::NMI_MASK);
            let holdout = pc & !irq_class;
            let voluntary_sleep = prev_state.is_sleeping()
                || prev_state.contains(TaskState::STOPPED);
            // r-strand round 4: a LIVE hardirq bracket on this cpu makes
            // the IRQ-class bits LEGAL nesting, not phantoms — the
            // exemption below would zero a real bracket's count, its
            // paired irq_exit sub then underflow-clamps or re-leaks on
            // the parked sleeper (the 1k-round livelock's marks-drift
            // producer). The reference never parks inside an interrupt:
            // its IRQ nesting count is per-CPU and irq_exit runs it to
            // zero before any sleeping context resumes. Refuse the park
            // while the bracket is live; the interrupted wait loop
            // retries its schedule once the bracket exits (the count is
            // then clean and the phantom branch below applies).
            let live_bracket = crate::interrupt::preempt::hardirq_depth_live(cpu_id);
            if !voluntary_sleep || holdout != 0 || live_bracket {
                // r10 (lmbench strand handoff): a VOLUNTARY sleeper with
                // a real preempt-class holdout is the definitive leak
                // moment — its wait loop now degenerates into an
                // unpreemptible poll, its slot keeps naming it (curr_on)
                // and every later wake is refused (why=c). Name the
                // outermost acquire site of that holdout directly
                // (rux:leakhunt (retired debug=leakhunt)-gated; IRQs are off but the report is
                // direct-console, GRQ NOT held here — printk would also
                // work, keep it raw for the IRQ-off window).
                if voluntary_sleep && holdout != 0 {
                    crate::debug::leakorigin::report_outermost(
                        b"PARK-REFUSED-ORIGIN",
                        prev,
                    );
                }
                crate::arch::cpu::restore_irq(flags);
                prev_t.set_state(TaskState::new(TaskState::RUNNING));
                crate::sched::dequeue_if_enqueued(prev_t);
                return;
            }
            // Phantom interrupt bits on a voluntary sleeper: same
            // medicine WAKE-HEAL administers (full zero — the holdout is
            // clean, so the whole count IS the phantom).
            prev_t.ti_preempt_count_zero();
            crate::arch::cpu::restore_irq(flags);
            static LPP_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if crate::trace::EVENT_LOSTPARK.armed() && LPP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 120
            {
                use crate::debug::taskdump::putc_brief as putchar;
                fn pdec3(mut v: u64) {
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
                const M: &[u8] = b"\nLP-PARK-IRQBITS-ZEROED pid=";
                for &b in M {
                    putchar(b);
                }
                pdec3(prev_t.pid() as u64);
                const M2: &[u8] = b" was_pc=0x";
                for &b in M2 {
                    putchar(b);
                }
                pdec3(pc as u32 as u64);
                putchar(b'\n');
            }
        }
    }

    // Fast path: if current task is idle and no tasks are runnable,
    // skip the GRQ lock entirely.  This avoids idle CPUs spinning on
    // the lock with IRQs disabled (via lock_irqsave), which would
    // prevent timer ticks from updating the soft-lockup timestamp.
    if prev_pid == 0 {
        let nr = GlobalRunQueue::grq_nr_running();
        if nr == 0 {
            // RT-2X item-7 hunt: the pinned-RT wedge shape — cpu0 idle
            // forever with runnable RT tasks — convicts this branch when
            // the counter desyncs from actual class linkage. Tripwire
            // (cmdline rt2xdbg=1, 32-line budget): names the gap live.
            if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
                let grq = grq();
                let rt_nr = grq.rt_rq.nr_running() as u64;
                let rr_nr = grq.rt_rq.rr_nr_running.load(core::sync::atomic::Ordering::Relaxed) as u64;
                let cfs_linked = {
                    // count() requires the lock; approximate via the
                    // timeline's own atomic size if exposed — otherwise
                    // report -1 (the RT rows alone decide the wedge).
                    u64::MAX
                };
                if rt_nr != 0 || rr_nr != 0 {
                    // Rule 1 (grq-lock-1st): event ring, not the
                    // console (pid = cpu, a packs rt|rr counts, b = the
                    // CFS linked bitmap witness).
                    crate::sched::sched_hooks::sched_ev(
                        crate::sched::sched_hooks::EV_NR_RUNNING_DESYNC,
                        crate::arch::cpu_id() as u32,
                        ((rt_nr as u64) << 32) | rr_nr as u64,
                        cfs_linked,
                    );
                }
            }
            return;
        }
    }

    // grq-hold-slim Cut 1 (bookkeeping out of the lock): charge prev's
    // own CFS entity BEFORE the GRQ acquire. This is exactly the
    // non-slot-owner arm of the old in-lock charge (R20-6's "everyone
    // else charges its own prev directly"): it touches ONLY prev's
    // per-entity atomics. prev is unlinked here (picked tasks leave the
    // timeline at their pick; a still-RUNNING prev is re-linked later
    // in this same section, a sleeping prev is dequeued there), so no
    // timeline reader can observe a mid-charge vruntime. A concurrent
    // waker on another CPU CAN re-link a sleeping prev mid-charge, but
    // (add_vruntime RMW | set_vruntime placement store) serialize
    // atomically and each order is a defensible outcome — the waker's
    // placement behavior is unchanged from the old in-lock charge
    // (a waker that won the same window there also skipped the lag
    // capture, which only the sleep-dequeue path takes).
    //
    // IRQ bracket: update_exec_runtime is NOT atomic (load ->
    // fetch_add -> store of exec_start). The only other chargers of
    // prev's entity run on THIS CPU (scheduler_tick / yield_cpu, both
    // under the GRQ); schedule() is entered with IF=1 on every
    // non-IRQ-exit path, so without the bracket a timer tick landing
    // inside that window double-charges the accumulated delta (the
    // tick adds E->now_tick, we add E->now). The cli/sti pair runs
    // OUTSIDE the measured GRQ hold — it costs the pre-segment, not
    // the critical section. The slot-owner arm (update_curr — the
    // GRQ-shared min_vruntime/avg-vruntime base) stays under the lock:
    // after this pre-lock charge stored exec_start, update_curr's
    // delta is precisely the acquire-wait residue, so the owner's
    // charge remains complete and min_vruntime still refreshes
    // in-lock exactly as before.
    let prev_policy_pre = prev_t.policy();
    if prev_policy_pre == SchedPolicy::Normal
        || prev_policy_pre == SchedPolicy::Batch
        || prev_policy_pre == SchedPolicy::Idle
    {
        let irq_state = crate::arch::cpu::save_and_disable_irq();
        let now = crate::sched::entities::sched_clock();
        let se = prev_t.sched_entity();
        let delta = se.update_exec_runtime(now);
        if delta > 0 {
            se.update_vruntime(delta);
        }
        crate::arch::cpu::restore_irq(irq_state);
    }

    // Lock the global RQ
    let cc_t1 = crate::debug::cctx::now();
    crate::debug::cctx::add(&crate::debug::cctx::SCH_PRE_NS, cc_t0);
    sched_cs_stamp(cpu_id, 1);
    let mut grq_guard = grq().lock_irqsave_site(grq_site(GRQ_CALLER_SCHED, GRQ_STEP_TRY));
    sched_cs_stamp(cpu_id, 2);
    // grq-lock-1st (debug-grq-wedge): the reproducible-wedge injection. The
    // cpu takes the GRQ and spins INSIDE the critical section — the
    // exact shape of the OH wedge (a holder that never releases, IRQs
    // off). Every other cpu then spins on the lock and the deadlock
    // detector fires; the crime-scene dump names this holder's site and
    // walks its stack.
    #[cfg(feature = "debug-grq-wedge")]
    if grq_wedge_inject() {
        grq_site_mark_fp_at(cpu_id, grq_site(GRQ_CALLER_SCHED, GRQ_STEP_PICK));
        grq_raw_puts(b"\nGRQ-WEDGE-INJECT: cpu=");
        grq_raw_putdec(cpu_id as u64);
        grq_raw_puts(b" holding the GRQ forever\n");
        loop {
            core::hint::spin_loop();
        }
    }
    let cc_t2 = crate::debug::cctx::now();
    crate::debug::cctx::add(&crate::debug::cctx::SCH_LOCKWAIT_NS, cc_t1);
    // grq-10ns: the EXACT GRQ critical section — one bracket from
    // acquisition to the unlock below (the acct/deq/pick/pub phase rows
    // remain, this row answers "how long is the lock held").
    let cc_hold = crate::debug::cctx::now();

    // Update runtime accounting for current task based on its scheduling class.
    // CFS: update vruntime; DL: consume runtime budget.
    // grq-hold-slim Cut 1: prev's OWN charge ran PRE-LOCK (see the
    // block above the acquire) for owner and non-owner alike — the
    // entity's runtime/vruntime are per-task atomics, and the tick and
    // yield paths still charge through update_curr under this same GRQ.
    // What remains in-lock for CFS is only the GRQ-SHARED state:
    // min_vruntime, refreshed here when no later step of this hold
    // refreshes it (the voluntary-sleep path unlinks nothing — prev was
    // unlinked at its pick — so neither the dequeue nor the requeue
    // arm's update_min_vruntime runs; the requeue path's enqueue_inner
    // and the dequeue path both refresh it themselves). The dropped
    // residue charge is the acquire-wait only (sub-us, vs ms-scale
    // slices); sum_exec_runtime/vruntime remain exact to within that
    // bound, and avg_vruntime() folds curr's live vruntime at read
    // time, so the pre-lock charge is fully visible to placement.
    //
    // This early read of prev_running (pre-nanosleep-rescue) serves
    // ONLY the min_vruntime condition: the nanosleep-loss park gate
    // below re-reads state under this same hold and may flip a timed
    // sleeper back to RUNNING — in that rescued case the requeue's
    // enqueue_inner refreshes min_vruntime itself, so acting on the
    // pre-rescue value here is at most a redundant refresh, never a
    // missed one.
    let prev_running = prev_t.state() == TaskState::new(TaskState::RUNNING);
    let prev_policy = prev_t.policy();
    if prev_policy == SchedPolicy::Normal
        || prev_policy == SchedPolicy::Batch
        || prev_policy == SchedPolicy::Idle
    {
        // R20-6: cfs_rq.curr is a SINGLE global slot while several CPUs
        // can run CFS tasks concurrently — the old unconditional
        // update_curr() charged the slot's task once per running CPU
        // (double charge) and never charged the other running tasks at
        // all (frozen vruntime → permanent leftmost hog). Only the slot
        // owner touches GRQ-shared CFS state here; everyone else's
        // charge was purely per-entity (pre-lock).
        if grq_guard.cfs_rq.get_curr() == prev && !prev_running {
            grq_guard.cfs_rq.update_min_vruntime();
        }
    } else if prev_policy == SchedPolicy::Deadline {
        // Update DL runtime accounting; on exhaustion THROTTLE prev so the
        // requeue below is refused cleanly (prev goes out like a sleeper;
        // the replenish timer re-enqueues it at the deadline — batch 8 CBS).
        let dl = prev_t.dl_entity();
        let now = crate::sched::entities::sched_clock();
        let exec_start = dl.exec_start.load(core::sync::atomic::Ordering::Acquire);
        if exec_start != 0 && now > exec_start {
            let delta = now - exec_start;
            if !dl.consume_runtime(delta) {
                dl_throttle(crate::sched::edges::task_mut(prev));
            }
        }
        dl.exec_start.store(now, core::sync::atomic::Ordering::Release);
    }

    // Deactivate prev: dequeue from the correct class-specific runqueue.
    let cc_acct = crate::debug::cctx::now();
    crate::debug::cctx::add(&crate::debug::cctx::SCH_ACCT_NS, cc_t2);
    let mut prev_running = prev_t.state() == TaskState::new(TaskState::RUNNING);
    // nanosleep-loss fix (reference shape): a timed sleeper parks ONLY
    // with a live waker — hrtimer_nanosleep's
    //     set_current_state(TASK_INTERRUPTIBLE);
    //     if (likely(t.task)) ret = schedule();
    // where the callback (or its cancellation) clears t.task BEFORE the
    // sleeper can schedule on it; a consumed sleeper never sleeps. Here
    // the equivalent adjudication runs at the park decision itself, under
    // this GRQ hold (the same lock the wake path's state gate and enqueue
    // serialize on): a task carrying a timed-sleep shadow whose wheel
    // entry is GONE (fired — its wake was delivered into the RUNNING
    // phase of the park protocol and consumed as the designed-benign
    // redundant class, or otherwise spent) must NOT park — it has no
    // waker left. Flip it back to RUNNING so the requeue branch below
    // keeps it runnable; its wait loop re-checks the condition (now
    // past-deadline) and re-arms through the guard's belt-and-braces
    // path. Lock order note: this introduces GRQ -> TIMER_TABLE
    // (timer_pending's lock); no path holds TIMER_TABLE and takes GRQ
    // (delivery happens after the table lock is released — R12-3), so
    // the order is safe.
    if !prev_running && prev_pid != 0 {
        let kind = prev_t
            .timed_sleep_kind
            .load(core::sync::atomic::Ordering::Relaxed);
        if kind != crate::timer::TSLEEP_NONE {
            let id = prev_t
                .timed_sleep_timer_id
                .load(core::sync::atomic::Ordering::Relaxed);
            // grq-lock-1st: checkpoint before the TIMER_TABLE lock (the
            // one nested lock the schedule path takes under the GRQ).
            grq_site_mark_fp_at(cpu_id, grq_site(GRQ_CALLER_SCHED, GRQ_STEP_NESTED_LOCK));
            if id != 0 && !crate::timer::timer_pending(id) {
                // R13 (oh-launch round 13, the pid-312 strand): the rescue
                // may flip ONLY a task that actually went to sleep
                // voluntarily — a SLEEPING prev. The terminal deschedule
                // path (schedule_dead_never_returns) calls schedule() with
                // STALE timer ids/kinds left over from the dying task's
                // earlier timed sleeps, and the unconditional
                // set_state(RUNNING) here kept flipping the parked-dying
                // task DEAD -> RUNNING on EVERY retry round (fr11:
                // TERM-DESCHED stuck pid=312 rounds=N state=0x1 pacing
                // 13+ seconds under the abort storm; every relink screen
                // then legitimately treats it as a live RUNNING task —
                // the harvest, the phantom sweep, the picks). A DEAD or
                // otherwise non-sleeping prev is never "rescued" from a
                // sleep it is not in; clear the stale id so the dead leg
                // does not re-evaluate every round.
                if prev_t.state().is_sleeping() {
                    // SAFETY: prev is this CPU's current task.
                    prev_t.set_state(TaskState::new(TaskState::RUNNING));
                    prev_running = true;
                    NSLPARK_RESCUED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                } else {
                    // SAFETY: prev is this CPU's current task; clearing its
                    // own stale sleep-timer record.
                    prev_t
                        .timed_sleep_timer_id
                        .store(0, core::sync::atomic::Ordering::Relaxed);
                    prev_t
                        .timed_sleep_kind
                        .store(crate::timer::TSLEEP_NONE, core::sync::atomic::Ordering::Relaxed);
                }
            }
        }
    }
    if !prev_running && prev_pid != 0 {
        pick_trace(prev, 10);
        match prev_policy {
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
        // grq-lock-1st: checkpoint before the prev dequeue (class op).
        grq_site_mark_at(cpu_id, grq_site(GRQ_CALLER_SCHED, GRQ_STEP_CLASS_OP));
                if grq_guard.cfs_rq.dequeue(prev) {
                    ww_record(prev, WW_EV_UNLINK_SCHED);
                } else {
                    // 64th (update_entity_lag parity): prev was PICKED
                    // (unlinked) and now sleeps — the reference captures
                    // the clamped lag here (dequeue_entity on curr,
                    // fair.c:5534); the next WAKE places at
                    // avg_vruntime − vlag.
                    grq_guard.cfs_rq.capture_sleep_vlag(prev);
                }
                // R8-1: unconditional — see dequeue_task() fix.
                if grq_guard.cfs_rq.get_curr() == prev {
                    grq_guard.cfs_rq.set_curr(core::ptr::null_mut());
                }
            }
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                grq_guard.rt_rq.dequeue(prev);
            }
            SchedPolicy::Deadline => {
                grq_guard.dl_rq.dequeue(prev);
            }
        }
    }

    // Re-enqueue prev if still runnable and not idle
    if prev_running && prev_pid != 0 {
        // R41: prev MUST leave this section linked on a class queue (or be
        // verifiably already linked) — a refused insert here is the B-form
        // phantom (RUNNING, unlinked, never picked; see requeue_prev_locked).
        // The old plain enqueue_task_locked call trusted the class on_rq
        // guard; a stale-true flag dropped prev permanently.
        // grq-lock-1st: checkpoint before the requeue (the class enqueue
        // is the one GRQ operation that can ALLOCATE — the deadline queue
        // is still a BTreeMap).
        grq_site_mark_fp_at(cpu_id, grq_site(GRQ_CALLER_SCHED, GRQ_STEP_CLASS_OP));
        requeue_prev_locked(&mut *grq_guard, crate::sched::edges::task_mut(prev));
        // timer-gap lane (pinned-RT slot-corruption fix, the pi_stress
        // -g 2 wedge): prev is leaving THIS cpu and its affinity no
        // longer includes it — the sched_setaffinity self-migration
        // shape. The affinity syscall's own resched_cpu(target) fires
        // BEFORE this park: the target cpu's single IPI-driven pick can
        // race this cpu's in-flight switch-out (ownership bits still
        // held), re-pick its incumbent, clear NEED_RESCHED — and no
        // later event ever pokes again (a LINKED RT task gets dispatch
        // only at wake-enqueue; FIFO has no tick preemption). prev then
        // sits pickable-but-never-picked on the only cpu its mask
        // allows (pi_min: FIFO-85 pinned to cpu0 behind a FIFO-80
        // spinner for 3 s, WAKE-STRANDED, RT-PICK-SKIP both cpus).
        //
        // The reference ties the dispatch to the migration enqueue:
        // set_cpus_allowed_ptr → move_queued_task → enqueue + check_
        // preempt_curr(dst_rq) under the rq lock — the poke is atomic
        // with the re-link, so the target either consumes it after the
        // link or its own pick (same lock) sees the task. Mirror that
        // HERE, at prev's re-link, still under this GRQ hold: claim an
        // idle allowed cpu, else run the class wakeup-preemption
        // against every allowed cpu's incumbent (resched_cpu is
        // lock-free — the established under-GRQ IPI shape of
        // enqueue_task's check_rt_preempt).
        let prev_mask = prev_t.cpus_allowed();
        if prev_mask & (1u32 << cpu_id) == 0 {
            // Rule 1 (grq-lock-1st): the opt-in rt2xdbg census — event
            // ring, not the console (a = the affinity mask driving the
            // migration dispatch).
            if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
                crate::sched::sched_hooks::sched_ev(
                    crate::sched::sched_hooks::EV_RQD_MIGRATE,
                    prev_pid,
                    prev_mask as u64,
                    0,
                );
            }
            match crate::sched::claim_idle_cpu_for(prev_mask) {
                Some(idle_cpu) => resched_cpu(idle_cpu),
                None => match prev_t.policy() {
                    SchedPolicy::Fifo | SchedPolicy::Rr => {
                        // No placement yet for a migrating prev (its old
                        // cpu is excluded from the mask) — affinity scan.
                        check_rt_preempt(prev, prev_mask, -1)
                    }
                    SchedPolicy::Deadline => {
                        check_dl_preempt(prev, prev_mask, -1)
                    }
                    _ => check_cfs_preempt(prev, prev_mask),
                },
            }
        }
    }
    let cc_t3 = crate::debug::cctx::now();
    crate::debug::cctx::add(&crate::debug::cctx::SCH_DEQREQ_NS, cc_acct);

    // Pick next task (R8-1b: prev is passed so the switching CPU may
    // re-pick itself through the next == prev fast path).
    // grq-lock-1st: checkpoint before the pick (the pick walks the
    // class trees; a corrupt tree can wedge the walk).
    grq_site_mark_fp_at(cpu_id, grq_site(GRQ_CALLER_SCHED, GRQ_STEP_PICK));
    let next = pick_next_task(&mut *grq_guard, cpu_id, prev);
    let cc_t4 = crate::debug::cctx::now();
    crate::debug::cctx::add(&crate::debug::cctx::SCH_PICK_NS, cc_t3);

    // Capture next_pid while we still hold references (before unlock)
    let next_pid = if !next.is_null() {
        crate::sched::edges::task_shared(next).pid()
    } else {
        0
    };

    // R63 + stress-port r8: publish the switch-out ownership BEFORE the
    // per-CPU slot flips to next. The marker makes every mid-switch
    // observer (parkwalk census, heals, pick ownership) treat prev as
    // still-owned from the first instruction after the pick until
    // __switch_to's release store lands. Storing it after the slot flip
    // left a gap — slot already names next, marker not yet set — in
    // which a racy census walk on another CPU classified a parking prev
    // (sleeping state set, its own GRQ guard still in hand) as "parked
    // with a live count": stress-port round 7's PARKED-WILDWRITE /
    // PARKED-LEAK datum (pc=1, LEAK-ORIGIN sched.rs:2641 = prev's OWN
    // scheduler acquire, self-resolved within the jiffy). The reference
    // kernel's prev->on_cpu covers the whole switch — prev stays owned
    // from its pick until finish_task_switch clears it — so publishing
    // earlier is strictly closer to that shape. The original R63 waker
    // invariant is preserved (the marker still lands inside this GRQ
    // hold, before any GRQ-serialized observer can act) and only
    // widens the protection window.
    let switching = next != prev && !next.is_null();
    if switching {
        SWITCHING_OUT[cpu_id].store(prev as u64, core::sync::atomic::Ordering::Release);
        // glib-mutex lane: stamp the marker's birth (under the same GRQ
        // hold) so the WAKE-HEAL adjudicates staleness by MARKER AGE,
        // not by the sighting history of the target task.
        SWITCHING_OUT_JIF[cpu_id].store(
            crate::timer::get_jiffies(),
            core::sync::atomic::Ordering::Release,
        );
    }

    // Update per-CPU current under lock
        // grq-lock-1st: the holder-naming rig's checkpoint (publish).
        grq_site_mark_at(cpu_id, grq_site(GRQ_CALLER_SCHED, GRQ_STEP_PUBLISH));
    this_cpu_mut().current = next;
    if switching {
        crate::sched::sched::ww_record(next, WW_EV_PICK_PUB);
    }
    if !next.is_null() {
        pick_trace(next, if switching { 3 } else { 4 });
    }
    slot_tag_write(cpu_id, SLOT_W_SCHED_PICK, next);
    // r11: open this CPU's pick generation — retired when the picked
    // context actually resumes (post-context_switch / schedule_tail /
    // the repick fast path below).
    pick_gen_publish(cpu_id, next);

    // Clear idle bit since we're about to run something
    grq().clear_idle(cpu_id);

    // Per-task schedstat accounting (T7 perf-support slice,
    // /proc/[pid]/schedstat): one sched_clock read serves both edges,
    // gated by one relaxed load when off (the tracepoint discipline).
    // A same-task repick (next == prev, the CPU-bound tick case) is NOT
    // a new timeslice — Linux counts switches-to — so only an open wait
    // window closes in that case; the exec stamp keeps running.
    if enabled() {
        let now = crate::sched::entities::sched_clock();
        let same = next == prev;
        if !prev.is_null() && prev_t.pid() != 0 {
            if same {
                // Repick: close a wait window a wake may have opened,
                // keep the exec stamp.
                let ws = prev_t.schedstat_wait_start.swap(0, core::sync::atomic::Ordering::Relaxed);
                if ws != 0 && now > ws {
                    prev_t.schedstat_wait_ns.fetch_add(now - ws, core::sync::atomic::Ordering::Relaxed);
                }
            } else {
                // PUT: accumulate prev's on-cpu time since its pick.
                let stamp = prev_t.schedstat_exec_start.swap(0, core::sync::atomic::Ordering::Relaxed);
                if stamp != 0 && now > stamp {
                    prev_t.schedstat_runtime_ns.fetch_add(now - stamp, core::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        if !same && !next.is_null() {
            let next_t = crate::sched::edges::task_shared(next);
            if next_t.pid() != 0 {
                // PICK: a new timeslice, and the wait window closes.
                next_t
                    .schedstat_timeslices
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let ws = next_t
                    .schedstat_wait_start
                    .swap(0, core::sync::atomic::Ordering::Relaxed);
                if ws != 0 && now > ws {
                    next_t
                        .schedstat_wait_ns
                        .fetch_add(now - ws, core::sync::atomic::Ordering::Relaxed);
                }
                next_t
                    .schedstat_exec_start
                    .store(now, core::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    // R63 (marker stored above, before the slot flip — stress-port r8
    // moved it up; a waker serialized by this lock still sees it before
    // the GRQ drops, so wake_up_enqueue's phantom heal cannot clear
    // prev's on_cpu mark while prev's context is still being saved).

    // Release lock but keep IRQs disabled for context_switch
    let flags = grq_guard.unlock_irqretain();
    sched_cs_stamp(cpu_id, 3);
    crate::debug::cctx::add(&crate::debug::cctx::SCH_PUB_NS, cc_t4);
    // grq-10ns: close the exact GRQ-hold bracket (see the acquire site).
    crate::debug::cctx::add(&crate::debug::cctx::SCH_HOLD_NS, cc_hold);

    // grq-hold-slim Cut 1: the picked task's own entity bookkeeping,
    // relocated from pick_next_task's CFS arm (out of the GRQ hold).
    // Runs with IRQs off, before the switch / repick return — see the
    // comment at the pick arm for why nothing can observe these fields
    // earlier. slice math reads the GRQ counters WITHOUT the lock:
    // another CPU's enqueue in this window only perturbs the slice by
    // one in-flight task's weight, the same advisory noise the tick's
    // own slice recomputation already tolerates (sched_slice is not a
    // GRQ invariant — it is a per-activation advisory value).
    if !next.is_null() && next_pid != 0 {
        // IRQs-off switch window (INV-SCE-1 (b)): the picked task is
        // unpickable to every other CPU until it runs; its slice stamp
        // is exclusive here.
        let next_t = crate::sched::edges::task_mut(next);
        let np = next_t.policy();
        if np == SchedPolicy::Normal || np == SchedPolicy::Batch || np == SchedPolicy::Idle {
            let se = next_t.sched_entity();
            // R20-6: reset the execution clock at pick. Nothing else
            // resets exec_start, so after a sleep it still holds the
            // timestamp of the task's PREVIOUS run — the first
            // update_curr/direct charge after wakeup billed the entire
            // sleep duration into vruntime.
            se.set_exec_start(crate::sched::entities::sched_clock());
            // Batch 8: start a fresh activation window — scheduler_tick
            // judges preemption on sum_exec_runtime -
            // prev_sum_exec_runtime (current run length) vs the slice
            // (Linux check_preempt_tick).
            se.prev_sum_exec_runtime.store(
                se.sum_exec_runtime.load(core::sync::atomic::Ordering::Acquire),
                core::sync::atomic::Ordering::Release,
            );
            let slice_ns = grq().cfs_rq.sched_slice(se);
            let slice_ms = crate::sched::entities::sched_slice_to_ms(slice_ns);
            next_t.set_time_slice(slice_ms.max(1) as u32);
        }
    }

    if next == prev {
        // Same-task repick: no context switch happens, so the post-switch
        // notify processing below would be skipped. A pending deferred
        // exit notification from an EARLIER exiter on this CPU must not
        // wait for the next real switch. Guard: if the pending child is
        // prev itself (a racing wake re-enqueued the exiting task and it
        // re-picked itself), do NOT fire its own notify — prev is still
        // running here, and the deferred-delivery protocol exists
        // precisely to keep an unreaped parent from freeing a task that
        // is still on a CPU.
        // fg1 (oh-launch round 5): the notify fires with IRQs ON, AFTER
        // the repick bookkeeping — it used to run in the IRQs-off window
        // this path inherits from unlock_irqretain (the context_switch
        // discipline), and its wake chain (send_signal -> signal_wake_up
        // / wake_group_chldexit -> WaitQueueHead::wake_up -> wake_up_
        // enqueue) then held the CPU with interrupts DISABLED for as
        // long as the waitqueue lock was contended — 4.5+ s on the
        // wedged boots ("not servicing IPIs", the GRQ standoff's holder;
        // REMOTE-BT: schedule -> process_deferred_exit_pid -> wake_up).
        // The reference never wakes from the switcher with IRQs off
        // (wake_q; the post-switch tail runs enabled). Decide here,
        // deliver after enable_irq.
        let cpu = crate::arch::cpu_id() as usize;
        let mut fire_notify = false;
        if cpu < MAX_CPUS {
            let packed = DEFERRED_EXIT_NOTIFY[cpu].load(core::sync::atomic::Ordering::Relaxed);
            if packed != 0 {
                let (_parent, child) = unpack_notify(packed);
                if child != prev_pid {
                    fire_notify = true;
                }
            }
        }
        // Same-task repick: still return with IRQs ON (the
        // preempt_schedule_irq local_irq_enable() discipline — see the
        // SP-5 note at the post-context_switch enable above).
        // r11: the repick's generation is trivially materialized — prev
        // never stopped running.
        pick_gen_retire(cpu_id);
        sched_cs_stamp(cpu_id, 0);
        crate::arch::cpu::enable_irq();
        if fire_notify {
            process_deferred_exit_notify_cpu(cpu);
        }
        return;
    }

    // Note: do NOT capture the CPU before context_switch. __schedule's frame
    // after context_switch belongs to the task being switched IN (coroutine
    // semantics), so a pre-captured cpu_id would be the CPU where THAT task
    // was switched out previously — not this CPU. After the switch, tp
    // points at `next` whose ti_cpu was just set to the hardware CPU by
    // context_switch, so a fresh cpu_id() read is the exiting task's CPU.

    // SAFETY: Runnable tasks cannot be freed while still on a CPU or runqueue.
    // IRQs remain disabled on this CPU, preventing concurrent scheduling.
    if !next.is_null() {
        crate::debug::cctx::bump(&crate::debug::cctx::CTX_N);
        let next_t = crate::sched::edges::task_shared(next);
        // The sched:sched_switch tracepoint (recorded in
        // __schedule just before the switch). One `armed()` check when
        // off; the prev/next fields are marshalled only when on. IRQs
        // are off here, so the record path must not (and does not)
        // take locks.
        if crate::trace::EVENT_SCHED_SWITCH.armed() {
            // The idle task's comm is empty; the convention names it
            // swapper/N in the trace arguments.
            let prev_comm = if prev_t.pid() == 0 {
                swapper_comm(cpu_id)
            } else {
                *prev_t.comm()
            };
            let next_comm = if next_t.pid() == 0 {
                swapper_comm(cpu_id)
            } else {
                *next_t.comm()
            };
            let prev_words = crate::trace::comm_to_words(&prev_comm);
            let next_words = crate::trace::comm_to_words(&next_comm);
            crate::trace::record(
                &crate::trace::EVENT_SCHED_SWITCH,
                [
                    prev_words[0],
                    prev_words[1],
                    prev_t.pid() as u64,
                    prev_t.prio() as u64,
                    prev_t.state().bits() as u64,
                    next_words[0],
                    next_words[1],
                    next_t.pid() as u64,
                    next_t.prio() as u64,
                    0,
                ],
            );
        }
        context_switch(
            crate::sched::edges::task_mut(prev),
            crate::sched::edges::task_mut(next),
        );
    }

    // R63: the switch completed on this hardware CPU — prev's context is
    // fully saved and published; retire the ownership mark. (Runs in the
    // incoming task's coroutine frame, but on the SAME CPU, and cpu_id()
    // reads the per-CPU hardware slot, so it still names this CPU.)
    // R11 (the terminal-release window): this is the ONLY point with
    // positive knowledge that prev's final switch-out REALLY completed
    // (the marker retiring in the INCOMING context; an aborted switch
    // also reaches this tail, but there current() == prev — the aborted
    // prev keeps running — so the discriminator below stays false and a
    // resumed terminal loop is not marked done). Publish that for the
    // exit-path frees: schedule_dead_never_returns releases the
    // ownership pair BEFORE its schedule() call (the sc191 v2 fix), and
    // until this store the free-side waits alone cannot distinguish
    // "switch-out complete" from "released, switch not yet started" —
    // the freed-under-its-own-schedule-chain producer (rip=0x1 /
    // rip=heap / #GP-on-ret at exit-storm time).
    if switching {
        // P0 exit-reap (2026-11-23): retire the terminal-switch markers
        // for the task the PER-CPU MARKER names — not the frame-local
        // `prev`. In this kernel's coroutine switch model (both arch
        // twins: __switch_to freezes the outgoing frame at its own call
        // site and the resume re-enters the INCOMING task's old frame)
        // this tail executes either in the resumed task's own old
        // __schedule frame — whose frame-local `prev` is by construction
        // the task that parked through that frame, i.e. ALWAYS ==
        // current() — or, on a switch abort (.Lswitch_abort), in the
        // outgoing task's own frame where current() == prev again. The
        // old `current() != prev` arm was therefore unreachable for the
        // task that actually switched out: terminal_switch_done could
        // only ever land via the newborn schedule_tail site, and in the
        // fork+exit shape (child dies, the PARKED parent resumes — not a
        // newborn) it never landed at all. Measured on the pbench P4
        // storm (x86/KVM): 0 tsd-arm completions, every one of the 1024
        // drainer entries blocked on terminal_pacing forever, PENDING at
        // cap, every later reap on the 3 s synchronous fallback
        // (RTS-DEFER spam, battery never reaches DONE). The marker is
        // the one piece of per-CPU state that positively names the
        // outgoing task of THIS completed switch — published under the
        // GRQ before the slot flip, retired here in the incoming frame
        // after __switch_to's release store (the reference kernel's
        // finish_task_switch `last` shape).
        let cpu = crate::arch::cpu_id() as usize;
        let retired = SWITCHING_OUT[cpu].swap(0, core::sync::atomic::Ordering::AcqRel) as *mut Task;
        let cur = crate::sched::current().map(|c| c as *mut Task);
        if !retired.is_null() && cur != Some(retired) {
            // Discriminator: only a task whose terminal deschedule loop
            // was executing (pacing) takes the positive done marker —
            // and the pacing flag retires here with the switch (the
            // tail that set it has truly stopped executing; boot-63's
            // fix). A normal parked task must NOT carry a stale
            // tsd=true: a task preempted mid-exit-tail before its
            // terminal loop re-arms the flag would otherwise present
            // {dead-ish, !on_cpu, slotless, tsd=true} to the reap
            // drainer and free before its exit tail finished.
            let rt = crate::sched::edges::task_shared(retired);
            if rt
                .terminal_pacing
                .swap(false, core::sync::atomic::Ordering::AcqRel)
            {
                rt.terminal_switch_done
                    .store(true, core::sync::atomic::Ordering::Release);
            }
        }
    }
    // r11: the pick generation materialized — the picked context is
    // resuming right here (this frame's own switch-in). Retire the
    // CURRENT generation of this CPU; between the publish and this point
    // IRQs were off, so no other publish can have intervened (and the
    // unconditional current-gen store is monotonic-safe against a
    // concurrent heal-side supersede anyway).
    pick_gen_retire(crate::arch::cpu_id() as usize);

    // SP-5 round 5 (2026-11-03): re-enable interrupts IMMEDIATELY after the
    // context switch — the finish_task_switch() discipline (the reference
    // kernel's finish_lock_switch is spin_unlock_irq(&rq->lock): IRQs go
    // back ON the moment the switch completes, and ALL post-switch work —
    // mmdrop, put_task_struct, our deferred-exit notify below — runs at
    // IF=1). The old per-caller tail restore (R54) kept IRQs off through
    // this whole tail AND restored the INCOMING task's own saved flags —
    // which are 0 for every task that was switched out through the
    // trap-exit asm_schedule path (IRQ handlers run cli'd to their exit,
    // so the exit loop's resched schedule entered at IF=0). Resuming such
    // a task with flags=0 handed it back to its KERNEL continuation at
    // IF=0 — and under the SP-5 exhaustion ramp that continuation is a
    // direct-reclaim round: both CPUs ran hundreds of milliseconds with
    // IRQs off, the LAPIC timer IRQs piled up undelivered in IRR
    // (TICKTRAP: IF=false + irr7=0x29 + tmcct=0 on BOTH cpus, ticks
    // frozen at ~50, tb advancing), jiffies/watchdog/kill-windows died —
    // the whole SP-5 "dead clock" family. The reference's
    // preempt_schedule_irq() likewise returns from __schedule with
    // local_irq_enable() — schedule() NEVER hands a caller back at IF=0.
    // The x86 trap-exit tails are safe with schedule() returning IF=1:
    // both tails do their own unconditional `cli` immediately before the
    // register pops, and iretq/sysretq restore the frame's IF.
    crate::arch::cpu::enable_irq();

    // After context_switch, the NEW task is running.  The exiting task
    // (prev) is no longer on any CPU, so it is now safe to notify its
    // parent (SIGCHLD + wake_up).  The deferred notification was stored
    // in a per-CPU slot by do_exit → defer_exit_notify() on THIS CPU.
    process_deferred_exit_notify_cpu(crate::arch::cpu_id() as usize);

    // Tail return: IRQs ON unconditionally (see the SP-5 note above — the
    // preempt_schedule_irq local_irq_enable() discipline; per-caller
    // flag restoration is what exported the poison). `flags` stays bound
    // for the repick path's documentation trail.
    let _saved_flags = flags;
    sched_cs_stamp(crate::arch::cpu_id() as usize, 0);
    crate::arch::cpu::enable_irq();
}


// ==================== nanosleep-loss pick-phase tracer (hunt) ================
//
// Per-task milestone stamps across the pick→switch handoff, so a frozen
// task's last recorded phase names exactly where its pick died. Phases:
// 1 walk-unlinked   (fair.rs walk allowed-branch, post-unlink)
// 2 mark_picked     (publish done)
// 3 slot-published  (__schedule current=next)
// 4 repick-return   (__schedule next==prev fast path)
// 5 wrapper-enter   (context_switch entry)
// 6 affirm-ok       (try_affirm_pick Ok)
// 7 affirm-fail     (try_affirm_pick Err)
// 8 early-abort     (wrapper revoked-pick check)
// 9 asm-abort       (x86_switch_abort)
// a post-switch     (context_switch returned / new context)
// Side table keyed by task pointer (validated); 32 slots.


// ==================== pick→switch handoff ledger (wake-fastrepro) ====================
//
// ORPHAN-MARK hunt: exactly ONE site publishes the ownership qword
// (mark_picked_on_cpu) and exactly TWO terminations consume a pick (the
// switch-in affirm CAS and the switch-in abort). A pick whose
// publication has neither termination is a SWITCHLESS PICK — the R56
// slot-capture orphan at its creation moment. This side table (no Task
// layout change) records the last publish per task and is cleared by
// both terminations; the stale-claim heal reads it: an intact entry at
// heal time names the publisher CPU, the published pick-state and the
// publish's age — the creation interleaving on one line.




/// DEBUG diagnostic: authoritative CFS linked-state by pointer scan, used by
/// taskdump to distinguish flag-desync from really-off-queue phantoms.
///
/// The scan mirrors dequeue's own lookup, but the CFS `tasks_timeline`
/// BTreeMap may ONLY be iterated under the GRQ lock — including from
/// diagnostics. The old lockless scan ("a racing tree is acceptable for a
/// diagnostic") was unsound: std BTreeMap iteration concurrent with
/// structural insert/remove on another CPU walks nodes that are being
/// split/merged/freed, and the iterator's `next_unchecked` contract breaks
/// (observed: `navigate.rs` `next_kv().ok().unwrap()` panic under GLib
/// multi-threaded scheduling churn). The GRQ is therefore acquired with
/// try_lock semantics so a wedged holder can never wedge the diagnostic
/// itself; a busy lock reports linkage as unknown (None) and the dump
/// labels it, exactly like the skipped pid-hash buckets.
pub fn grq_diag_cfs_linked(task: *mut crate::process::task::Task) -> Option<bool> {
    if task.is_null() {
        return Some(false);
    }
    // SAFETY: GRQ is initialized before the first task exists; the scan
    // treats the pointer as a key comparison only and runs under the lock.
    let guard = grq().try_lock_irqsave()?;
    Some(guard.cfs_rq.is_linked(task))
}


/// DEBUG diagnostic (4-thread hang hunt): grq.nr_running (the global runnable
/// counter the idle fast path trusts) versus the per-class queue depths it is
/// supposed to mirror. A divergence pins the counter-drift engine in one shot.
pub fn grq_diag_counts() -> (usize, u64, u64, u64) {
    let g = grq();
    (
        g.nr_running.load(core::sync::atomic::Ordering::Relaxed),
        g.cfs_rq.nr_running(),
        g.rt_rq.nr_running() as u64,
        g.dl_rq.nr_running() as u64,
    )
}


/// 105th: idle-CPU bitmask snapshot for the same census (cheap Acquire
/// load; diagnostic only).
pub fn grq_idle_mask() -> u32 {
    grq().idle_cpus.load(core::sync::atomic::Ordering::Acquire)
}

/// 105th census twin: total runnable load (all classes).
pub fn grq_nr_running_now() -> usize {
    GlobalRunQueue::grq_nr_running()
}

/// DEBUG diagnostic: per-CPU current task pid (0 = idle) + idle bit + raw task
/// pointer, to see which CPU (if any) is executing/spinning while a queued
/// task goes unpicked. Hash-invisible currents (post-exit spinners) are
/// identifiable by their pointer not resolving in the dump body.
pub fn grq_diag_cpu_currents() -> [(
    u32,
    bool,
    *mut crate::process::task::Task,
); crate::config::MAX_CPUS] {
    let mut out = [(0u32, false, core::ptr::null_mut()); crate::config::MAX_CPUS];
    for c in 0..crate::config::MAX_CPUS {
        let st = cpu_state(c);
        let cur = st.current;
        out[c] = (
            if cur.is_null() {
                u32::MAX
            } else {
                // SAFETY: per-CPU current is null or a valid Task set by the
                // scheduler; read-only pid access for diagnostics.
                crate::sched::edges::task_shared(cur).pid()
            },
            g_diag_idle_bit(c),
            cur,
        );
    }
    out
}


// ==================== Scheduler Tick ====================

pub fn scheduler_tick() {
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    // P4 async-reap: advance the deferred task-storage ladder (one
    // CAS-guarded CPU per tick, budget-bounded, empty-list fast path
    // is one try_lock). Runs FIRST so a storm's reaps recycle their
    // stacks within the same tick cadence the forks consume them.
    crate::sched::reapdrain::reap_drain_tick();
    // grq-lock-1st: service the printk deferrals (the deferred kmsg
    // reader wake + one pending console line) and drain the Rule-1
    // scheduler event ring. All three run with NO scheduler lock held —
    // the tick is the designated safe servicer (printk is Rule-2-safe
    // from here: the ring write is lock-free and the console emit
    // defers).
    crate::printk::service_deferred_kmsg_wake();
    crate::sched::sched_hooks::sched_ev_drain(6);
    crate::debug::softlockup::touch(cpu_id);
    // Linux's schedstat sched_count: one scheduler invocation.
    crate::fs::perf_ctr::note_sched_run(cpu_id);
    // phantom-bits orphan scan: bits the interrupted task carried INTO
    // the tick whose release never executed (softirq bit outside any
    // open window / stranded hardirq bit). rux:leakhunt (retired debug=leakhunt)-gated inside.
    crate::interrupt::preempt::tick_orphan_scan(cpu_id);

    // Allocator class-lock holder watch (rux:allocwatch (retired debug=allocwatch)): scan ALL mm
    // stamp slots CROSS-CPU and name any holder frozen >1 s mid-section.
    // The holder sits at IF=0 (lock_irqsave), so its OWN CPU's tick is
    // dead — this check only ever fires from the OTHER CPU. The waiters
    // cannot self-report (they are parked IN the allocator, printing
    // allocates). See debug/allocwatch.rs.
    crate::debug::allocwatch::tick_check();

    // OH-launcher wall (2026-10-31): CLAIM-SWEEP — position-independent
    // stale-claim heal. The pick-side STALE-CLAIM-RESET only reaches a
    // victim a pick walk EXAMINES, and walks stop at the first allowed
    // leftmost candidate: a stale-claimed task mid-tree (frozen vruntime
    // at strike time) is skipped forever without ever being examined —
    // lb20's softbus_server lost 579s between its two resets, and its
    // worker threads were never swept at all. Once per 100 jiffies
    // (1 s), one CPU wins the CAS below and sweeps the whole CFS
    // timeline under the GRQ with the pick-side adjudication (see
    // CfsRunQueue::claim_sweep for the protocol-impossibility proof).
    // Every strike then costs at most ~1 s + normal CFS service latency
    // instead of a leftmost drift measured in hundreds of seconds —
    // inside the 5.24 s softbus timeout that gates the
    // device_manager/accountmgr chain. The census line is wwcensus-only;
    // the reset report shares the pick-side 2-jiffy print budget.
    {
        static SWEEP_LAST: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let now = crate::timer::get_jiffies();
        let last = SWEEP_LAST.load(core::sync::atomic::Ordering::Acquire);
        if now.wrapping_sub(last) >= 100
            && SWEEP_LAST
                .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
                .is_ok()
        {
            let g = grq().lock_irqsave();
            let (linked, stale_reset, owned_claim, oncpu_marked, oncpu_pids, owned_pids) =
                g.cfs_rq.claim_sweep(cpu_id);
            drop(g);
            if crate::trace::EVENT_WWCENSUS.armed()
                && (stale_reset > 0 || owned_claim > 0 || oncpu_marked > 0)
            {
        // HOTPATH-PRINT: switch-gated (cmdline/tracepoint arm above); zero cost when off.
                crate::pr_info!(
                    "CLAIM-CENSUS linked={} reset={} owned_claim={} oncpu={} oncpu_pids={:?} owned_pids={:?}",
                    linked,
                    stale_reset,
                    owned_claim,
                    oncpu_marked,
                    oncpu_pids,
                    owned_pids
                );
            }
        }
    }

    // Kernel-preemption round 2 (2026-10-20): LEAK tripwire. A task whose
    // preempt_count stays nonzero for seconds while it keeps RUNNING in
    // task context is carrying a leaked count bit — with IRQ-exit kernel
    // preemption such a leak means the task can never be preempted again,
    // and (worse) __schedule's switch-out discipline refuses to ever put
    // it to sleep: the 160-291 s one-activation monopolies of sc65/sc66
    // (strand incumbents showed pid 1 at pc=0x04000001/0x08000001).
    // Side-table keyed by Task pointer (wakewatch pattern; no Task layout
    // change): record when a nonzero count is first seen, report the
    // task + count after 2 s, clear when the count drains. Direct
    // console, rate-gated — this runs from the timer tick.
    {
        // Rotation-proof leak window (sc41, 2026-10-31): the window is
        // keyed by TASK, not by (cpu, task). The old per-CPU window
        // reset on every CPU switch, and the sc41 rotation storm —
        // several leak carriers cycling the CPUs every few ms (census:
        // activations of 0 ms, incumbents 319/564/601/1348 alternating,
        // ~27K picks/s) — kept it permanently reset: PREEMPT-HEAL fired
        // ZERO times in 380 s while carriers held base counts for the
        // whole boot and pid 1 stranded up to 48 s. A per-task window
        // survives migration and rotation, and the 200-observation bar
        // is CUMULATIVE across leak episodes (sc42: carriers whose
        // counts flip leak→drain→re-leak starve any consecutive-samples
        // bar exactly like the per-CPU window did) — every real guard
        // section is microseconds, so the bar keeps a five-order margin
        // under any continuity shape (a336568f's evidence).
        const LEAK_SLOTS: usize = 16;
        struct LeakWin {
            task: core::sync::atomic::AtomicU64,
            first: core::sync::atomic::AtomicU64,
            last: core::sync::atomic::AtomicU64,
        }
        static LEAK_WIN: [LeakWin; LEAK_SLOTS] = {
            #[allow(clippy::declare_interior_mutable_const)]
            const ZERO: LeakWin = LeakWin {
                task: core::sync::atomic::AtomicU64::new(0),
                first: core::sync::atomic::AtomicU64::new(0),
                last: core::sync::atomic::AtomicU64::new(0),
            };
            [ZERO; LEAK_SLOTS]
        };
        static LEAK_LAST_PRINT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        if cpu_id < crate::config::MAX_CPUS {
            let cur = crate::sched::sched::cpu_state(cpu_id).current;
            // freed-task lane: skip a lying slot — a freed/painted task
            // must not feed the leak window (see the TICK-GHOST gate at
            // this tick's tail for the capture story). One volatile pid
            // read; poison fails the compare.
            let cur = if !cur.is_null() && crate::sched::edges::task_pid_volatile(cur) == TASK_POISON {
                core::ptr::null_mut()
            } else {
                cur
            };
            if !cur.is_null() {
                // The probe samples from inside the tick hardirq, so the
                // sampled count legitimately carries THIS tick's own
                // HARDIRQ_OFFSET for whatever task is current — counting
                // it made every idle-cpu sample "hold nonzero pc" forever
                // (the PREEMPT-LEAK pc=0x10000 spam on idle tasks,
                // 2026-11-19: the reporter was measuring its own bracket).
                // Subtract exactly the tick's one level before any
                // adjudication; a task's own leaked hardirq level still
                // shows as a residue (and h_extra reads >= 1 level).
                let pc = crate::sched::edges::task_shared(cur).preempt_count()
                    .saturating_sub(crate::interrupt::preempt::HARDIRQ_OFFSET);
                // Per-field consecutive-PRESENT run counters (phantom-bits
                // r2/3, stress-port r3 2026-11-02): a field's phantomness
                // is proven only by ITS OWN observations — a field seen
                // PRESENT at 200 consecutive own-CPU ticks (with no
                // intervening legal drain, which resets the run from the
                // release path) is phantom; every legal bracket is
                // microseconds, so 2 s of observed presence is five orders
                // beyond any live hold. Counted BEFORE the pc != 0 gate so
                // an entirely-zero count re-arms every field. Three relaxed
                // ops per tick; only the task's own CPU writes these (the
                // release-side resets run in the task's own context, the
                // heal-side resets in the healing tick / parkwalk walk).
                //
                // stress-port r3 (replaces absence-stamp AGE): stamp age
                // had three false-fire producers, all observed on the
                // memory2/memory-hunt2 boots: (1) age accrued for free
                // while the task was off-CPU — a task parked mid-section
                // through the 100 s pick-skip strand resumed instantly
                // "stale" and its first tick healed a LIVE guard
                // (underflow at the page-allocator zone lock);
                // (2) jiffies is timebase-derived, so a >2 s host steal
                // window inflated age by 200+ ticks with zero ticks
                // delivered; (3) the pb3 release-side re-arm stored the
                // superseded ti_pc_zero_jif field, so the bar never
                // re-armed on legal drains and lock-hot tasks accumulated
                // stale bars whenever the 100 Hz sampler missed the
                // microsecond absent-gaps between guard pairs (the
                // held_jiffies=0 heal storm, oage_jif=1 on every line).
                // Observation COUNTING is immune to all three by
                // construction: no tick, no increment. Parked phantoms
                // stay owned by the parkwalk's 4 s PARKED-HEAL (no tick
                // needed).
                //
                // Arm-on-first-observation (stress-port r1) is inherent:
                // a fresh task's counters start at 0 and must climb to
                // 200 real observations before any heal.
                {
                    let h_extra_now = (pc & crate::interrupt::preempt::HARDIRQ_MASK)
                        >= crate::interrupt::preempt::HARDIRQ_OFFSET;
                    crate::sched::edges::task_shared(cur).ti_pc_present_runs_tick(
                            pc & crate::interrupt::preempt::PREEMPT_MASK != 0,
                            pc & crate::interrupt::preempt::SOFTIRQ_MASK != 0,
                            h_extra_now,
                        );
                }
                if pc != 0 {
                    let now = crate::timer::get_jiffies();
                    let task_word = cur as usize as u64;
                    // Find-or-claim this task's window slot: first slot
                    // naming it, else the first empty, else evict the
                    // least-recently observed. Claim races between two
                    // CPUs are benign — a lost claim delays one window.
                    let mut w = &LEAK_WIN[0];
                    let mut lru_last: u64 = u64::MAX;
                    for s in LEAK_WIN.iter() {
                        let t = s.task.load(core::sync::atomic::Ordering::Relaxed);
                        if t == task_word || t == 0 {
                            w = s;
                            break;
                        }
                        let l = s.last.load(core::sync::atomic::Ordering::Relaxed);
                        if l < lru_last {
                            lru_last = l;
                            w = s;
                        }
                    }
                    let same = w.task.load(core::sync::atomic::Ordering::Relaxed)
                        == task_word;
                    let mut t0 = if same {
                        w.first.load(core::sync::atomic::Ordering::Relaxed)
                    } else {
                        0
                    };
                    // Cumulative bar (sc42 refinement): the window
                    // accumulates across EPISODES, not consecutive
                    // samples — sc42 showed carrier 319's count flips
                    // (leak, drain via pick-zero, re-leak) with clean
                    // gaps long enough that any consecutive-samples bar
                    // starves exactly like the per-CPU window did, while
                    // each episode still stranded pid 1. The statistical
                    // argument survives the relaxation: every real guard
                    // section is microseconds, so 200 tick observations
                    // of this task holding a base count — under ANY
                    // continuity shape — is still five orders of
                    // magnitude beyond a legitimate holder, and a false
                    // heal's later guard drop underflow-clamps at zero
                    // and REPORTS (sub_preempt_count_at) instead of
                    // silently poisoning.
                    if t0 == 0 {
                        t0 = now;
                    }
                    // Per-field adjudication (phantom-bits round 3,
                    // supersedes BOTH bars): the sc42 cumulative window
                    // and d2381bf9's PREEMPT-pair re-arm were two wrong
                    // answers to the same question — "has THIS field been
                    // held continuously?" — asked through a shared
                    // window that any field's activity (or inactivity)
                    // answered for all of them. The cumulative bar
                    // false-healed live softirq windows and guards
                    // (make-test sgid_mode VW-WEDGE: the window's pending
                    // bits clear before its handlers run; a heal mid-
                    // window makes it preemptible, the handler never
                    // finishes, 29 completions lost, both CPUs idle
                    // forever — 3112b576 passes the same boot, the only
                    // delta is the heal policy); the pair-based re-arm
                    // shielded stranded S/H bits on lock-hot carriers
                    // (lb61-lb66: ZERO heals, every CPU monopolized).
                    // The run counters above answer the question per
                    // field: run >= 200 = present at 200 consecutive
                    // own-CPU tick observations with no legal drain
                    // (release-side reset) in between.
                    let s_set = pc & crate::interrupt::preempt::SOFTIRQ_MASK != 0;
                    let p_set = pc & crate::interrupt::preempt::PREEMPT_MASK != 0;
                    let h_extra =
                        (pc & crate::interrupt::preempt::HARDIRQ_MASK) >=
                            2 * crate::interrupt::preempt::HARDIRQ_OFFSET;
                    // stress-port r3: the bar counts OBSERVATIONS, not
                    // jiffies — see the counter block above for the three
                    // defeated age-producer families. run == 0 ("younger
                    // than one observation") is inherently not stale.
                    let stale_200 = |run: u64| run >= 200;
                    let p_stale = p_set && {
                        stale_200(crate::sched::edges::task_shared(cur).ti_pc_pirq_present_run())
                    };
                    let s_stale = s_set && {
                        stale_200(crate::sched::edges::task_shared(cur).ti_pc_sirq_present_run())
                    };
                    let h_stale = h_extra && {
                        stale_200(crate::sched::edges::task_shared(cur).ti_pc_hirq_present_run())
                    };
                    w.task.store(task_word, core::sync::atomic::Ordering::Relaxed);
                    w.first.store(t0, core::sync::atomic::Ordering::Relaxed);
                    w.last.store(now, core::sync::atomic::Ordering::Relaxed);
                    if t0 != 0
                            && now.wrapping_sub(t0) >= 200
                            // Serial-noise policy: the periodic PREEMPT-LEAK
                            // reporter is a hunt instrument (rux:wwcensus (retired debug=wwcensus)),
                            // not default output — idle-task word residue on
                            // quiet boots made the serial unusable.
                            && crate::trace::EVENT_WWCENSUS.armed()
                        {
                            let lp = LEAK_LAST_PRINT.load(
                                core::sync::atomic::Ordering::Acquire);
                            if now.wrapping_sub(lp) >= 20
                                && LEAK_LAST_PRINT
                                    .compare_exchange(
                                        lp,
                                        now,
                                        core::sync::atomic::Ordering::AcqRel,
                                        core::sync::atomic::Ordering::Acquire)
                                    .is_ok()
                            {
                                use crate::debug::taskdump::putc_brief as putchar;
                                fn ph(v: u64) {
                                    use crate::debug::taskdump::putc_brief as putchar;
                                    for sh in (0..16).rev() {
                                        let n = ((v >> (sh * 4)) & 0xF) as u8;
                                        putchar(if n < 10 {
                                            b'0' + n
                                        } else {
                                            b'a' + n - 10
                                        });
                                    }
                                }
                                const M: &[u8] = b"\nPREEMPT-LEAK cpu=";
                                for &b in M {
                                    putchar(b);
                                }
                                ph(cpu_id as u64);
                                const M2: &[u8] = b" pid=";
                                for &b in M2 {
                                    putchar(b);
                                }
                                ph(crate::sched::edges::task_shared(cur).pid() as u64);
                                const M3: &[u8] = b" pc=";
                                for &b in M3 {
                                    putchar(b);
                                }
                                ph(pc as u32 as u64);
                                const M4: &[u8] =
                                    b" held_nonzero_for=";
                                for &b in M4 {
                                    putchar(b);
                                }
                                ph(now.wrapping_sub(t0));
                                const M5: &[u8] = b"jiffies rip=";
                                for &b in M5 {
                                    putchar(b);
                                }
                                // The interrupted kernel instruction at
                                // tick time: a leaked-count task poll-
                                // loops in the wait whose schedule()
                                // keeps being refused — this RIP names
                                // that loop directly (symbolize with
                                // addr2line).
                                {
                                    use crate::arch::pt_regs::PtRegs;
                                    let pr = crate::arch::trap::current_pt_regs()
                                        as *const PtRegs;
                                    if !pr.is_null() {
                                        ph(crate::sched::edges::current_trap_ip());
                                    }
                                }
                                // pctick: the interrupted task's own count
                                // at tick ENTRY (snapshot taken in the
                                // timer trap path before irq_enter books
                                // the tick's offset) — 0x1 means the
                                // reportable bit is a leaked PREEMPT
                                // disable; >=0x10000 means a hardirq bit
                                // was already stranded on the task.
                                const M6: &[u8] = b" pctick=";
                                for &b in M6 {
                                    putchar(b);
                                }
                                ph(crate::interrupt::preempt::TICK_ENTRY_PC
                                    [cpu_id]
                                    .load(core::sync::atomic::Ordering::Relaxed)
                                    as u64);
                                putchar(b'\n');
                                // rux:leakhunt (retired debug=leakhunt): the acquire site of the
                                // leaked count (last PREEMPT_OFFSET
                                // acquire on the incumbent).
                                crate::debug::leakorigin::report_incumbent_site(cur);
                            }
                        }
                        // PREEMPT-HEAL (round 3c, bound 10 s -> 2 s by
                        // a336568f, rotation-proofed by sc41): a
                        // TASK-context base count held for 2 s
                        // continuously is a phantom — every real guard
                        // section is microseconds, and the only
                        // multi-second holds in task context are the
                        // unsleepable-monopoly wedges themselves. The
                        // sc41 rotation storm proved the per-CPU window
                        // could never mature; the per-task window above
                        // now keeps the bar honest. Zero it
                        // (this runs in the tick IRQ on the task's own
                        // CPU, so the task cannot be concurrently inside
                        // a count mutation; other CPUs only ever add via
                        // current()). The leaked task immediately becomes
                        // sleepable/preemptible again. A legit 10 s-held
                        // section (if one ever exists) is already a
                        // wedge; its later unpairing sub will underflow,
                        // clamp and REPORT rather than poison. This is
                        // the safety net that lets IRQ-exit kernel
                        // preemption stay enabled while the leak's
                        // producer is still being hunted.
                        //
                        // HEAL-GATE (122nd, runq-strand follow-up): the
                        // 200-observation bar proves the count was
                        // PRESENT at every tick for 2 s — rs2 proved
                        // that cannot distinguish a leaked count from a
                        // LEGITIMATE waiter inside a slow acquire: those
                        // waits were on the stalled binder OWNER-LATCH
                        // chain (latch_age 2820ms) and the 20.5s heal
                        // storm zeroed live waiters' counts mid-acquire
                        // (was=0x10001/0x10003). The contended TTAS loop
                        // now publishes acquire-side evidence (per-CPU
                        // task+jiffies, refreshed every 64K spins —
                        // sync::spinlock::ACQUIRE_EVIDENCE); a count
                        // whose owner was observed in that loop within
                        // ACQUIRE_EVIDENCE_FRESH_JIF jiffies is
                        // acquire-coupled and LIVE: refuse the heal
                        // (both legs — a slow waiter actively spinning
                        // must never be healed). The refusal is
                        // rate-gated (20j, PREEMPT-LEAK policy) and does
                        // NOT reset the window or run counters — once
                        // the evidence goes stale (the waiter acquired,
                        // or was never spinning), the next tick heals a
                        // still-stuck count normally.
                        let base = pc & !crate::interrupt::preempt::HARDIRQ_MASK;
                        let acq_live = crate::sync::spinlock::acquire_evidence_fresh(
                            cpu_id,
                            cur as usize,
                            now,
                        );
                        // CARRIER-HEAL (123rd, heal-gate follow-up): the
                        // softirq-window carrier leg. HEAL-GATE's acquire
                        // stamp can only vouch for waiters INSIDE the
                        // contended TTAS loop; its residual healable class
                        // (HEAL-GATE.md §4.4) includes holders executing
                        // an __do_softirq window — the make-test VW-WEDGE
                        // conviction proved what a heal landing mid-window
                        // does (executor preemptible, handler never
                        // finishes, completions lost, machine wedged).
                        // Two signals, both carrier-side (no fast path
                        // touched): (1) the exact window-open flag —
                        // SOFTIRQ_IN_PROGRESS is written only by this
                        // CPU's own __do_softirq bracket, and this heal
                        // runs in the tick IRQ on the SAME CPU, so the
                        // pair can never run concurrently: a set flag
                        // means the tick interrupted a live window whose
                        // executor IS cur; (2) the {task, jiffies}
                        // CARRIER_EVIDENCE slot stamped at window entry
                        // and every vector dispatch — covers the
                        // just-closed-window sliver with the same
                        // freshness doctrine as the acquire stamp. A
                        // count whose holder is inside (or within 10j of)
                        // a softirq window is carrier-coupled and LIVE:
                        // refuse the heal, both legs. No window or run
                        // reset — once the window closes and the stamp
                        // goes stale, the next tick heals a still-stuck
                        // count normally (a parked-window victim is at
                        // most 10j + one window late).
                        let car_win =
                            crate::interrupt::softirq::in_progress_cpu(cpu_id);
                        let car_live = car_win
                            || crate::interrupt::softirq::carrier_evidence_fresh(
                                cpu_id,
                                cur as usize,
                                now,
                            );
                        if (base != 0 && (p_stale || s_stale) || base == 0 && h_stale)
                            && (acq_live || car_live)
                        {
                            static REFUSE_LAST_PRINT: core::sync::atomic::AtomicU64 =
                                core::sync::atomic::AtomicU64::new(0);
                            let rp = REFUSE_LAST_PRINT.load(
                                core::sync::atomic::Ordering::Acquire);
                            if now.wrapping_sub(rp) >= 20
                                && REFUSE_LAST_PRINT
                                    .compare_exchange(
                                        rp,
                                        now,
                                        core::sync::atomic::Ordering::AcqRel,
                                        core::sync::atomic::Ordering::Acquire)
                                    .is_ok()
                            {
                                use crate::debug::taskdump::putc_brief as putchar;
                                fn ph(v: u64) {
                                    use crate::debug::taskdump::putc_brief as putchar;
                                    for sh in (0..16).rev() {
                                        let n = ((v >> (sh * 4)) & 0xF) as u8;
                                        putchar(if n < 10 {
                                            b'0' + n
                                        } else {
                                            b'a' + n - 10
                                        });
                                    }
                                }
                                const M: &[u8] = b"\nPREEMPT-HEAL-REFUSED cpu=";
                                for &b in M {
                                    putchar(b);
                                }
                                ph(cpu_id as u64);
                                const M2: &[u8] = b" pid=";
                                for &b in M2 {
                                    putchar(b);
                                }
                                ph(crate::sched::edges::task_shared(cur).pid() as u64);
                                const M3: &[u8] = b" pc=";
                                for &b in M3 {
                                    putchar(b);
                                }
                                ph(pc as u32 as u64);
                                const M4: &[u8] = b" ev_age=";
                                for &b in M4 {
                                    putchar(b);
                                }
                                // evidence age (jiffies since this CPU's
                                // last contended-loop stamp by cur)
                                ph(now.wrapping_sub(
                                    crate::sync::spinlock::ACQUIRE_EVIDENCE[cpu_id]
                                        .jiffies
                                        .load(core::sync::atomic::Ordering::Relaxed),
                                ));
                                // CARRIER-HEAL: carrier-evidence age +
                                // which evidence refused the heal
                                // (A = acquire stamp, C = softirq-window
                                // carrier stamp/flag, AC = both).
                                const M4B: &[u8] = b"j car_age=";
                                for &b in M4B {
                                    putchar(b);
                                }
                                ph(crate::interrupt::softirq::carrier_evidence_age(
                                    cpu_id, now,
                                ));
                                const M5: &[u8] = b"j face=";
                                for &b in M5 {
                                    putchar(b);
                                }
                                putchar(if base != 0 { b'B' } else { b'H' });
                                const M5B: &[u8] = b" via=";
                                for &b in M5B {
                                    putchar(b);
                                }
                                if acq_live {
                                    putchar(b'A');
                                }
                                if car_live {
                                    putchar(b'C');
                                }
                                const M6: &[u8] =
                                    b" (live acquire/carrier evidence; heal deferred)";
                                for &b in M6 {
                                    putchar(b);
                                }
                                putchar(b'\n');
                            }
                        } else if base != 0 && (p_stale || s_stale) {
                            // Fleet session 3 (2026-10-25): clear base bits
                            // ONLY. This heal runs inside the timer IRQ
                            // bracket, so cur's count at this exact moment
                            // carries the tick's own in-flight
                            // HARDIRQ_OFFSET — the old whole-word zero
                            // deleted it and the irq_exit sub a few frames
                            // up then underflowed (count_was=0 sub=0x10000;
                            // one guaranteed PREEMPT-UNDERFLOW per heal
                            // firing — see ltp-a r18-r20 fleet evidence).
                            // PARKED-HEAL below keeps the full zero: a task
                            // current on NO CPU has no in-flight nesting by
                            // definition.
                            // phantom-bits round 2: when the same count also
                            // carries stale EXTRA hardirq levels (>= 2 at
                            // tick, one being the observer's own), collapse
                            // them to exactly one kept level instead of
                            // preserving them (clear_base keeps ALL hardirq
                            // bits, so a 0x10101-style heal used to leave
                            // the 0x10000 monopoly standing).
                            {
                                if h_stale {
                                    crate::sched::edges::task_shared(cur).ti_preempt_count_keep_tick_nesting();
                                } else {
                                    crate::sched::edges::task_shared(cur).ti_preempt_count_clear_base();
                                }
                            }
                            // "Actually preempt" (launcher perf wall): the
                            // zero alone leaves the incumbent running —
                            // the kernel IPC-loop shapes never call
                            // schedule() and the trap-exit asm gate only
                            // fires at user-mode return (sc267: the
                            // leaked incumbents were stranded exactly
                            // there, the 1-2.6 s self-bound). The flag is
                            // pure insurance: setting it never forces a
                            // switch by itself, but every preempt point
                            // the healed task passes from here on
                            // (user-return, cond_resched in loops,
                            // SleepMutex slowpath) now takes it instead of
                            // waiting for the next tick's slice check.
                            set_need_resched();
                            // Re-arm: next nonzero observation starts a
                            // fresh window instead of re-healing each
                            // tick.
                            w.task.store(0, core::sync::atomic::Ordering::Relaxed);
                            w.first.store(0, core::sync::atomic::Ordering::Relaxed);
                            w.last.store(0, core::sync::atomic::Ordering::Relaxed);
                            use crate::debug::taskdump::putc_brief as putchar;
                            fn ph(v: u64) {
                                use crate::debug::taskdump::putc_brief as putchar;
                                for sh in (0..16).rev() {
                                    let n = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if n < 10 {
                                        b'0' + n
                                    } else {
                                        b'a' + n - 10
                                    });
                                }
                            }
                            const M: &[u8] = b"\nPREEMPT-HEAL cpu=";
                            for &b in M {
                                putchar(b);
                            }
                            ph(cpu_id as u64);
                            const M2: &[u8] = b" pid=";
                            for &b in M2 {
                                putchar(b);
                            }
                            ph(crate::sched::edges::task_shared(cur).pid() as u64);
                            const M3: &[u8] = b" was=";
                            for &b in M3 {
                                putchar(b);
                            }
                            ph(pc as u32 as u64);
                            const M4: &[u8] = b" held_jiffies=";
                            for &b in M4 {
                                putchar(b);
                            }
                            ph(now.wrapping_sub(t0));
                            // stress-port r3: name the bars that fired
                            // (window age is bookkeeping; the RUNS are
                            // the proof — a post-heal storm shows
                            // run=1..3 with held=0, a true phantom shows
                            // 200+).
                            const M4B: &[u8] = b" prun=";
                            for &b in M4B {
                                putchar(b);
                            }
                            ph(crate::sched::edges::task_shared(cur).ti_pc_pirq_present_run());
                            const M4C: &[u8] = b" srun=";
                            for &b in M4C {
                                putchar(b);
                            }
                            ph(crate::sched::edges::task_shared(cur).ti_pc_sirq_present_run());
                            const M4D: &[u8] = b" hrun=";
                            for &b in M4D {
                                putchar(b);
                            }
                            ph(crate::sched::edges::task_shared(cur).ti_pc_hirq_present_run());
                            // CARRIER-HEAL (123rd): name the healed
                            // carrier at fire time with the tick-entry
                            // snapshots (already refreshed every tick on
                            // both arches): pctick = the incumbent's OWN
                            // count before this tick's HARDIRQ_OFFSET
                            // (splits the printed was= into task-owned
                            // bits vs observer offset), rip = the
                            // interrupted instruction (symbolize with
                            // addr2line; a userspace address means the
                            // count returned to user mode with the
                            // task — residue by definition), cs = the
                            // interrupted code segment (ring 3 = user,
                            // ring 0 = kernel carrier).
                            const M4E: &[u8] = b" pctick=";
                            for &b in M4E {
                                putchar(b);
                            }
                            ph(crate::interrupt::preempt::TICK_ENTRY_PC[cpu_id]
                                .load(core::sync::atomic::Ordering::Relaxed)
                                as u64);
                            const M4F: &[u8] = b" rip=";
                            for &b in M4F {
                                putchar(b);
                            }
                            ph(crate::interrupt::preempt::TICK_ENTRY_RIP[cpu_id]
                                .load(core::sync::atomic::Ordering::Relaxed));
                            const M4G: &[u8] = b" cs=";
                            for &b in M4G {
                                putchar(b);
                            }
                            ph(crate::interrupt::preempt::TICK_ENTRY_CS[cpu_id]
                                .load(core::sync::atomic::Ordering::Relaxed));
                            putchar(b'\n');
                            // preempt-hunt (2026-11-01): the
                            // outermost-acquire record is guaranteed
                            // intact here — the incumbent's base never
                            // returned to 0 since the phantom formed
                            // (inner pairs are 1->2->1 and do not clear
                            // it), so this names the acquire whose 1->0
                            // release is missing / late.
                            crate::debug::leakorigin::report_outermost(
                                b"PREEMPT-HEAL-ORIGIN",
                                cur,
                            );
                            // phantom-bits: when the healed count carried
                            // the SOFTIRQ bit, name its +0x100 booker
                            // (the monopolist face — the release never
                            // executed anywhere: no sub=0x100 underflow
                            // exists to point at it instead).
                            if pc & crate::interrupt::preempt::SOFTIRQ_MASK != 0 {
                                crate::debug::leakorigin::report_softirq_origin(
                                    b"PREEMPT-HEAL-SOFTIRQ",
                                    cur,
                                );
                            }
                        }
                        // phantom-bits round 2 — the pure-field faces the
                        // base leg can never reach: base == 0 with a
                        // stranded SOFTIRQ bit (pc=0x100 at task level) or
                        // extra HARDIRQ levels (pc=0x10000-phantom shows as
                        // >= 2 levels at tick, one being the observer's
                        // own). These monopolize exactly like the base
                        // face but the old gate (base != 0) skipped them
                        // by design — the unified keep-exactly-one-level
                        // form was written for them (task.rs
                        // ti_preempt_count_keep_tick_nesting) but never
                        // wired. Same 2 s window, same five-orders
                        // doctrine: a softirq bracket or hardirq nesting
                        // held across 200 consecutive ticks is phantom;
                        // the kept single level pairs the observer's own
                        // still-pending irq_exit sub.
                        // (HEAL-GATE: else-chained behind the refusal so a
                        // refused candidate cannot fall through to this
                        // leg.)
                        else if base == 0 && h_stale {
                            crate::sched::edges::task_shared(cur).ti_preempt_count_keep_tick_nesting();
                            set_need_resched();
                            w.task.store(0, core::sync::atomic::Ordering::Relaxed);
                            w.first.store(0, core::sync::atomic::Ordering::Relaxed);
                            w.last.store(0, core::sync::atomic::Ordering::Relaxed);
                            use crate::debug::taskdump::putc_brief as putchar;
                            fn ph(v: u64) {
                                use crate::debug::taskdump::putc_brief as putchar;
                                for sh in (0..16).rev() {
                                    let n = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if n < 10 {
                                        b'0' + n
                                    } else {
                                        b'a' + n - 10
                                    });
                                }
                            }
                            const M: &[u8] = b"\nPREEMPT-HEAL-FIELD cpu=";
                            for &b in M {
                                putchar(b);
                            }
                            ph(cpu_id as u64);
                            const M2: &[u8] = b" pid=";
                            for &b in M2 {
                                putchar(b);
                            }
                            ph(crate::sched::edges::task_shared(cur).pid() as u64);
                            const M3: &[u8] = b" was=";
                            for &b in M3 {
                                putchar(b);
                            }
                            ph(pc as u32 as u64);
                            const M4: &[u8] = b" held_jiffies=";
                            for &b in M4 {
                                putchar(b);
                            }
                            ph(now.wrapping_sub(t0));
                            const M5: &[u8] = b" face=";
                            for &b in M5 {
                                putchar(b);
                            }
                            putchar(if h_stale { b'H' } else { b'S' });
                            putchar(b'\n');
                            if s_stale {
                                crate::debug::leakorigin::report_softirq_origin(
                                    b"PREEMPT-HEAL-SOFTIRQ",
                                    cur,
                                );
                            }
                            if h_stale {
                                crate::debug::leakorigin::report_hardirq_origin(
                                    b"PREEMPT-HEAL-HARDIRQ",
                                    cur,
                                );
                            }
                        }
                    }
                }
            }
    }

    // Kernel-preemption hunt round 3 (2026-10-21, sc69+): sc69 falsified
    // the "hook causes the leak" hypothesis — the leak reproduces with
    // the IRQ-exit hook compiled out, so the producer is pre-existing.
    // The static audit of every preempt_count add/sub site finds no
    // unpaired path, so the remaining producers are runtime shapes: a
    // replayed acquire (a task resumed at a stale point re-running a
    // guard's fetch_add) or a direct wild write into thread_info. Both
    // violate ONE invariant: with no kernel preemption, a task that is
    // parked (current on NO CPU) can never legally hold a nonzero
    // preempt_count — guards exist only on the executing task, and the
    // do_exit R9-3 windows are dead/exiting by the time they park.
    //
    // Round 3b: keep per-task last-walk snapshots (count, parked) in a
    // side table. A task parked for TWO consecutive walks whose count
    // went 0 -> nonzero in between is the unambiguous wild-write proof
    // (no code path can add to a parked task); a task that acquired its
    // count while running narrows the producer to a replayed acquire in
    // that 40 ms window. PARKED-LEAK reports every parked-nonzero task
    // as before; the walk also stamps PARKWALK-ARMED once at boot so a
    // silent logic failure cannot masquerade as "no hits".
    {
        struct ParkSnap {
            task: core::sync::atomic::AtomicUsize,
            count: core::sync::atomic::AtomicI32,
            parked: core::sync::atomic::AtomicBool,
            seen: core::sync::atomic::AtomicU64,
            /// When this task was first seen parked with a nonzero
            /// count (0 = not currently in that state) — drives the
            /// parked-victim side of PREEMPT-HEAL.
            bad_since: core::sync::atomic::AtomicU64,
        }
        static SNAPS: [ParkSnap; 1024] = {
            const FN: ParkSnap = ParkSnap {
                task: core::sync::atomic::AtomicUsize::new(0),
                count: core::sync::atomic::AtomicI32::new(0),
                parked: core::sync::atomic::AtomicBool::new(false),
                seen: core::sync::atomic::AtomicU64::new(0),
                bad_since: core::sync::atomic::AtomicU64::new(0),
            };
            [FN; 1024]
        };
        static PARKWALK_LAST: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        static PARKWALK_PRINT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        static PARKWALK_ARMED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        let now = crate::timer::get_jiffies();
        let last = PARKWALK_LAST.load(core::sync::atomic::Ordering::Acquire);
        if now.wrapping_sub(last) >= 4
            && PARKWALK_LAST
                .compare_exchange(
                    last,
                    now,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
        {
            if !PARKWALK_ARMED.swap(true, core::sync::atomic::Ordering::AcqRel) {
                const MSG: &[u8] = b"PARKWALK-ARMED\n";
                for &b in MSG {
                    crate::debug::taskdump::putc_brief(b);
                }
            }
            // Walk epoch: a slot not refreshed for 2+ walks belongs to a
            // dead/exited task and may be reclaimed for new tasks.
            static WALK_EPOCH: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let epoch = WALK_EPOCH.fetch_add(1, core::sync::atomic::Ordering::AcqRel) + 1;
            // stress-port r3 census: one count per completed walk entry.
            if crate::trace::EVENT_LEAKHUNT.armed()
            {
                PARKWALK_CENSUS[0].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            struct Visit {
                task: *mut crate::process::Task,
                pc: i32,
                running: bool,
                // stress-port r8: was ANY cpu's ARCH tp register naming
                // the task AT SAMPLE TIME? The slot-based "running" test
                // alone cannot distinguish "parked" from "parking" — a
                // voluntary sleeper inside its own __schedule (state set,
                // own GRQ guard in hand, slot already flipped to next) is
                // slotless yet very much executing. tpexec=1 on a
                // parked-classified sample proves the sample caught that
                // transient (benign); tpexec=0 with pc!=0 is the genuine
                // wild-write/lying-slot face. Sampled here, not at print
                // time — the µs-scale window closes long before the
                // walk's post-processing runs (the r7 datum's report-time
                // tp=-1 was exactly this latency artifact).
                tp_named: bool,
            }
            // The pid-hash walk holds bucket locks; do the side-table
            // bookkeeping after collecting the snapshot (no locks held
            // while touching SNAPS beyond atomics).
            let mut visited: [Option<Visit>; 128] = [const { None }; 128];
            let mut nvisited = 0usize;
            crate::process::pid_hash::pid_hash_for_each_task_try(|task| {
                {
                    let pid = task.pid();
                    if pid == 0 || pid == TASK_POISON {
                        return;
                    }
                    // Skip the do_exit R9-3 windows: DEAD/ZOMBIE (and
                    // anything PF_EXITING) may park with a transient
                    // count while the exit path unwinds.
                    if task.state().is_dead()
                        || (task.ti_flags()
                            & crate::process::task::task_flags::TaskFlags::PF_EXITING
                                .bits()
                            != 0)
                    {
                        return;
                    }
                    let pc = task.preempt_count();
                    let mut running = false;
                    let mut tp_named = false;
                    // The side-table stores raw identity — recover the
                    // pointer from the walk reference (same value).
                    let task = task as *const crate::process::Task
                        as *mut crate::process::Task;
                    for c in 0..crate::config::MAX_CPUS {
                        if cpu_state(c).current == task {
                            running = true;
                        }
                        if crate::arch::cpu::get_thread_id_of(c) as *mut crate::process::Task
                            == task
                        {
                            tp_named = true;
                        }
                    }
                    // s14 (canary r1 msgstress01 capture): a task being
                    // switched OUT is slotless from the pick's slot
                    // publish until context_switch finishes saving it —
                    // and it LEGITIMATELY holds the GRQ guard (count >= 1)
                    // in the sub-window before the lock release. The
                    // walk classified that shape as "parked with a
                    // nonzero count" and PARKED-WILDWRITE fired on a
                    // state=RUNNING task (legal preemption switch-out),
                    // burning the print's "no in-tree path can do this"
                    // evidentiary value. R63's SWITCHING_OUT marker
                    // names exactly this window — treat it as running,
                    // for the wildwrite test AND the 4 s PARKED-HEAL
                    // (healing a mid-switch-out task's count to zero
                    // would re-open preemption inside its spinlock
                    // section and underflow the guard drop).
                    if !running && task_is_mid_switch_out(task) {
                        running = true;
                    }
                    // Sample up to 128 tasks per walk, biased to the
                    // interesting ones (nonzero count) so the table sees
                    // leak transitions even under heavy task churn.
                    if nvisited < 128 {
                        let interesting = pc != 0;
                        if interesting || (nvisited < 32) {
                            visited[nvisited] =
                                Some(Visit { task, pc, running, tp_named });
                            nvisited += 1;
                        }
                    }
                    // stress-port r3 census (hunt2 postmortem): the walk
                    // printed ZERO parked-nonzero sightings in a boot that
                    // stranded pid 307 for 100 s — record what the walk
                    // actually observes so the next strand's log answers
                    // "was the victim invisible to the walk, or clean?"
                    // (visibly clean supports the resume-time
                    // stale-bar false-fire, not a parked leak).
                    if crate::trace::EVENT_LEAKHUNT.armed()
                    {
                        crate::sched::sched::PARKWALK_CENSUS[1]
                            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if pc != 0 && !running {
                            crate::sched::sched::PARKWALK_CENSUS[2]
                                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
            });
            for v in visited.iter().take(nvisited) {
                let Visit { task, pc, running, tp_named } = match v {
                    Some(x) => x,
                    None => continue,
                };
                // Find or (re)claim a slot.
                let mut mine: Option<&ParkSnap> = None;
                let mut free: Option<&ParkSnap> = None;
                for s in SNAPS.iter() {
                    let key = s.task.load(core::sync::atomic::Ordering::Relaxed);
                    if key == *task as usize {
                        mine = Some(s);
                        break;
                    }
                    if key == 0 && free.is_none() {
                        free = Some(s);
                    }
                }
                let slot = match mine {
                    Some(s) => Some(s),
                    None => free,
                };
                let mut wildwrite_shape = false;
                if let Some(s) = slot {
                    let key = s.task.load(core::sync::atomic::Ordering::Relaxed);
                    if key == *task as usize {
                        let prev_pc =
                            s.count.load(core::sync::atomic::Ordering::Relaxed);
                        let prev_parked =
                            s.parked.load(core::sync::atomic::Ordering::Relaxed);
                        // Parked for two consecutive walks with a
                        // 0 -> nonzero count transition in between: no
                        // in-tree code path can do this. Wild write.
                        wildwrite_shape =
                            prev_parked && !*running && prev_pc == 0 && *pc != 0;
                    } else if s.task
                        .compare_exchange(
                            0,
                            *task as usize,
                            core::sync::atomic::Ordering::AcqRel,
                            core::sync::atomic::Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        s.seen.store(epoch, core::sync::atomic::Ordering::Relaxed);
                    }
                    // Reclaim check happens lazily at claim time below.
                }
                if let Some(s) = slot {
                    s.count.store(*pc, core::sync::atomic::Ordering::Relaxed);
                    s.parked.store(!*running, core::sync::atomic::Ordering::Relaxed);
                    s.seen.store(epoch, core::sync::atomic::Ordering::Relaxed);
                }
                // Parked-victim PREEMPT-HEAL (round 3d): the tick-side
                // heal keys on the per-CPU LEAK slot, which resets every
                // time the victim migrates or parks at a user boundary —
                // sc71 showed exactly that (pid 0x13c parked with count 1
                // for minutes, migrating between CPUs, never reaching the
                // 10 s same-CPU window). This side keys on the TASK: a
                // task seen parked (current on NO cpu) with a nonzero
                // base count for 4 s of consecutive walks gets its count
                // zeroed. A parked task holds no legal guard (guards live
                // on the executing task), so the count is phantom by
                // definition; zeroing lets the sleeper actually sleep.
                // stress-port r8: a task whose arch tp register names it
                // IS executing (its guard is live) — never heal it.
                if let Some(s) = slot {
                    if !*running && !*tp_named && *pc != 0 {
                        let bad0 = s.bad_since.load(core::sync::atomic::Ordering::Relaxed);
                        if bad0 == 0 {
                            s.bad_since.store(now, core::sync::atomic::Ordering::Relaxed);
                        } else if now.wrapping_sub(bad0) >= 400 {
                            { crate::sched::edges::task_shared(*task).ti_preempt_count_zero(); }
                            s.bad_since.store(0, core::sync::atomic::Ordering::Relaxed);
                            use crate::debug::taskdump::putc_brief as putchar;
                            fn ph(v: u64) {
                                use crate::debug::taskdump::putc_brief as putchar;
                                for sh in (0..16).rev() {
                                    let n = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if n < 10 {
                                        b'0' + n
                                    } else {
                                        b'a' + n - 10
                                    });
                                }
                            }
                            const M: &[u8] = b"\nPARKED-HEAL pid=";
                            for &b in M {
                                putchar(b);
                            }
                            ph(crate::sched::edges::task_shared(*task).pid() as u64);
                            const M2: &[u8] = b" was=";
                            for &b in M2 {
                                putchar(b);
                            }
                            ph(*pc as u32 as u64);
                            const M3: &[u8] = b" held_jiffies=";
                            for &b in M3 {
                                putchar(b);
                            }
                            ph(now.wrapping_sub(bad0));
                            putchar(b'\n');
                        }
                    } else {
                        s.bad_since.store(0, core::sync::atomic::Ordering::Relaxed);
                    }
                }
                if wildwrite_shape {
                    // Report immediately, not rate-gated: this is the
                    // proof event.
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn ph(v: u64) {
                        use crate::debug::taskdump::putc_brief as putchar;
                        for sh in (0..16).rev() {
                            let n = ((v >> (sh * 4)) & 0xF) as u8;
                            putchar(if n < 10 {
                                b'0' + n
                            } else {
                                b'a' + n - 10
                            });
                        }
                    }
                    const M: &[u8] = b"\nPARKED-WILDWRITE pid=";
                    for &b in M {
                        putchar(b);
                    }
                    ph(crate::sched::edges::task_shared(*task).pid() as u64);
                    const M2: &[u8] = b" pc=";
                    for &b in M2 {
                        putchar(b);
                    }
                    ph(*pc as u32 as u64);
                    // stress-port r8: tpexec=1 => the sample caught the
                    // task EXECUTING (its own park in flight — the
                    // slot-based classification is the artifact);
                    // tpexec=0 => genuinely parked at sample time.
                    const M2b: &[u8] = b" tpexec=";
                    for &b in M2b {
                        putchar(b);
                    }
                    ph(*tp_named as u64);
                    putchar(b'\n');
                }
                if *running || *pc == 0 {
                    continue;
                }
                // Parked with a live count: report, rate-gated to one
                // line per 2 s (direct console — IRQ context).
                let lp = PARKWALK_PRINT.load(core::sync::atomic::Ordering::Acquire);
                if now.wrapping_sub(lp) >= 20
                    && PARKWALK_PRINT
                        .compare_exchange(
                            lp,
                            now,
                            core::sync::atomic::Ordering::AcqRel,
                            core::sync::atomic::Ordering::Acquire,
                        )
                        .is_ok()
                {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn ph(v: u64) {
                        use crate::debug::taskdump::putc_brief as putchar;
                        for sh in (0..16).rev() {
                            let n = ((v >> (sh * 4)) & 0xF) as u8;
                            putchar(if n < 10 {
                                b'0' + n
                            } else {
                                b'a' + n - 10
                            });
                        }
                    }
                    const M: &[u8] = b"\nPARKED-LEAK pid=";
                    for &b in M {
                        putchar(b);
                    }
                    ph(crate::sched::edges::task_shared(*task).pid() as u64);
                    const M2: &[u8] = b" pc=";
                    for &b in M2 {
                        putchar(b);
                    }
                    ph(*pc as u32 as u64);
                    const M3: &[u8] = b" state=";
                    for &b in M3 {
                        putchar(b);
                    }
                    ph(crate::sched::edges::task_shared(*task).state().bits() as u64);
                    const M4: &[u8] = b" tcpu=";
                    for &b in M4 {
                        putchar(b);
                    }
                    ph(crate::sched::edges::task_shared(*task).ti_cpu() as u32 as u64);
                    const M4b: &[u8] = b" tpexec=";
                    for &b in M4b {
                        putchar(b);
                    }
                    ph(*tp_named as u64);
                    putchar(b'\n');
                    // rux:leakhunt (retired debug=leakhunt): name the acquire site of the leaked
                    // count (last PREEMPT_OFFSET acquire on this task).
                    crate::debug::leakorigin::report_parked_site(*task);
                }
            }
            // Lazily reclaim slots whose tasks missed 2+ walks.
            for s in SNAPS.iter() {
                let key = s.task.load(core::sync::atomic::Ordering::Relaxed);
                if key != 0 {
                    let seen = s.seen.load(core::sync::atomic::Ordering::Relaxed);
                    if epoch.wrapping_sub(seen) > 2 {
                        let _ = s.task.compare_exchange(
                            key,
                            0,
                            core::sync::atomic::Ordering::AcqRel,
                            core::sync::atomic::Ordering::Acquire,
                        );
                    }
                }
            }
            // stress-port r3 census print: every 1000 walks while
            // rux:leakhunt (retired debug=leakhunt) is armed (direct console — IRQ context).
            if crate::trace::EVENT_LEAKHUNT.armed()
                && epoch % 1000 == 0
            {
                use crate::debug::taskdump::putc_brief as putchar;
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
                for &b in b"\nPARKWALK-CENSUS walks=" {
                    putchar(b);
                }
                pdec(PARKWALK_CENSUS[0].load(core::sync::atomic::Ordering::Relaxed));
                for &b in b" obs=" {
                    putchar(b);
                }
                pdec(PARKWALK_CENSUS[1].load(core::sync::atomic::Ordering::Relaxed));
                for &b in b" parkednz=" {
                    putchar(b);
                }
                pdec(PARKWALK_CENSUS[2].load(core::sync::atomic::Ordering::Relaxed));
                putchar(b'\n');
            }
        }
    }

    // Poll UART for pending data — MUST be before current check.
    if crate::console::uart_has_data() {
        crate::console::read_waitq().wake_up_one();
    }

    // Update load average (auto-throttled to every 5 seconds).
    crate::fs::perf_ctr::update_load_avg();

    // R18-3b: full context line for correlation.
    // R18-3: per-tick canary check — DIAGNOSED (r18): pid 305's call
    // chain legitimately reaches the stack's deepest 8 bytes; frames spill
    // into the adjacent heap page (the children-list zeroing engine).
    // First-hit-only reporting; the fix is stack-size/depth work (r19).
    if let Some(t) = crate::sched::current() {
        unsafe {
            let t = crate::sched::edges::task_shared(t);
            let bottom = t.kernel_stack_bottom();
            if bottom != 0 {
                let v = core::ptr::read_volatile(bottom as *const u64);
                static REPORTED: core::sync::atomic::AtomicBool =
                    core::sync::atomic::AtomicBool::new(false);
                if v != 0xCAFE_F00D_DEAD_BEEF
                    && v != 0
                    && !REPORTED.swap(true, core::sync::atomic::Ordering::AcqRel)
                {
                    use crate::console::putchar;
                    const MSG: &[u8] = b"TICK-CANARY pid=";
                    for &b in MSG { putchar(b); }
                    let pid = crate::sched::edges::task_shared(t).pid();
                    let mut vv = pid as usize;
                    if vv == 0 { putchar(b'0'); }
                    let mut dd = [0u8; 10]; let mut kk = 0;
                    while vv > 0 { dd[kk] = b'0' + (vv % 10) as u8; kk += 1; vv /= 10; }
                    while kk > 0 { kk -= 1; putchar(dd[kk]); }
                    const M2: &[u8] = b" v=0x";
                    for &b in M2 { putchar(b); }
                    let mut sh = 64;
                    while sh > 0 { sh -= 4; let nb = ((v >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                    putchar(b'\n');
                    // R18-6: consistency — bottom vs live kernel_stack.
                    const M0: &[u8] = b" bottom=0x";
                    for &b in M0 { putchar(b); }
                    {
                        let b2 = bottom;
                        let mut sh0 = 64;
                        while sh0 > 0 { sh0 -= 4; let nb = ((b2 >> sh0) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                    }
                    const M6: &[u8] = b" ks=0x";
                    for &b in M6 { putchar(b); }
                    {
                        let ks = match crate::sched::edges::task_shared(t).get_kernel_stack() { Some(p) => p as usize, None => 0 };
                        let mut sh = 64;
                        while sh > 0 { sh -= 4; let nb = ((ks >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                    }
                    putchar(b'\n');
                    // R18-5: the interrupted epc — if the smash is fresh,
                    // the victim was executing in the deep chain RIGHT NOW.
                    const M5: &[u8] = b" epc=0x";
                    for &b in M5 { putchar(b); }
                    {
                        use crate::arch::trap::current_pt_regs;
                        use crate::arch::pt_regs::PtRegs;
                        let pr = current_pt_regs() as *const PtRegs;
                        if !pr.is_null() {
                            let e = crate::sched::edges::current_trap_ip();
                            let mut sh = 64;
                            while sh > 0 { sh -= 4; let nb = ((e >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                        }
                    }
                    putchar(b'\n');
                    // R18-4: frame count + deepest symbols at smash time.
                    let mut frames = 0u32;
                    crate::debug::backtrace::walk_stack_trace(&mut |pc, _fp| {
                        frames += 1;
                        if frames <= 6 {
                            const M3: &[u8] = b" f=0x";
                            for &b in M3 { putchar(b); }
                            let mut sh = 64;
                            while sh > 0 { sh -= 4; let nb = ((pc >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                        }
                    });
                    const M4: &[u8] = b" frames=";
                    for &b in M4 { putchar(b); }
                    let mut vv = frames;
                    if vv == 0 { putchar(b'0'); }
                    let mut dd = [0u8; 6]; let mut kk = 0;
                    while vv > 0 { dd[kk] = b'0' + (vv % 10) as u8; kk += 1; vv /= 10; }
                    while kk > 0 { kk -= 1; putchar(dd[kk]); }
                    putchar(b'\n');
                    // repair so we report once
                    core::ptr::write_volatile(bottom as *mut u64, 0xCAFE_F00D_DEAD_BEEF);
                }
            }
        }
    }

    // freed-task lane (2026-11-15, live capture): this slot-resolved
    // `current` is the dereferencer that MUTATED a freed Task page in
    // the dio_read storm — a lying slot (task freed after its on_cpu
    // cleared, slot never re-published) fed Task::rlimit() a painted
    // page; the lock bracket inside rlimit() fetched +1/-1 on the
    // 0xc3c3c3c3 paint (PREEMPT-UNDERFLOW count_was=0xc3c3c3c4,
    // FREED-TOUCH cpu-cur, chain scheduler_tick->Task::rlimit). Gate
    // every use below on the poison: a DEADBEEF/painted pid or a
    // dead state means the slot is stale — fall back to the hardware
    // truth (tp) and skip the tick work for the ghost. One budgeted
    // report per boot class; the free-side slot-quiescence gate in
    // free_task_slot keeps this from ever firing on a healthy boot.
    let mut current = this_cpu().current;
    if !current.is_null() {
        // SAFETY: read at compile-time Task offsets in the kernel
        // linear map (same contract as the R127 screen / poison probe
        // — cannot fault).
        let ghost = {
            let (pid, state) = crate::sched::edges::task_poison_pair_volatile(current);
            pid == TASK_POISON || state == TASK_POISON
        };
        if ghost {
            static GHOST_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if GHOST_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                use crate::debug::taskdump::putc_brief as putchar;
                const M: &[u8] = b"\nTICK-GHOST-CURRENT cpu=";
                for &b in M {
                    putchar(b);
                }
                let mut v = cpu_id as u64;
                {
                    let mut buf = [0u8; 10];
                    let mut n = 0;
                    if v == 0 {
                        n = 1;
                        buf[0] = b'0';
                    }
                    while v > 0 && n < buf.len() {
                        buf[n] = b'0' + (v % 10) as u8;
                        v /= 10;
                        n += 1;
                    }
                    let mut i = n;
                    while i > 0 {
                        i -= 1;
                        putchar(buf[i]);
                    }
                }
                const M2: &[u8] = b" slot=0x";
                for &b in M2 {
                    putchar(b);
                }
                let mut a = current as usize as u64;
                for sh in (0..16).rev() {
                    putchar(match ((a >> (sh * 4)) & 0xF) as u8 {
                        n if n < 10 => b'0' + n,
                        n => b'a' + n - 10,
                    });
                }
                putchar(b'\n');
            }
            // The hardware truth: tp still names the task this CPU
            // actually executes; the tick's per-task work runs on that.
            current = crate::arch::cpu::get_thread_id() as *mut Task;
        }
    }
    if current.is_null() {
        return;
    }

    // RLIMIT_CPU enforcement (P1 rlimits): CPU seconds beyond the soft
    // limit earn SIGXCPU once per further second (Linux re-arms it every
    // second), the hard limit earns SIGKILL. CPU time comes from the
    // sched entity's sum_exec_runtime (ns). Signal delivery from tick
    // context follows the posix-timer precedent (timer.rs softirq calls
    // send_signal directly). Kernel threads and idle carry RLIM_INFINITY
    // and skip the check entirely.
    {
        // INV-SCE-2: the rlimit cgroup-tick check reads the current
        // task's own fields.
        let current_t = crate::sched::edges::task_shared(current);
        let (soft, hard) =
            current_t.rlimit(crate::process::task::rlimit_res::CPU);
        if soft != u64::MAX || hard != u64::MAX {
            let secs = current_t
                .sched_entity()
                .sum_exec_runtime
                .load(core::sync::atomic::Ordering::Acquire)
                / 1_000_000_000;
            if hard != u64::MAX && secs >= hard {
                let _ = crate::signal::send_signal(
                    crate::sched::edges::task_shared(current).pid() as u32,
                    crate::signal::Signal::SIGKILL as i32,
                );
            } else if soft != u64::MAX && secs >= soft && secs > crate::sched::edges::task_shared(current).cpu_time_last_sigxcpu() {
                current_t.set_cpu_time_last_sigxcpu(secs);
                let _ = crate::signal::send_signal(
                    crate::sched::edges::task_shared(current).pid() as u32,
                    crate::signal::Signal::SIGXCPU as i32,
                );
            }
        }
    }

    // ITIMER_VIRTUAL / ITIMER_PROF (P2): CPU-time interval timers fire
    // against the sched entity's sum_exec_runtime (ns). VIRTUAL counts
    // only user execution — approximated by the task's own runtime; PROF
    // would add system time, which the single accounting counter cannot
    // split, so both track the same total. Delivery follows the
    // RLIMIT_CPU / posix-timer precedent (send_signal from tick context).
    // A one-shot (interval==0) disarms after firing.
    {
        let current_t = crate::sched::edges::task_shared(current);
        let se = current_t.sched_entity();
        let now = se.sum_exec_runtime.load(core::sync::atomic::Ordering::Acquire);
        let virt_deadline = current_t.itimer_virt[0].load(core::sync::atomic::Ordering::Acquire);
        if virt_deadline != 0 && now >= virt_deadline {
            let interval =
                current_t.itimer_virt[1].load(core::sync::atomic::Ordering::Acquire);
            let next = if interval != 0 { now + interval } else { 0 };
            current_t.itimer_virt[0].store(next, core::sync::atomic::Ordering::Release);
            let _ = crate::signal::send_signal(
                crate::sched::edges::task_shared(current).pid() as u32,
                crate::signal::Signal::SIGVTALRM as i32,
            );
        }
        let prof_deadline = current_t.itimer_prof[0].load(core::sync::atomic::Ordering::Acquire);
        if prof_deadline != 0 && now >= prof_deadline {
            let interval =
                current_t.itimer_prof[1].load(core::sync::atomic::Ordering::Acquire);
            let next = if interval != 0 { now + interval } else { 0 };
            current_t.itimer_prof[0].store(next, core::sync::atomic::Ordering::Release);
            let _ = crate::signal::send_signal(
                crate::sched::edges::task_shared(current).pid() as u32,
                crate::signal::Signal::SIGPROF as i32,
            );
        }

        // POSIX timers on the CPU-time clocks (CLOCK_PROCESS/THREAD_
        // CPUTIME_ID, LTP timer_settime01/timer_delete01): same accounting
        // counter, per-timer deadlines in posix_timers; SIGEV_SIGNAL
        // delivery only (SIGEV_NONE just counts the expiry).
        // — if the interrupted task holds the lock (mid timer_settime),
        // spinning for it would self-deadlock the CPU. A skipped check
        // simply re-runs on the next tick.
        if let Some(posix_timers) = current_t.posix_timers.try_lock() {
            for pt in posix_timers.iter() {
                let deadline = pt.cputime_deadline_ns.load(core::sync::atomic::Ordering::Acquire);
                if deadline != 0 && now >= deadline {
                    let interval =
                        pt.cputime_interval_ns.load(core::sync::atomic::Ordering::Acquire);
                    let next = if interval != 0 { now + interval } else { 0 };
                    pt.cputime_deadline_ns.store(next, core::sync::atomic::Ordering::Release);
                    if pt.sigev_notify != 1 && pt.sigev_signo > 0 {
                        let _ = crate::signal::send_signal(
                            crate::sched::edges::task_shared(current).pid() as u32,
                            pt.sigev_signo,
                        );
                    }
                }
            }
        }
    }

    // RT bandwidth accounting / throttle (batch 8) — runs every tick on
    // every CPU, before the class-specific work. Per-CPU (the 70th): the
    // ticking cpu charges ITS OWN rt_rq budget.
    rt_bandwidth_tick(crate::arch::cpu_id() as u64 as usize, current);

    // timer-gap lane: pinned-RT wedge-state dump (rt2xdbg=1-gated).
    rt_wedge_dump_tick(crate::arch::cpu_id() as u64 as usize);

    // cgroup v2 cpu controller (U1b): charge this tick into the current
    // task's cgroup chain, roll quota periods over / unthrottle, and
    // force a resched when the chain is (or was) throttled so the CPU
    // switches away from / reconsiders affected tasks.
    if crate::sched::cgroup::cgroup_cpu_tick(current) {
        set_need_resched();
    }

    // current is this_cpu().current, a valid Task pointer set during CPU
    // init; null check above; fields of the current CPU's task (INV-SCE-2).
    {
        let current = crate::sched::edges::task_shared(current);
        let policy = current.policy();

        match policy {
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                let now = crate::sched::entities::sched_clock();

                // Single GRQ lock acquisition: update vruntime + check
                // preemption together.  Previously this was two separate
                // lock/unlock cycles, which doubled the contention window
                // and caused soft lockups when other CPUs held the lock.
                let should_resched = {
                    let mut grq_guard = grq().lock_irqsave_site(grq_site(GRQ_CALLER_TICK, GRQ_STEP_TRY));
                    // R20-6: see __schedule — only the CPU whose task owns
                    // the single cfs_rq.curr slot charges through it; every
                    // CPU charges its own current task exactly once.
                    if grq_guard.cfs_rq.get_curr() == core::ptr::from_ref(current) as *mut Task {
                        grq_guard.cfs_rq.update_curr(now);
                    } else {
                        let se = crate::sched::edges::task_shared(current).sched_entity();
                        let delta = se.update_exec_runtime(now);
                        if delta > 0 {
                            se.update_vruntime(delta);
                        }
                    }

                    // Batch 8: preempt when the CURRENT ACTIVATION has run
                    // longer than its time slice (Linux check_preempt_tick:
                    // `if (delta_exec > slice) resched_curr()`). The old
                    // vruntime-vs-leftmost comparison misjudged unequal
                    // weights: a heavy task's slice is proportionally
                    // longer than the time its vruntime needs to overtake
                    // the leftmost (and vice versa for light tasks).
                    //
                    // NOHZ round: the reference guards check_preempt_tick
                    // with `if (cfs_rq->nr_running > 1)` (entity_tick) — a
                    // SOLO runnable task is never tick-preempted. The GRQ
                    // slice check ran unconditionally, so a solo CPU-bound
                    // task (the lmbench calibration chase) was dequeued,
                    // requeued and re-picked on EVERY tick: pure churn
                    // (pick-state writes + sub_preempt_count + the ENQSB
                    // print storm in the requeue path) that lands inside
                    // every measured interval.
                    //
                    // STRUCTURAL MAPPING: the reference's cfs_rq->
                    // nr_running INCLUDES the running current (on_rq);
                    // Rux's timeline count drops it at pick, so
                    // `reference_count > 1` maps to
                    // `timeline_count + 1 > 1` = `timeline_count > 0`
                    // here — at least one OTHER queued CFS task.
                    if grq_guard.cfs_rq.nr_running() > 0 {
                        let se = crate::sched::edges::task_shared(current).sched_entity();
                        let slice_ns = grq_guard.cfs_rq.sched_slice(se);
                        let sum =
                            se.sum_exec_runtime.load(core::sync::atomic::Ordering::Acquire);
                        let prev_sum = se
                            .prev_sum_exec_runtime
                            .load(core::sync::atomic::Ordering::Acquire);
                        sum.saturating_sub(prev_sum) > slice_ns
                    } else {
                        false
                    }
                }; // grq_guard dropped here

                if should_resched {
                    set_need_resched();
                }
            }
            SchedPolicy::Rr => {
                let rt_entity = crate::sched::edges::task_shared(current).rt_entity();
                let remaining = rt_entity.dec_time_slice();
                if remaining == 0 {
                    rt_entity.reset_time_slice();
                    // R20-1: only rotate a task that is still RUNNING. The
                    // tick IRQ can land inside two windows where the old
                    // unconditional set_state(RUNNING)+enqueue corrupted
                    // state:
                    //  (a) prepare_to_wait has set INTERRUPTIBLE but
                    //      schedule() has not run yet — the rotation would
                    //      resurrect the sleeper onto the runqueue;
                    //  (b) do_exit has set ZOMBIE (its preempt_disable does
                    //      NOT block the timer IRQ, only the IRQ-exit
                    //      preemption point) — the rotation would re-queue
                    //      an exiting task, letting another CPU pick and
                    //      resume a zombie.
                    // current is paused on THIS CPU inside the IRQ, so the
                    // state read is race-free.
                    if crate::sched::edges::task_shared(current).state() == TaskState::new(TaskState::RUNNING) {
                        let mut grq_guard = grq().lock_irqsave_site(grq_site(GRQ_CALLER_TICK, GRQ_STEP_TRY));
                        // R39: propagate the insert result and pair the
                        // grq.nr_running count. The rotation links a RUNNING
                        // task that __schedule will then NOT re-count (its
                        // on_rq guard skips the insert, so the fetch_add
                        // there never fires) — every RR rotation leaked one
                        // count from the atomic, drift that any future
                        // reader of grq.nr_running would inherit.
                        if grq_guard.rt_rq.enqueue(core::ptr::from_ref(current) as *mut Task, false) {
                            grq_guard.nr_running.fetch_add(
                                1,
                                core::sync::atomic::Ordering::Relaxed,
                            );
                        }
                        set_need_resched(); // Set before dropping lock to prevent lost wake-up
                        drop(grq_guard);
                    }
                }
            }
            SchedPolicy::Fifo => {
                // FIFO: no time slice management
            }
            SchedPolicy::Deadline => {
                let now = crate::sched::entities::sched_clock();
                let dl_entity = crate::sched::edges::task_shared(current).dl_entity();
                let delta = now - dl_entity.exec_start.load(core::sync::atomic::Ordering::Relaxed);
                dl_entity.exec_start.store(now, core::sync::atomic::Ordering::Release);
                if !dl_entity.consume_runtime(delta) {
                    // Batch 8 (CBS): budget exhausted — throttle (state left
                    // INTERRUPTIBLE, replenish timer armed at the deadline)
                    // and leave the CPU to lower classes.
                    dl_throttle(crate::sched::edges::task_mut(core::ptr::from_ref(current) as *mut Task));
                    set_need_resched();
                }
            }
        }
    }
}

// ==================== Context Switch ====================


#[no_mangle]
pub extern "C" fn schedule_tail(_prev: *mut Task) {
    // forkprof: first Rust code in the newborn's frame — splits the
    // switch machinery (context_switch entry -> here) from the
    // return-to-user tail (here -> first syscall entry).
    if crate::debug::forkprof::trace_on() {
        let pid = crate::sched::get_current_pid();
        crate::debug::forkprof::trace(12, pid); // T_NEWBORN_TAIL
    }
    // Called after context_switch in the new task's context (ret_from_fork).
    // R9-2 (HIGH): a newborn never returns into __schedule's tail, so the
    // per-CPU deferred exit notify stored by the task we replaced would be
    // skipped — and the next exit on this CPU overwrites the slot, losing
    // the parent's SIGCHLD forever (silent wait hang in fork+fast-exit
    // pipelines). Process it here; the slot clear makes this idempotent
    // with the __schedule tail.
    //
    // fg1 (oh-launch round 5): enable IRQs BEFORE the notify — the
    // newborn frame inherits the switcher's IRQs-off window, and the
    // notify's wake chain (signal_wake_up / wake_group_chldexit ->
    // wake_up) must never spin on a waitqueue lock with interrupts
    // disabled (the 4.5 s "not servicing IPIs" holder shape; same fix
    // as the repick path). The trap-exit tails do their own cli before
    // the register pops, so returning IF=1 here is safe (SP-5).
    crate::arch::cpu::enable_irq();
    process_deferred_exit_notify_cpu(crate::arch::cpu_id() as usize);
    // R63: for the same reason, retire the switch-out ownership mark the
    // replaced task's __schedule set on this CPU — a newborn's first
    // resume bypasses __schedule's tail, so without this the mark would
    // linger until this CPU's next context switch and keep the phantom
    // heal conservative for a task that is already fully switched out.
    SWITCHING_OUT[crate::arch::cpu_id() as usize].store(0, core::sync::atomic::Ordering::Release);
    // R11 (terminal-release window): the newborn's context is by
    // definition not the replaced task's — the replaced prev's final
    // switch-out has really completed here, so publish the positive
    // terminal flag for the exit-path frees (same store as the
    // __schedule-tail retire site; a dying task the newborn replaced
    // would otherwise wait for a tail that never runs).
    // P0 exit-reap (2026-11-23): same pacing discriminator as the
    // __schedule-tail retire — the done marker is stamped ONLY on a
    // task whose terminal deschedule loop was executing when the
    // newborn replaced it. The old unconditional store stamped a stale
    // tsd=true onto every task a newborn ever replaced (in a fork storm
    // that is every parked parent); the stale flag survives until the
    // task's own terminal loop re-arms it, and a task preempted between
    // its ZOMBIE store and that re-arm would present
    // {dead-ish, !on_cpu, slotless, tsd=true} to the reap drainer's
    // switch stage — freed before its exit tail finished (the
    // recycled-page face, the exact class boot-63's pacing gate
    // exists to close).
    if !_prev.is_null() {
        // INV-SCE-2: the newborn's context is by definition not prev's
        // (the R11 terminal-release positive-knowledge store).
        let pt = crate::sched::edges::task_shared(_prev);
        if pt
            .terminal_pacing
            .swap(false, core::sync::atomic::Ordering::AcqRel)
        {
            pt.terminal_switch_done
                .store(true, core::sync::atomic::Ordering::Release);
        }
    }
    // r11: and retire the pick generation that switched us in — the
    // newborn is the materialization of this CPU's current generation
    // (the fourth retire site; without it every fork child leaves its
    // publisher's generation unretired).
    pick_gen_retire(crate::arch::cpu_id() as usize);
}

// ==================== Utility Functions ====================


/// R25-6: load_weight rebalance for an in-tree reweight (GRQ-locked caller).
pub fn grq_cfs_adjust(old_w: u64, new_w: u64) {
    // Operate on the GRQ's CFS class — the caller holds the GRQ lock
    // (INV-SCE-3 mutation side; interior-mutable atomics need no &mut).
    let g = grq();
    g.cfs_rq.load_weight.fetch_add(new_w, core::sync::atomic::Ordering::AcqRel);
    g.cfs_rq.load_weight.fetch_sub(old_w, core::sync::atomic::Ordering::AcqRel);
}


/// Iterate over all tasks.
///
/// R20-3: iterate the PID hash table. The previous implementation only
/// walked the per-CPU current+idle pointers, making every caller that
/// needs "all tasks" blind to sleeping/queued tasks:
///   - mm/rmap.rs try_to_unmap: stale PTEs in sleeping tasks survived
///     swap-out/migration (data-corruption family);
///   - mm/compact.rs migration: sleeping tasks were skipped entirely;
///   - debug/hung_task.rs: D-state tasks are by definition not running on
///     any CPU, so the detector never saw its exact target;
///   - sysinfo procs count: undercount.
/// kill(-1) in syscall/process.rs already documented this gap and switched
/// to pid_hash_for_each_task; these callers went through this wrapper.
///
/// `f` runs under the per-bucket lock: a task linked into a bucket cannot
/// be removed (removal needs the same lock) and therefore cannot be freed
/// while `f` runs. `f` must not sleep or take a PID-hash bucket lock.
pub fn for_each_task<F>(mut f: F)
where
    F: FnMut(&Task),
{
    crate::process::pid_hash::pid_hash_for_each_task(|t| f(t));
}

/// Non-blocking variant of [`for_each_task`] for NON-PREEMPTIBLE contexts
/// (the timer-softirq heartbeat slot and any IRQ-side diagnostic).
///
/// lb325 wedge forensics (18th successor): both `debug::slowtask::audit` and
/// `futex_stuck_census` phase B walk tasks from the 1 Hz heartbeat, which
/// runs with IF=0. A task-context pid-hash writer (insert/remove/for_each
/// callback) that a timer interrupt suspends mid-critical-section holds its
/// bucket lock across the suspension; the heartbeat walker on that CPU then
/// reaches the SAME bucket, blocks on `lock_bucket`, and the interrupted
/// holder can never resume — self-deadlock by interrupt (the second CPU's
/// heartbeat walker joins the spin; serial goes silent; QEMU burns 2 vCPUs
/// forever). Evidence: both vCPUs at the identical RIP inside
/// `AtomicBool::compare_exchange` with RDI = pid_hash BUCKET_LOCK+22,
/// RFL IF=0 on both, BUCKET_LOCK[22] the only held byte in the array, and
/// the active frames `slowtask::audit → for_each_task →
/// pid_hash_for_each_task → lock_bucket` / `futex_stuck_census →
/// for_each_task → …`.
///
/// The blocking walk stays correct for preemptible callers (task context,
/// kernel threads): there the holder always resumes. IRQ-side callers MUST
/// use this try variant — buckets held at snapshot time are skipped and the
/// skipped count is returned for the caller to report. Repeated snapshots
/// converge: a bucket skipped one round is usually free the next.
pub fn for_each_task_try<F>(mut f: F) -> usize
where
    F: FnMut(&Task),
{
    crate::process::pid_hash::pid_hash_for_each_task_try(|t| f(t))
}


pub fn get_current_pid() -> u32 {
    let tp = crate::arch::cpu::get_thread_id() as *const Task;
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        // SAFETY: tp is the current task pointer from tp register; null and address checks above.
        crate::sched::edges::task_shared(tp).pid()
    }
}

pub fn get_current_ppid() -> u32 {
    let tp = crate::arch::cpu::get_thread_id() as *const Task;
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        // SAFETY: tp is the current task pointer from tp register; null and address checks above.
        crate::sched::edges::task_shared(tp).ppid()
    }
}

pub fn find_task_by_pid(pid: Pid) -> *mut Task {
    crate::process::pid_hash::pid_hash_lookup(pid)
}


/// Compatibility stubs — no longer meaningful with global RQ
pub fn this_cpu_rq() -> Option<&'static crate::sync::spinlock::Spinlock<RunQueue>> {
    None
}

pub fn cpu_rq(_cpu_id: usize) -> Option<&'static crate::sync::spinlock::Spinlock<RunQueue>> {
    None
}

// ==================== CPU Idle Loop ====================


/// Per-policy on_rq flag read (cheap, unlocked — a pre-filter only; the
/// GRQ-locked paths never trust it).
pub fn task_onrq_flag(t: &Task) -> bool {
    match t.policy() {
        SchedPolicy::Fifo | SchedPolicy::Rr => t.rt_entity().is_on_rq(),
        SchedPolicy::Deadline => t
            .dl_entity()
            .on_rq
            .load(core::sync::atomic::Ordering::Acquire),
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            t.sched_entity().is_on_rq()
        }
    }
}


/// R80 (diag): one parked-context line for a harvested orphan, rate-limited
/// to one per 25 jiffies system-wide (the v2 storm healed ~3 victims/s; the
/// console must not become the bottleneck). Mirrors the R76 parked-stack
/// walk: caller frames of the saved kernel sp (kernel-text words only,
/// symbolizable with addr2line). Runs from the idle-loop harvester with NO
/// GRQ held — reads are volatile and diagnostic-only; the task is pinned by
/// the caller's try_pin.
fn harvest_print_parked(task: *mut Task, pid: u32) {
    use crate::debug::taskdump::putc_brief as putchar;
    static LAST: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST.load(core::sync::atomic::Ordering::Relaxed);
    if now.wrapping_sub(last) < 25 {
        return;
    }
    if LAST
        .compare_exchange(last, now, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
        .is_err()
    {
        return;
    }
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
    {
        const M: &[u8] = b"  ORPHAN-CTX pid=";
        for &b in M { putchar(b); }
        pdec(pid as u64);
        const O: &[u8] = b" o=";
        for &b in O { putchar(b); }
        putchar(b'0' + crate::sched::edges::task_shared(task).on_cpu() as u8);
        let claim = crate::sched::edges::task_shared(task).running_on_cpu.load(core::sync::atomic::Ordering::Relaxed);
        const C: &[u8] = b" cl=";
        for &b in C { putchar(b); }
        if (claim as i64) < 0 { putchar(b'-'); }
        pdec((claim as i64).unsigned_abs() as u64);
        const P: &[u8] = b" pk=";
        for &b in P { putchar(b); }
        pdec(crate::sched::edges::task_shared(task).pick_state() as u64);
        const T: &[u8] = b" ti_cpu=";
        for &b in T { putchar(b); }
        pdec(crate::sched::edges::task_shared(task).ti_cpu() as i64 as u64);
        let thsp = crate::sched::edges::task_shared(task).thread().sp;
        const K: &[u8] = b" ksp=0x";
        for &b in K { putchar(b); }
        phex(crate::sched::edges::task_shared(task).ti_kernel_sp());
        if (thsp >> 48) == 0xffff && (thsp & 7) == 0 && thsp > 0x1000 {
            const S: &[u8] = b" stk=0x";
            for &b in S { putchar(b); }
            phex(thsp);
            // Caller frames above the saved sp (stacks grow down).
            let base = thsp as *const u64;
            let mut shown = 0;
            for k in 0..64 {
                // SAFETY: diagnostic volatile read of the pinned task's own
                // kernel-stack pages.
                let w = crate::sync::edges::read_volatile_u64((base as usize) + 8 * k);
                let (lo, hi) = kernel_text_window();
                if w >= lo && w < hi && (w & 1) == 0 {
                    putchar(b' ');
                    phex(w);
                    shown += 1;
                    if shown >= 6 {
                        break;
                    }
                }
            }
        }
        putchar(b'\n');
    }
}

/// R56 round 2: authoritative GRQ-locked heal for the two orphan shapes
/// `wake_up_enqueue` cannot fix on its own:
///
/// (a) QUEUED-UNPICKABLE — the task is linked on its class queue but still
///     carries an on_cpu mark that no slot owns (and no in-flight switch
///     is saving it). The class pick refuses on_cpu-marked tasks (the
///     NEW2 skip: "context not saved yet"), so the task sits counted in
///     nr_running, unpickable forever, and the idle CPUs spin on the
///     R20-2 re-check. Legitimate producers of linked ∧ on_cpu are (i)
///     mid switch-out — excluded because __schedule publishes
///     SWITCHING_OUT before releasing the GRQ and __switch_to clears the
///     mark when the save completes — and (ii) the next == prev fast-path
///     re-pick — excluded because the switching CPU's slot still names
///     the task. Under the GRQ lock neither can present this shape, so
///     the mark is stale: clear it and the task becomes pickable.
///
/// (b) SLOT-CAPTURE — the task is RUNNING, unlinked, and a BUSY slot
///     still names it, but the caller (the idle-loop harvester) has
///     time-confirmed the shape across ticks AND verified the naming
///     CPU has not taken a scheduler tick in seconds: the pick that
///     published the slot never ran its switch and the hart is no longer
///     executing kernel code. Clear the pick marks, re-link the task and
///     rewrite the lying slots to their idle tasks so no wedged loop can
///     re-consume the link.
///
///     R56-heal hardening (gnome-boot pfault family, m25/m26/m27): tick
///     staleness does NOT prove the naming hart is dead — under MTTCG a
///     live vCPU that the host descheduled (or a long IRQ-off stretch)
///     takes no ticks either, and the original steal double-ran such
///     picks at nearly 100% during Xorg start. The steal is now gated by
///     a per-task pick-state machine (Task::ti_pick_state): the pick
///     publishes PICKED under the GRQ, the picking CPU's switch-in
///     affirms it LIVE with an atomic CAS as the last step before
///     restoring the task's registers, and the steal may only revoke
///     (CAS PICKED → REVOKED) a still-pending pick. A starved-but-live
///     picker therefore aborts its resume at the failed CAS (no double
///     run), and a task that already resumed (LIVE) is never touched.
///
/// R57: true when the task is on its way out — PF_EXITING set (do_exit's
/// first store) or a SIGKILL pending. Such a task terminates through its
/// own final schedule(); no heal may re-enqueue or re-offer it (the
/// n1-organic kill chain: gsettings aborted → group SIGKILL → the steal
/// re-linked the mid-do_exit dconf worker → double-run → epc-on-own-stack
/// illegal-instruction panic).
pub fn task_is_dying(task: *mut Task) -> bool {
    use crate::process::task::task_flags::TaskFlags;
    if task.is_null() {
        return true;
    }
    {
        if crate::sched::edges::task_shared(task).ti_flags() & TaskFlags::PF_EXITING.bits() != 0 {
            return true;
        }
        crate::sched::edges::task_shared(task).pending.peek_info(crate::signal::Signal::SIGKILL as i32).is_some()
    }
}


/// Returns true when something was healed.
pub fn harvest_relink(task: *mut Task) -> bool {
    if task.is_null() {
        return false;
    }
    let cpus_allowed;
    let outcome;
    {
        let mut g = grq().lock_irqsave();
        // SAFETY: the caller (idle-loop harvester) passes pid-hash-derived
        // task pointers that the bucket lock pins in the table; all state
        // transitions below run under the GRQ lock.
        {
            let pid = crate::sched::edges::task_shared(task).pid();
            if pid == 0 || pid == TASK_POISON {
                return false;
            }
            let st = crate::sched::edges::task_shared(task).state();
            if !st.is_running() {
                return false;
            }
            cpus_allowed = crate::sched::edges::task_shared(task).cpus_allowed();
            let linked = match crate::sched::edges::task_shared(task).policy() {
                SchedPolicy::Fifo | SchedPolicy::Rr => g.rt_rq.is_linked(task),
                SchedPolicy::Deadline => g.dl_rq.is_linked(task),
                SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                    g.cfs_rq.is_linked(task)
                }
            };

            // (a) queued but unpickable: orphaned on_cpu mark.
            if linked && crate::sched::edges::task_shared(task).on_cpu() && !task_is_mid_switch_out(task) {
                // No live owner may exist: mid switch-out is excluded by the
                // SWITCHING_OUT check above and a fast-path re-pick keeps
                // the switching CPU's slot naming the task — the caller's
                // pre-filter passed only slotless tasks here.
                //
                // R56-heal: the pick state is the authority now. A LIVE
                // task is executing on its picking CPU — its on_cpu mark
                // is NOT stale (clearing it would let a second CPU pick a
                // running task: the m25 double-run shape). A PICKED pick
                // must be REVOKED first so a starved-but-live picker's
                // switch-in aborts at the affirm CAS instead of resuming.
                let st = crate::sched::edges::task_shared(task).pick_state();
                if st == PICK_LIVE {
                    // R80 (LIVE-on-linked stale-mark heal): under the GRQ we
                    // hold, a LINKED task with on_cpu set and NO
                    // SWITCHING_OUT mark is executing NOWHERE. Every leg is
                    // provable: (1) mid switch-out publishes
                    // SWITCHING_OUT[name] before releasing the GRQ —
                    // excluded above; (2) an in-flight pick UNLINKS the task
                    // under this same lock before publishing — a linked task
                    // has no pending pick; (3) a task running as some CPU's
                    // current is named by that CPU's slot until its next
                    // __schedule, which links it only in the mid-switch-out
                    // shape of (1); (4) a LIVE publish (fast-path repick /
                    // R57 forced-LIVE) happens with the publisher's slot
                    // naming the task under this lock. The v1-r1 full-session
                    // capture stranded Xorg in exactly this shape for 30
                    // minutes (linked ∧ {on_cpu=1, claim=1} ∧ LIVE ∧
                    // slotless, every picker skipping why=1, every heal
                    // refusing on LIVE) because the original unconditional
                    // refusal trusted the LIVE word with no way to adjudicate
                    // it. Re-verify the slot scan under THIS lock hold (the
                    // caller's pre-filter raced nothing since it also holds
                    // no slots, but do not trust it), then require the
                    // orphan watch to have time-confirmed the shape for >=1s
                    // — an impossible-in-tree state that PERSISTS is by
                    // definition a stale mark, and leaving it strands the
                    // task forever (the phantom holds whatever lock it took,
                    // wedging everyone else: the EXT4-BIGLOCK stall family).
                    // The m25 double-run fear does not apply: releasing the
                    // qword of a task that provably executes nowhere cannot
                    // hand it to a second hart.
                    if (0..MAX_CPUS).any(|c| cpu_state(c).current == task) {
                        // A slot names it after all (pre-filter raced a
                        // pick's slot publish): genuinely owned — refuse.
                        return false;
                    }
                    if !orphan_watch_confirm(
                        task,
                        pid,
                        crate::timer::get_jiffies(),
                        50,
                    ) {
                        // First (or recent) sighting — come back after the
                        // shape has persisted half a second with a frozen
                        // exec clock (R81: the exec-frozen rule convicts at
                        // any observer cadence; a task genuinely resuming
                        // here is charged by its first tick and refreshes
                        // the watch instead).
                        return false;
                    }
                    crate::sched::edges::task_shared(task).release_pick_ownership();
                    // Rule 1 (grq-lock-1st): event ring, not the console.
                    crate::sched::sched_hooks::sched_ev(
                        crate::sched::sched_hooks::EV_R80_LIVE_MARK,
                        pid,
                        crate::sched::edges::task_shared(task).running_on_cpu.load(
                            core::sync::atomic::Ordering::Acquire,
                        ) as u64,
                        0,
                    );
                    r56_print_pid(
                        b"",
                        crate::sched::edges::task_shared(task).running_on_cpu.load(
                            core::sync::atomic::Ordering::Acquire,
                        ).unsigned_abs(),
                        b"\n",
                    );
                    outcome = true;
                } else {
                    if st == PICK_PICKED && !crate::sched::edges::task_shared(task).revoke_pending_pick() {
                        // Lost the race against the picker's affirm CAS — the
                        // task just went LIVE; not healable.
                        return false;
                    }
                    crate::sched::edges::task_shared(task).release_pick_ownership();
                        // Rule 1 (grq-lock-1st): event ring, not the console.
                        crate::sched::sched_hooks::sched_ev(
                            crate::sched::sched_hooks::EV_R56_QUEUED_MARK,
                            pid,
                            0,
                            0,
                        );
                    outcome = true;
                }
            } else if linked {
                return false;
            } else if task_is_mid_switch_out(task) {
                // Someone is genuinely saving this context right now (or a
                // stale SWITCHING_OUT lingered — the harvester's watch
                // table time-confirms before retrying).
                return false;
            } else {
                // (b) slot-capture steal: rewrite every naming slot to its
                // idle task, clear the pick marks, and re-link.
                //
                // R56-heal (affirm-gated steal): the revocation CAS
                // (PICKED → REVOKED) is the ONLY gate. It succeeds
                // exclusively while the picking CPU is still between the
                // GRQ release and __switch_to's affirm — a picker that is
                // merely TCG-starved (not dead) aborts the resume at the
                // failed CAS and repairs itself. Once the picker affirmed
                // LIVE, the task's registers are (or are about to be)
                // restored on that hart and no steal may touch it — tick
                // staleness proves nothing under MTTCG (the gnome-boot
                // pfault family, m25/m26/m27: heal cpu=2 pid=311, instant
                // KERNPANIC, epc/ra inside the task's own kernel stack).
                // R56-STEAL tripwire (cold path — most boots never steal):
                // the pre-revoke ownership picture and the post-enqueue
                // outcome. st must read PICKED(0); a st=1 (LIVE) read whose
                // CAS still wins means the page was recycled under us (the
                // pb_7 adjudication capture that motivated the lifecycle
                // pin). sw = bitmask of naming CPUs still mid-switch; rq=1
                // means the re-link took.
                let _pre_state = crate::sched::edges::task_shared(task).pick_state();
                let mut _sw_mask: u32 = 0;
                for c in 0..MAX_CPUS {
                    if cpu_state(c).current == task
                        && SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire) != 0
                    {
                        _sw_mask |= 1 << c;
                    }
                }
                // R57 (defense in depth for other callers): a naming CPU
                // whose timer-IRQ count still moves is alive no matter how
                // stale its scheduler tick reads (the n1-organic lesson:
                // starved ≠ dead under MTTCG), and a dying task always
                // leaves through its own final schedule(). Refuse both.
                for c in 0..MAX_CPUS {
                    if cpu_state(c).current == task
                        && crate::debug::softlockup::cpu_irq_frozen_ms(c) < 5000
                    {
                        return false;
                    }
                }
                if task_is_dying(task) {
                    return false;
                }
                if !crate::sched::edges::task_shared(task).revoke_pending_pick() {
                    // LIVE (genuinely running — leave it alone) or an
                    // already-consumed REVOKED marker (a previous heal
                    // pass owns the recovery).
                    return false;
                }
                for c in 0..MAX_CPUS {
                    if cpu_state(c).current == task {
                        let idle_task = cpu_state(c).idle;
                        if !idle_task.is_null() && idle_task != task {
                            cpu_state_mut(c).current = idle_task;
                            slot_tag_write(c, SLOT_W_STEAL, idle_task);
                            // r11: the revoked pick's generation is
                            // superseded (the aborted switch-in's
                            // republish would supersede again — harmless).
                            pick_gen_supersede(c, idle_task);
                                // Rule 1 (grq-lock-1st): event ring, not
                                // the console (pid = the healed task, a =
                                // the slot's cpu).
                                crate::sched::sched_hooks::sched_ev(
                                    crate::sched::sched_hooks::EV_R56_SLOT_CAPTURE,
                                    pid,
                                    c as u64,
                                    0,
                                );
                            r56_print_pid(b"", pid, b"\n");
                        }
                    }
                }
                // R56-heal: revoking the pick's ownership qword is what the
                // picking CPU's switch-in validates — if that vCPU is
                // merely TCG-starved mid-switch (not dead), its
                // __switch_to sees claim != my-cpu and aborts the resume
                // (riscv_switch_abort / x86_switch_abort repair the slot
                // and page table), so this steal cannot double-run it.
                crate::sched::edges::task_shared(task).release_pick_ownership();
                outcome = enqueue_task_locked(&mut g, crate::sched::edges::task_mut(task));
                // R56-STEAL diagnostics tail: st/sw/rq on ONE line.
                // Rule 1 (grq-lock-1st): event ring, not the console
                // (a packs pre_state|sw_mask, b packs rq|outcome).
                crate::sched::sched_hooks::sched_ev(
                    crate::sched::sched_hooks::EV_R56_STEAL,
                    pid,
                    ((_pre_state as u64) << 32) | _sw_mask as u64,
                    outcome as u64,
                );
            }
        }
    }
    // Nudge an idle CPU outside the lock (mirrors enqueue_task's tail).
    if outcome {
        if let Some(idle_cpu) = grq().find_idle_cpu(cpus_allowed) {
            grq().clear_idle(idle_cpu);
            resched_cpu(idle_cpu);
        }
    }
    outcome
}

/// R56-heal (switch-in abort repair): the slot-capture steal revoked a
/// pick that was still in flight (a TCG-starved vCPU between the GRQ
/// release and __switch_to's register restore), and that vCPU's switch
/// just aborted the resume. The steal rewrote this CPU's slot to its
/// idle task, but the task actually continuing here is `prev` —
/// republish the truth so the slot scans (harvest pre-filters, wake-side
/// curr_on checks, task dumps) do not classify prev as a slotless
/// orphan while it keeps running.
///
/// Called from the arch abort handlers (riscv_switch_abort /
/// x86_switch_abort) with IRQs disabled and no scheduler locks held;
/// takes the GRQ alone (no pid-hash bucket — complies with the
/// bucket → GRQ order the harvester itself uses).
pub fn republish_current_after_abort(cpu: usize, prev: *mut Task) {
    if prev.is_null() || cpu >= MAX_CPUS {
        return;
    }
    let _g = grq().lock_irqsave();
    // SAFETY: prev is the task executing on this CPU right now (its
    // context was never handed over); the slot write runs under the GRQ.
    {
        if cpu_state(cpu).current != prev {
            cpu_state_mut(cpu).current = prev;
            slot_tag_write(cpu, SLOT_W_ABORT_REPUB, prev);
            // r11: the aborted switch-in's generation — the pick whose
            // claim was revoked mid-flight — never materialized and never
            // will; supersede it (prev, which keeps running, is the new
            // ledger content).
            pick_gen_supersede(cpu, prev);
        }
        if crate::sched::edges::task_shared(prev).pid() == 0 {
            // Continuing the idle task: restore the idle bit our pick
            // cleared (idempotent with the idle loop's own mark_idle).
            grq().mark_idle(cpu);
        }
    }
}


/// R56: re-link picked-but-lost tasks (multi-mode emulator-race orphan).
/// Runs from the idle loop. A task is an orphan when it is RUNNING, not
/// linked on any class queue, and no live CPU owns it. Such a task will
/// never be picked again and nothing will wake it — every recorded capture
/// of the multi-thread pipe hang had exactly this shape.
///
/// R56 round 2 (this rewrite) closes the three blind spots that made the
/// original harvester miss permanently:
///
/// 1. The nr_running != 0 hard gate: the harvest only ran when the queue
///    read empty. A queued-unpickable orphan (stale on_cpu mark) KEEPS
///    nr_running above zero, so the gate made that shape permanent; and
///    every other orphan had to wait for the whole queue to drain before
///    anything looked for it (the f05rep2 corruption window). The scan is
///    now rate-limited under load instead of suppressed.
/// 2. The on_cpu() predicate requirement: a RUNNING ∧ unlinked ∧ !on_cpu
///    phantom owned by no slot (the R46 smash family) is skipped by the
///    pre-filter — and since no wake can reach a task that is not waiting
///    on anything, nothing else ever healed it. The pre-filter now routes
///    every slotless RUNNING-unlinked task through wake_up_enqueue, whose
///    GRQ-locked classification is authoritative.
/// 3. The slot-capture family (a busy slot still names the orphan): the
///    outer check admitted it but wake_up_enqueue's curr_on scan refused
///    it — silently, forever (the ftest01 permanent-hang shape). Live CPUs
///    now self-heal (R56d in __schedule), lying idle-marked slots are
///    healed inside wake_up_enqueue (R56c), and a truly dead hart is
///    handled here by time-confirmed steal (harvest_relink).
fn harvest_orphan_tasks(_my_cpu: usize) {
    // Rate gate (probe-slow tightening). The loaded case keeps e90bc010's
    // discipline: at most ONE scan per 2 jiffies system-wide (a CAS on the
    // last-scan jiffy makes one CPU the scanner). The EMPTY case used to
    // run the pid-hash walk on EVERY idle-loop pass — defensible when a
    // pass meant "about to WFI for a long while", but each 10ms timer tick
    // wakes both idle CPUs, so a quietly idle 2-vCPU system still paid
    // ~200 full 256-bucket walks per second, pure overhead under TCG
    // single-thread. An empty queue now scans at most once per jiffy
    // system-wide (same CAS, interval 1): worst-case orphan-detection
    // latency on an idle system becomes ~10-20ms — two orders below the
    // harvester's own time-confirmation thresholds (3-jiffy watch, 5s
    // stale-tick), so no heal is delayed meaningfully.
    static LAST_LOADED_SCAN: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let scan_interval: u64 = if GlobalRunQueue::grq_nr_running() != 0 {
        2
    } else {
        1
    };
    {
        let now = crate::timer::get_jiffies();
        let last = LAST_LOADED_SCAN.load(core::sync::atomic::Ordering::Acquire);
        if now.wrapping_sub(last) < scan_interval {
            return;
        }
        if LAST_LOADED_SCAN
            .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
            .is_err()
        {
            return;
        }
    }
    let now = crate::timer::get_jiffies();
    crate::process::pid_hash::pid_hash_for_each_task_try(|t| {
        // Helper APIs (heals, watch, relink) still take *mut Task.
        let t = t as *const Task as *mut Task;
        let pid = crate::sched::edges::task_shared(t).pid();
        // Idle tasks are RUNNING ∧ unlinked by design; freed pages must
        // not be touched further.
        if pid == 0 || pid == TASK_POISON {
            return;
        }
        // R56-heal (lifecycle pin): the scan dereferences — and its heals
        // WRITE — this task page far beyond the bucket-lock section this
        // walk holds. An exiting task can finish pid_hash_remove +
        // synchronize_rcu + task_put in that window and fork reuses the
        // page mid-heal (the m25 adjudication capture: a steal whose
        // pre-revoke read saw LIVE won its CAS only because new_task_at
        // had rewritten ti_pick_state on the recycled page; the heal then
        // linked the half-built child — epc=0x0 double panic). Pinning
        // defers the free past every heal below; the heals' own RUNNING
        // re-checks refuse a task that exits under the pin.
        if !Task::try_pin(t) {
            return;
        }
        let heal_body = || {
        if !crate::sched::edges::task_shared(t).state().is_running() {
            return;
        }
        let onrq = task_onrq_flag(crate::sched::edges::task_shared(t));
        let oncpu = crate::sched::edges::task_shared(t).on_cpu();
        let slot = (0..MAX_CPUS).find(|&c| cpu_state(c).current == t as *const Task as *mut Task);
        if !onrq {
            match slot {
                None => {
                    // Slot moved on (the classic R56 shape) or the mark was
                    // never set (R46 family): wake_up_enqueue's GRQ-locked
                    // phantom path verifies linkage/ownership and heals.
                    if wake_up_enqueue(t)
                        && crate::trace::EVENT_SCHEDTRACE.armed()
                    {
                        // R80 (diag): name the healed pid — the v1-r1 wedge
                        // printed hundreds of anonymous harvest lines while
                        // the actual blocker (a LIVE-mark phantom) was
                        // refused elsewhere and never named.
                        r56_print_pid(b"R56-ORPHAN-HARVESTED pid=", pid, b"\n");
                        // debug-migrate: the harvest also records into the
                        // unified framework (rux:schedtrace
                        // kind=ORPHAN_HARVEST).
                        crate::trace::trace_schedtrace(2, pid as u32, 0, 0);
                        // R80 (diag, producer hunt): the v2 storm showed
                        // pids 306/336/338 cycling orphan -> heal -> orphan
                        // every ~200ms through THIS plain-insert path (no
                        // R49/R80 print — their marks were already clean).
                        // Print the victim's parked context, rate-limited:
                        // the saved thread.sp's caller frames name the exact
                        // syscall/wait site the context was stranded in, and
                        // repeated identical frames = one producer.
                        harvest_print_parked(t, pid);
                    }
                }
                Some(c) => {
                    let slot_idle = grq()
                        .idle_cpus
                        .load(core::sync::atomic::Ordering::Acquire)
                        & (1u32 << c)
                        != 0;
                    if slot_idle {
                        // Lying idle-marked slot — R56c inside
                        // wake_up_enqueue heals the slot and admits the
                        // re-link.
                        if wake_up_enqueue(t) {
                            if crate::trace::EVENT_SCHEDTRACE.armed() {
                                r56_print_pid(b"R56-ORPHAN-HARVESTED pid=", pid, b"\n");
                                // debug-migrate: rux:schedtrace
                                // kind=ORPHAN_HARVEST (slot-resync face).
                                crate::trace::trace_schedtrace(2, pid as u32, c as u64, 0);
                            }
                        }
                    } else if crate::sched::edges::task_shared(t).preempt_count() == 0
                        && orphan_watch_confirm(t, pid, now, 3)
                        && cpu_tick_stale_ms(c) >= 5000
                        // R57 (n1-organic postmortem): tick-staleness alone
                        // convicted a TCG-starved-but-alive hart — CPU3
                        // serviced 3-10 timer IRQs/s for 1600s while its
                        // scheduler tick read stale; the steal re-enqueued
                        // pid 365 mid-do_exit, another hart resumed its
                        // live kernel stack, and the machine died at
                        // epc=sp+0x26 (illegal instruction, the m25 shape).
                        // A hart whose timer-IRQ count still MOVES is alive:
                        // refuse the steal. Only a count frozen for as long
                        // as the tick is stale may be read as a dead picker.
                        && crate::debug::softlockup::cpu_irq_frozen_ms(c) >= 5000
                        // A dying task (PF_EXITING or SIGKILL pending)
                        // terminates through its own final schedule();
                        // re-enqueueing it mid-exit hands a live kernel
                        // stack to a second hart.
                        && !task_is_dying(t)
                    {
                        // Busy slot naming a task that is off-queue while
                        // the naming CPU has not taken a tick in 5s and the
                        // shape persisted 3+ jiffies: the pick that set the
                        // slot is dead, not in flight. A healthy runner is
                        // excluded by every leg: it keeps its CPU ticking,
                        // gets switched out within a slice, and a lock
                        // holder carries a nonzero preempt_count.
                        if harvest_relink(t) {
                            orphan_watch_clear(t);
                        }
                    }
                }
            }
        } else if oncpu && slot.is_none() {
            // Linked but unpickable (orphaned on_cpu mark, NEW2 skip).
            // Mid switch-out is the one legitimate producer — its
            // SWITCHING_OUT mark is published under the GRQ; time-confirm
            // before clearing through a mark that outlived its switch.
            if !task_is_mid_switch_out(t) || orphan_watch_confirm(t, pid, now, 3) {
                if harvest_relink(t) {
                    orphan_watch_clear(t);
                }
            }
        }
        };
        heal_body();
        Task::task_put(t);
    });
}


/// r24 (kill10 wedge, LTP lane): idle-loop own-slot heal.
///
/// The executing context is this CPU's IDLE task (the only caller is
/// cpu_idle_loop, after its schedule() returned) — that fact alone makes
/// the CPU the sole authority on what it is running. If this CPU's slot
/// names a REAL task at that point, the name is provably stale: every
/// in-tree path that publishes a real task into a slot does so from
/// that CPU's own pick inside schedule(), and if the pick had switched,
/// control would not be back in the idle loop. The named task is left
/// RUNNING/unlinked with every future wake refused by the curr_on scan
/// (the WAKE-HEAL-SKIP why=c / W2 family) — permanent on x86_64, where
/// the pid-hash orphan harvester is gated off (the x86-scribble4 steal
/// crash) and R56c only heals IDLE-marked slots. kill10's fork/exit
/// signal storm produces this shape within seconds and parks the whole
/// process graph; the r23 census carries it as the kill10 FAIL.
///
/// Heal deterministically (no time heuristics, no steal of a possibly
/// live task — the executing idle task IS the proof the slot lies):
/// rewrite the slot to the idle task under the GRQ, return the un-named
/// task PINNED (caller requeues it via wake_up_enqueue and task_put's
/// the pin). Foreign slot writers all hold the GRQ, so the re-read
/// under the lock closes the race with a concurrent repair.
pub fn idle_heal_own_slot(cpu: usize) -> *mut Task {
    if cpu >= MAX_CPUS {
        return core::ptr::null_mut();
    }
    let idle_task = cpu_state(cpu).idle;
    if idle_task.is_null() {
        return core::ptr::null_mut();
    }
    let mut victim: *mut Task = core::ptr::null_mut();
    let mut victim_pid = 0u32;
    let mut prev_tag = 0u32;
    {
        let _g = grq().lock_irqsave();
        let curr = cpu_state(cpu).current;
        if curr.is_null() || curr == idle_task || crate::sched::edges::task_shared(curr).pid() == 0 {
            return core::ptr::null_mut();
        }
        prev_tag = SLOT_WRITER[cpu].load(core::sync::atomic::Ordering::Relaxed);
        victim_pid = crate::sched::edges::task_shared(curr).pid();
        // Pin the page before dropping the slot's implicit pin; if the
        // page is at its last refcount (exiting), still rewrite the
        // stale name but do not hand a dead page to the wake path.
        if !crate::process::task::Task::try_pin(curr) {
            cpu_state_mut(cpu).current = idle_task;
            slot_tag_write(cpu, SLOT_W_IDLE_HEAL, idle_task);
            // r11: the provably-stale name's generation is superseded.
            pick_gen_supersede(cpu, idle_task);
            return core::ptr::null_mut();
        }
        cpu_state_mut(cpu).current = idle_task;
        slot_tag_write(cpu, SLOT_W_IDLE_HEAL, idle_task);
        pick_gen_supersede(cpu, idle_task);
        // r-strand (lat_fs conviction round, 2026-11-19): the victim may
        // be CURRENT on ANOTHER cpu — a task is named by exactly one
        // slot; a second naming cpu holds a stale slot from a switch
        // whose publish was lost. Requeueing a live-owned victim
        // inserts a GHOST NODE (linked while running elsewhere — every
        // later pick refuses it, the tree degrades, the strand
        // re-forms). Rewrite THIS slot only; the live owner keeps the
        // task. (The dump evidence: cpu slots `0 319 319 0 ...` — two
        // slots naming one worker.)
        let mut live_elsewhere = false;
        for c in 0..MAX_CPUS {
            if c != cpu
                && (cpu_state(c).current == curr
                    || (SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire)
                        == curr as u64
                        && {
                            let born = SWITCHING_OUT_JIF[c].load(
                                core::sync::atomic::Ordering::Acquire,
                            );
                            born != 0
                                && crate::timer::get_jiffies()
                                    .wrapping_sub(born)
                                    < SWITCHING_OUT_STALE_JIF
                        }))
            {
                live_elsewhere = true;
                break;
            }
        }
        if !live_elsewhere {
            victim = curr;
        }
    }
    // Census (rate-limited): the healed pid + the tag of the writer that
    // left the stale name — the producer hunt's anchor.
    static HEAL_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if HEAL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 32 {
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
        crate::pr_warn!(
            "SLOT-LIE-HEALED cpu={} pid={} prev_writer_tag={}",
            cpu,
            victim_pid,
            prev_tag
        );
    }
    victim
}

/// lmbench-fix lane (lat_sem / lat_connect / bw_tcp wedge hunt): heal the
/// UNLINKED RUNNING phantom — a task that is state=RUNNING, linked on no
/// class queue, and owned by no CPU (no current slot names it, no
/// in-flight SWITCHING_OUT). In the reference kernel this state cannot
/// exist: a context switch leaves prev either linked on its class queue
/// (still runnable) or in a sleep state, and a pick publishes the slot
/// and the ownership marks inside one GRQ hold. Here the state arrives
/// from the phantom-preempt-count / stranded-ownership family (organic
/// captures: lmbench lat_sem smp2 — the benchmp controller frozen as
/// {RUNNING, unlinked, slotless} with the worker pair parked; every
/// future wake of it is refused by the curr_on scan it fails, no pick
/// ever scans it because it is not linked, and the benchmark times out).
///
/// Adjudication is STRUCTURAL, not time-based (the R56 orphan harvester's
/// 5 s staleness steal is what keeps that harvester riscv64-only after
/// the x86-scribble4 double-run crash): under the GRQ the ownership
/// protocol makes "is on a CPU" decidable — no CPU can be mid-pick while
/// we hold the lock, every completed pick has published current == picked
/// in the same hold, and a genuinely running task's slot names it from
/// its pick until its next __schedule. A RUNNING task that is unlinked,
/// unnamed by every slot, not SWITCHING_OUT-marked and not exiting is
/// provably executing nowhere — relink it through the same insert pair
/// the wake path uses. This is the unlinked twin of R85's pick-side heal
/// (which can only reach LINKED candidates) and needs no concurrent wake
/// (unlike the R46/R49 heals inside wake_up_enqueue).
pub fn relink_unlinked_running_phantoms() -> u32 {
    use crate::process::task::task_flags::TaskFlags;
    const MAXCAND: usize = 16;
    let mut cand: [*mut Task; MAXCAND] = [core::ptr::null_mut(); MAXCAND];
    let mut n = 0usize;
    crate::process::pid_hash::pid_hash_for_each_task_try(|t| {
        if n >= MAXCAND {
            return;
        }
        {
            if t.pid() == 0 || t.pid() == u32::MAX {
                return;
            }
            if t.state() != TaskState::new(TaskState::RUNNING) {
                return;
            }
            if t.ti_flags() & TaskFlags::PF_EXITING.bits() != 0 {
                return;
            }
            // Pin against a concurrent exit freeing the page between this
            // walk and the GRQ re-verify below (same discipline as
            // idle_heal_own_slot). The candidate table stores raw
            // identity — recover the pointer from the walk reference.
            let t = t as *const Task as *mut Task;
            if !crate::process::task::Task::try_pin(t) {
                return;
            }
            cand[n] = t;
            n += 1;
        }
    });

    let mut healed = 0u32;
    let mut report_pid = 0u32;
    let mut report_pc = 0i32;
    // PROBE-LOCK DISCIPLINE (grq-lock-1st): the reports below used to be
    // issued INSIDE this GRQ hold. printk's ring-buffer write ends in
    // `KMSG_READ_WQ.wake_up_all()`, whose waitqueue lock is held across
    // `wake_up_process` -> the GRQ acquire — a report issued under the GRQ
    // re-acquires the GRQ on this cpu (self-deadlock with IRQs off) or
    // forms the GRQ->WQ / WQ->GRQ ABAA with another cpu. Capture the
    // fields here, emit after the guard is released.
    let mut pending: [GrqPendingReport; 8] =
        [GrqPendingReport { kind: 0, a: 0, b: 0, c: 0, d: 0, e: 0, comm: [0; 16] }; 8];
    let mut npending = 0usize;
    {
        let mut g = grq().lock_irqsave();
        // r-strand face (a) (lat_fs conviction, 2026-11-19): the same
        // sweep family as the unlinked-RUNNING phantom below, for the
        // LINKED victim: a counter/timeline desync (counter reads 0
        // while the tree holds the task) WFI-halts every idle CPU on
        // the nr_running==0 fast path and the linked task is never
        // picked. Resync from the tree truth; the stranded task is
        // then picked on the next idle pass. (The diag walk caps at
        // 999; a capped count still reads >0, which is all the idle
        // gate needs — an over-count is benign, the under-count was
        // the killer.)
        if let Some((was, now)) = g.cfs_rq.resync_nr_running_from_timeline() {
            static CR_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if CR_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 32 && npending < 8 {
                pending[npending] = GrqPendingReport {
                    kind: GRQ_RPT_COUNTER_RESYNC,
                    a: was,
                    b: now,
                    c: 0,
                    d: 0,
                    e: 0,
                    comm: [0; 16],
                };
                npending += 1;
            }
            healed += 1;
        }
        for i in 0..n {
            let task = cand[i];
            if task.is_null() {
                continue;
            }
            // SAFETY: the pin taken in the walk holds the page alive
            // through this section; every field read below is under the
            // GRQ (the same serialization the wake path relies on).
            {
                let tk = crate::sched::edges::task_shared(task);
                // Re-verify everything under the lock: the task may have
                // parked, exited, been picked or linked since the walk.
                if tk.pid() == 0 || tk.pid() == u32::MAX {
                    continue;
                }
                let st = tk.state();
                if st != TaskState::new(TaskState::RUNNING) || st.is_dead() {
                    continue;
                }
                if tk.ti_flags() & TaskFlags::PF_EXITING.bits() != 0 {
                    continue;
                }
                if task_is_mid_switch_out(task) {
                    continue;
                }
                // r-strand: the relink must ALSO retire an aged
                // switch-out mark and clear the task's stale ownership
                // qword — otherwise the next pick refuses it (oncpu=1
                // claim=X with no live owner) and the strand just
                // re-forms. The captured face: worker switched out
                // mid-creat, mark unretired, qword stale, machine idle
                // around it. (task_is_mid_switch_out above now reads
                // aged marks as NOT in flight, so we get here.)
                {
                    let now = crate::timer::get_jiffies();
                    for c in 0..MAX_CPUS {
                        if SWITCHING_OUT[c].load(core::sync::atomic::Ordering::Acquire)
                            == task as u64
                        {
                            let born = SWITCHING_OUT_JIF[c]
                                .load(core::sync::atomic::Ordering::Acquire);
                            if born == 0
                                || now.wrapping_sub(born) >= SWITCHING_OUT_STALE_JIF
                            {
                                SWITCHING_OUT[c]
                                    .store(0, core::sync::atomic::Ordering::Release);
                                SWITCHING_OUT_JIF[c]
                                    .store(0, core::sync::atomic::Ordering::Release);
                                static SR_N: core::sync::atomic::AtomicU32 =
                                    core::sync::atomic::AtomicU32::new(0);
                                if SR_N
                                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                                    < 24
                                    && npending < 8
                                {
                                    pending[npending] = GrqPendingReport {
                                        kind: GRQ_RPT_SHOUT_RETIRED,
                                        a: c as u64,
                                        b: tk.pid() as u64,
                                        c: 0,
                                        d: 0,
                                        e: 0,
                                        comm: [0; 16],
                                    };
                                    npending += 1;
                                }
                            }
                        }
                    }
                    // The qword: clear when it names an owner this
                    // sweep just proved absent (the `owned` scan ran
                    // under the same GRQ hold).
                    let q = crate::sched::edges::task_ownership_qword(task);
                    let q_oncpu = (q & 0xFF) != 0;
                    let q_claim = (q >> 32) as i32;
                    if q_oncpu || q_claim != -1 {
                        crate::sched::edges::task_shared(task).release_pick_ownership();
                    }
                }
                let mut owned = false;
                for c in 0..MAX_CPUS {
                    if cpu_state(c).current == task {
                        owned = true;
                        break;
                    }
                }
                if owned {
                    continue;
                }
                let linked = match tk.policy() {
                    SchedPolicy::Fifo | SchedPolicy::Rr => g.rt_rq.is_linked(task),
                    SchedPolicy::Deadline => g.dl_rq.is_linked(task),
                    _ => g.cfs_rq.is_linked(task),
                };
                if linked {
                    // r-strand ID scan (lat_fs conviction round): a
                    // LINKED, RUNNING task whose last pick-trace is
                    // stale (or absent) is the seed-shape face —
                    // name it so the next reproducer boot identifies
                    // which mechanism strands it (stale qword /
                    // counter desync / never-picked-at-all). The two
                    // heals above fix their own faces; this keeps
                    // ANY residual shape visible at 1 Hz. Rate-capped.
                    static SID_N: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    if SID_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 24 {
                        let (ago, traced) = match crate::sched::sched::pick_trace_peek(task) {
                            Some((_, a)) => (a, true),
                            None => (u64::MAX, false),
                        };
                        if ago > 200 && npending < 8 {
                            let q = crate::sched::edges::task_ownership_qword(task);
                            let cb = tk.comm();
                            let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                            let mut comm = [0u8; 16];
                            comm[..cl.min(16)].copy_from_slice(&cb[..cl.min(16)]);
                            pending[npending] = GrqPendingReport {
                                kind: if traced {
                                    GRQ_RPT_STRAND_ID
                                } else {
                                    GRQ_RPT_STRAND_ID_NOTRACE
                                },
                                a: tk.pid() as u64,
                                b: st.bits() as u64,
                                c: q,
                                d: tk.pick_state() as u64,
                                e: ago,
                                comm,
                            };
                            npending += 1;
                        }
                    }
                    continue;
                }
                // Same insert pair as wake_up_enqueue_opt: plain insert,
                // then the R41 stale-flag heal if the insert refused.
                let ok = enqueue_task_locked(&mut *g, crate::sched::edges::task_mut(task))
                    || ensure_linked_locked(&mut *g, crate::sched::edges::task_mut(task));
                if ok {
                    healed += 1;
                    if report_pid == 0 {
                        report_pid = tk.pid();
                        report_pc = tk.preempt_count();
                    }
                }
            }
        }
    }
    // Emit the reports captured under the GRQ — AFTER the guard is
    // released (grq-lock-1st: a printk under the GRQ re-acquires the GRQ
    // through the printk ring buffer's kmsg wake).
    emit_grq_reports(&pending[..npending]);
    // Drop the pins outside the GRQ (task_put may free).
    for i in 0..n {
        if !cand[i].is_null() {
            crate::process::task::Task::task_put(cand[i]);
        }
    }
    // Nudge an idle CPU so the relinked task is picked promptly (same
    // tail as the wake path's enqueue).
    if healed > 0 {
        static PR_N: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if PR_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 32 {
        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above); polled-UART cost bounded by the budget.
            crate::pr_warn!(
                "PHANTOM-RELINK n={} pid={} pc={:#x}",
                healed,
                report_pid,
                report_pc
            );
        }
        if let Some(idle_cpu) = grq().find_idle_cpu(u32::MAX) {
            grq().clear_idle(idle_cpu);
            resched_cpu(idle_cpu);
        }
    }
    healed
}

pub fn cpu_idle_loop() -> ! {
    use crate::arch;

    if !crate::arch::smp::is_boot_hart() {
        crate::arch::trap::enable_timer_interrupt();
    }

    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    // SCRIBBLE2-GUARD (idle depth): the idle loop's frame position is
    // fixed for the life of the CPU — every pass must return to the same
    // sp.  A monotonic drop means each resume lands deeper than the
    // matching switch-out saved (double-resume / wrong resume point: the
    // engine that burns whole stacks in small strides and finally
    // overflows into the neighboring allocations).
    static IDLE_SP_FLOOR: [core::sync::atomic::AtomicU64; MAX_CPUS] =
        [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];
    // IRREDUCIBLE F-ROW (F-REG-2) for all three arch arms below: the
    // live stack pointer has no safe read — the reference kernel keeps
    // a `current_stack_pointer` definition per arch for exactly this
    // (arch/x86/include/asm/stacktrace.h, riscv/entry.S's sp, arm64's
    // current_stack_pointer global); Rust's equivalent is this asm!
    // read. Consumer: the idle-stack-overflow guard (same shape as the
    // reference's TASK_SIZE-bounded stack check). Contract: read-only,
    // nomem + nostack; the output operand is the only effect.
    #[cfg(feature = "x86_64")]
    fn idle_guard_sp() -> usize {
        let sp: usize;
        // F-REG-2 (x86_64): mov rsp.
        unsafe { core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack)) };
        sp
    }
    #[cfg(feature = "riscv64")]
    fn idle_guard_sp() -> usize {
        let sp: usize;
        // F-REG-2 (riscv64): mv sp.
        unsafe { core::arch::asm!("mv {}, sp", out(reg) sp, options(nomem, nostack)) };
        sp
    }
    #[cfg(feature = "aarch64")]
    fn idle_guard_sp() -> usize {
        let sp: usize;
        // F-REG-2 (aarch64): mov sp.
        unsafe { core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack)) };
        sp
    }
    fn idle_guard_report(cpu: usize, sp: u64, floor: u64) {
        use crate::debug::taskdump::putc_brief as putchar;
        fn ph(mut v: u64) {
            for _ in 0..16 {
                let n = (v >> 60) as u8;
                putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                v <<= 4;
            }
        }
        const M: &[u8] = b"\nIDLE-DEPTH-DROP cpu=";
        for &b in M {
            putchar(b);
        }
        ph(cpu as u64);
        const M2: &[u8] = b" sp=0x";
        for &b in M2 {
            putchar(b);
        }
        ph(sp);
        const M3: &[u8] = b" floor=0x";
        for &b in M3 {
            putchar(b);
        }
        ph(floor);
        const M4: &[u8] = b" delta=0x";
        for &b in M4 {
            putchar(b);
        }
        ph(floor.wrapping_sub(sp));
        putchar(b'\n');
        #[cfg(feature = "x86_64")]
        crate::debug::scribble::ring_dump();
    }

    loop {
        // Idle-depth guard: measure at the same point every pass.
        {
            let sp = idle_guard_sp() as u64;
            let floor = IDLE_SP_FLOOR[cpu_id].load(core::sync::atomic::Ordering::Relaxed);
            if floor == 0 {
                IDLE_SP_FLOOR[cpu_id].store(sp, core::sync::atomic::Ordering::Relaxed);
            } else if sp + 0x400 < floor {
                idle_guard_report(cpu_id, sp, floor);
                // Re-baseline so the next report names the NEXT drop.
                IDLE_SP_FLOOR[cpu_id].store(sp, core::sync::atomic::Ordering::Relaxed);
            }
        }

        // R56 orphan harvest: the multi-thread gate still produces (rarely)
        // tasks left RUNNING + off-queue after pick — every in-tree
        // pick→switch path is atomic under SIE=0, so the surviving suspects
        // are emulator-model races (form-A family, now under true
        // parallelism). An idle CPU has nothing to lose: re-link any such
        // task whose owning slot is NOT actually running it (slot current
        // is idle or someone else). Genuine on-CPU tasks are skipped by
        // the slot check; re-linked ones print a one-shot tripwire.
        //
        // x86-scribble4 gate: riscv64 only. The harvest's steal decision
        // is a TIME heuristic (5s scheduler-tick staleness via softlockup
        // TOUCH_TS, 3-jiffy shape persistence) calibrated on the riscv64
        // TCG hosts the R56 forensics ran on. On x86_64 q35/MTTCG under
        // concurrent host load a healthy vCPU is regularly starved past
        // the 5s gate; the harvester then STEALS a task that is alive on
        // its CPU (capture: "R56-SLOT-CAPTURE-HEALED cpu=2 pid=1" —
        // pid 1 legitimately cycling on cpu 2 — followed within the same
        // tick by the scribble crash chain: BAD-FRAME rip=02ffffff80444b60,
        // #UD, byte-7-corrupted stack pointers). The steal re-opens the
        // double-run window R65 closed. Until the confirmation gates are
        // tuned for x86 timing, the harvest stays riscv64-only; R56c
        // (lying-slot heal, refused unless idle-marked and unlinked) and
        // R56d (own-slot resync) remain active on both arches.
        #[cfg(feature = "riscv64")]
        {
            harvest_orphan_tasks(cpu_id);
        }

        // Ensure IRQs are enabled before each schedule() call.
        // When the idle task is switched back to (from a task that called
        // schedule() with SIE=0, e.g. from syscall context), __schedule's
        // restore_irq restores SIE=0. Without re-enabling here, no timer
        // interrupts would fire and the system would be stuck.
        // This matches the reference implementation where the idle loop's
        // cpuidle enables IRQs before entering the idle state.
        crate::arch::cpu::restore_irq(true);

        // 1. Try to pick a task from the global RQ
        // SAFETY: called from idle task context; schedule() handles its own locking.
        let my_cpu = crate::arch::cpu_id() as usize;
        // WW_IDLE_PHASE 1: about to enter schedule() (may switch away and
        // back; the stamp survives until the next phase write).
        ww_idle_phase_set(cpu_id, 1);
        // tick_nohz_idle_exit: leaving the idle section — the picked task
        // (if any) runs with inidle clear, so its irq exits take the
        // full_update_tick path (the reference's idle loop brackets its
        // idle section with the same enter/exit pair).
        crate::timer::nohz::tick_nohz_idle_exit();
        {
            schedule();
        }

        // 2. Check if we're still running idle
        let is_idle = {
            let pcpu = this_cpu();
            let curr = pcpu.current;
            !curr.is_null() && crate::sched::edges::task_shared(curr).pid() == 0
        };

        // tick_nohz_idle_enter: back in the idle section (schedule
        // returned to the idle task) — the IRQ exits from here on take
        // the start-idle stop path.
        if is_idle {
            crate::timer::nohz::tick_nohz_idle_enter();
        }

        // Lost-park lie detector (fix-lostpark lane, rux:lostpark (retired debug=lostpark)): this
        // CPU just ran schedule() and is about to idle-mark/halt, but its
        // slot names a REAL task. With an empty global queue that task is
        // registered-nowhere and "running-nowhere" — every wake of it is
        // refused by the curr_on scan (the WAKE-HEAL-SKIP why=c family),
        // so this is exactly the moment the wedge becomes permanent.
        // Report the victim's ownership word and the SLOT_WRITER tag (the
        // last writer of the lying slot: 1=pick 2=R56d 3=R56c 4=steal
        // 5=abort-republish) so the producer is named live.
        if !is_idle
            && crate::trace::EVENT_LOSTPARK.armed()
        {
            lp_idle_lie_report(cpu_id);
        }

        // r24 (kill10 wedge): the same condition, healed instead of only
        // reported. We are the executing idle task of THIS cpu — the
        // slot naming a real task here is provably a stale pick mark.
        // Rewrite it to the idle task and requeue the un-named victim;
        // its parked wake waiters (WAKE-HEAL-SKIP why=c / W2 refusals)
        // then succeed on the next kick. Without this, x86_64 has no
        // healer for the shape at all (harvester gated riscv64-only) and
        // the whole process graph parks forever (kill10, and the
        // both-CPU permanent halt variant).
        let mut is_idle = is_idle;
        if !is_idle {
            let victim = idle_heal_own_slot(cpu_id);
            if !victim.is_null() {
                is_idle = {
                    let pcpu = this_cpu();
                    let curr = pcpu.current;
                    !curr.is_null() && crate::sched::edges::task_shared(curr).pid() == 0
                };
                // Requeue outside the heal's GRQ section (probe-lock
                // discipline): the pin keeps the page alive; the R46
                // phantom path inside re-verifies linkage/ownership
                // under its own lock and refuses a task that exited
                // under the pin.
                wake_up_enqueue(victim);
                crate::process::task::Task::task_put(victim);
            }
        }

        if is_idle {
            grq().mark_idle(cpu_id);
            crate::sync::rcu::rcu_note_context_switch();
            // WW_IDLE_PHASE 2: idle-marked, inside the R20-2 recheck window.
            ww_idle_phase_set(cpu_id, 2);

            // R20-2: a task may have been enqueued between schedule()'s
            // nr_running==0 fast-path read and mark_idle — enqueue_task's
            // find_idle_cpu missed us (the idle bit was not set yet), so no
            // resched IPI was sent and WFI would sleep until the next timer
            // tick (up to 10ms of needless latency). Re-check now: the
            // mark_idle above makes any enqueue after this point visible to
            // find_idle_cpu, so the check-then-WFI sequence is closed.
            if GlobalRunQueue::grq_nr_running() > 0 {
                grq().clear_idle(cpu_id);
                continue;
            }

            // Poll window before halting (closegap lane, 2026-11-02): a
            // wake dispatch to this idle CPU sets NEED_RESCHED and — while
            // POLLING_IDLE is published — SKIPS the resched IPI (see
            // resched_cpu). The poll loop turns the IPI + halt-exit +
            // idle-loop pass (~2-5 us of KVM exit latency per handoff,
            // cctx probe: IPI send alone 935-1139 ns waker-side) into a
            // tens-of-ns spin observe for every idle period shorter than
            // the window — the lmbench ping-pong class (lat_pipe/lat_ctx/
            // lat_sem/lat_unix) idles for microseconds between handoffs.
            // Lost-wake protocol: POLLING_IDLE is cleared BEFORE the final
            // nr_running/need_resched recheck below, and resched_cpu only
            // skips the IPI while the flag is set — a dispatch racing the
            // clear either sees the flag still set (no IPI, but the final
            // recheck observes the enqueue/flag) or sees it clear (sends
            // the IPI, which interrupts the halt). Same shape as the
            // reference's polling-idle / need_resched-before-halt.
            POLLING_IDLE[cpu_id].store(true, core::sync::atomic::Ordering::Release);
            let poll_t0 = crate::timer::monotonic_ns_since_boot();
            // paired-test 2 (perf-r3, 2026-11-03): the 30 us window already
            // absorbs most ping-pong handoffs, but the unix r/w roundtrip
            // anatomy showed IPIs still firing (815/1915 wakes) at a mean
            // 995 ns — handoffs landing just past the window. Widening to
            // 120 us keeps the idler spinning through the remaining
            // near-idle handoffs (each saves the ~1 us IPI+halt-exit
            // exit latency); the window still closes well before a 250 Hz
            // tick. Pure-spin, no allocations.
            //
            // RT-R4 (idlepoll=us boot knob, the reference's idle=poll
            // shape): under KVM every halt costs the fire and pick paths
            // a halt-exit each (the WW6 flat terms: fire ~10 us, pick
            // ~11-13 us at host load 4-6). An RT workload (cyclictest
            // i=1000: ~950 us of idle per period) can spend that idle
            // polling instead — the i=100 rows (no halt crossed) run the
            // whole wake at the 3 us pick class. Default stays 120 us
            // (standard boots keep halt power savings); the RT campaign
            // boots pass idlepoll= to widen it. Parsed once, cached in a
            // static (cmdline::get_param allocates — never on the idle
            // path).
            static IDLE_POLL_WINDOW_NS_CFG: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(u64::MAX);
            let mut window =
                IDLE_POLL_WINDOW_NS_CFG.load(core::sync::atomic::Ordering::Relaxed);
            if window == u64::MAX {
                window = crate::cmdline::get_param("idlepoll")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(|us| us.saturating_mul(1_000))
                    .unwrap_or(120_000);
                IDLE_POLL_WINDOW_NS_CFG.store(window, core::sync::atomic::Ordering::Relaxed);
            }
            let IDLE_POLL_WINDOW_NS: u64 = window;
            while crate::timer::monotonic_ns_since_boot()
                .saturating_sub(poll_t0)
                < IDLE_POLL_WINDOW_NS
            {
                if need_resched()
                    || GlobalRunQueue::grq_nr_running() > 0
                {
                    break;
                }
                core::hint::spin_loop();
            }
            POLLING_IDLE[cpu_id].store(false, core::sync::atomic::Ordering::Release);
            // Dekker pairing fence (see resched_cpu): orders this
            // flag-clear before the recheck loads below against the
            // waker's NEED_RESCHED-store / flag-load sequence — at least
            // one side of the race observes the other, so a skipped IPI
            // is always covered by this recheck and a sent IPI always
            // interrupts the halt.
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

            // Final recheck (closes the poll-clear window; also the R20-2
            // discipline against an enqueue that landed during the poll).
            if need_resched() || GlobalRunQueue::grq_nr_running() > 0 {
                grq().clear_idle(cpu_id);
                continue;
            }

            // Poll UART for pending data before entering WFI.
            // WW_IDLE_PHASE 3: recheck passed, pre-WFI (uart poll window).
            ww_idle_phase_set(cpu_id, 3);
            // closegap: the port read is a VM exit under KVM — only pay it
            // when a console reader is actually parked (interactive boots
            // keep it; benchmark boots with no reader skip one exit per
            // idle pass, one of the idle-side handoff costs).
            if crate::console::read_waitq().has_sleepers()
                && crate::console::uart_has_data()
            {
                crate::console::read_waitq().wake_up_one();
            }

            // printk deferred-console drain (Rule 2, grq-lock-1st): the
            // idle loop runs with IRQs ON and holds no lock — the best
            // context to pay the serial baud time of console lines that
            // printk deferred out of atomic/locked callers. Budget 4 per
            // idle pass; the tick drains 1/tick as the floor, so a busy
            // box still drains at ~100 lines/s (the serial line's own
            // physical rate class).
            if crate::printk::console_emit_pending_flag() {
                crate::printk::console_drain_pending(4);
            }

            // Close the IPI-before-halt window for wake dispatches to an
            // idle CPU (perf-ipc): a resched IPI that lands between the
            // nr_running recheck above and the halt below is SERVICED but
            // consumed — the subsequent WFI would sleep with the task
            // enqueued and the idle bit already cleared by the claim, and
            // nothing wakes us until the next tick. The IPI handler sets
            // our NEED_RESCHED flag; honoring it here (the reference's
            // need_resched-check-before-halt discipline) skips the halt
            // and the loop's schedule() picks the dispatched task.
            if need_resched() {
                grq().clear_idle(cpu_id);
                continue;
            }

            // Enter WFI to halt CPU until next interrupt (timer, UART, IPI).
            // IRQs must be enabled (SIE=1) so timer ticks and wake-ups arrive.
            // Charge the WFI residency to the per-CPU idle accounting that
            // feeds /proc/uptime's second column (P2: real idle stats).
            let idle_t0 = crate::arch::cpu::read_time();
            // WW_IDLE_PHASE 4: about to WFI (the mark is set here — a
            // strand heartbeat reading phase=4 with idle bit CLEAR is the
            // lost-mark shape directly).
            ww_idle_phase_set(cpu_id, 4);
            crate::arch::cpu::wfi();
            // WW_IDLE_PHASE 5: WFI returned, idle accounting + mark clear.
            ww_idle_phase_set(cpu_id, 5);
            crate::fs::perf_ctr::account_idle_cycles(
                cpu_id,
                crate::arch::cpu::read_time().wrapping_sub(idle_t0),
            );

            grq().clear_idle(cpu_id);
        }
    }
}
