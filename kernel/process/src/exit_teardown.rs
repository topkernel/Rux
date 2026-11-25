//! The exit TEARDOWN family, carved out of process/exit.rs for the
//! process service crate (seed-core-design §1a: the unit of the
//! move is the function). do_exit / do_exit_group / wake_group_chldexit
//! stay core-side — the arch traps, signal delivery and the scheduler
//! reap path call them BY NAME (hot sites, direct calls) — and reach
//! THIS family through the ExitHooks rows (once per task death: cold
//! by the call-direction scan). de_thread has no core caller (the
//! exec path in syscall reaches it downward) so it needs no row.

#![forbid(unsafe_code)]
use crate::errno;
use crate::task::{Pid, Task, TaskState};

// ============================================================================
// Robust list / clear_child_tid exit processing (must run while the mm lives)
// ============================================================================

/// FUTEX_OWNER_DIED bit (Linux include/uapi/linux/futex.h).
const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// TID mask of a futex word (low 30 bits hold the owner TID).
const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
/// FUTEX_WAITERS bit (bit 31) — preserved by the death mark.
const FUTEX_WAITERS: u32 = 0x8000_0000;
/// Max entries walked per robust list (defensive bound, Linux uses
/// ROBUST_LIST_LIMIT=2048 per chain).
const ROBUST_LIST_LIMIT: u32 = 2048;
/// sizeof(struct robust_list_head) on 64-bit (list.ptr + futex_offset +
/// pending ptr).
const ROBUST_LIST_HEAD_SIZE: usize = 24;

/// Process the dying task's robust futex list (Linux exit_robust_list).
///
/// For every list entry whose futex word still carries our TID as owner,
/// set FUTEX_OWNER_DIED and wake one waiter. Must run BEFORE the task's
/// mm reference is dropped (the words live in user memory).
///
/// Optimistic walk: any unreadable pointer terminates that chain (Linux
/// skips broken entries too); the head itself must be the registered
/// 24-byte layout.
///
/// Contract: `task` is the exiting task (valid; its mm still lives).
pub fn exit_robust_list(t: &Task) {
    use crate::arch::uaccess_safe as ua;

    // The caller (do_exit) passes the running task about to exit; the
    // mm is dropped strictly after this runs.
    let head = t.robust_list_head() as usize;
    if head == 0 || t.robust_list_len() != ROBUST_LIST_HEAD_SIZE {
        return; // not registered / legacy len — nothing to do
    }
    #[cfg(feature = "debug-futex-trace")]
    {
        use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
        taskdump_raw_line(b"FTX ROBUST-EXIT pid=");
        taskdump_dec(t.pid() as u64);
        taskdump_raw_line(b" head=");
        taskdump_dec(head as u64);
        taskdump_raw_line(b"\n");
    }
    let my_tid = t.pid();
    let mm_id = crate::sync::futex::task_futex_mm_id(t);

    // futex_offset (signed long) at head+8: uaddr = entry + futex_offset.
    let futex_offset: i64 = match ua::read_user::<i64>(head + 8) {
        Some(v) => v,
        None => return,
    };

    // Process one entry: mark OWNER_DIED if we still own it, wake waiters.
    // Reference parity (kernel/futex/core.c handle_futex_death 695-720):
    // the death mark is `(uval & FUTEX_WAITERS) | FUTEX_OWNER_DIED` — the
    // dying TID is CLEARED so an inheriting waiter (PI lock_pi take-over
    // or plain re-check) sees an unowned word, exactly like the
    // reference. The old `v | FUTEX_OWNER_DIED` kept the dead tid in the
    // word and left every PI waiter looping on an owner that could never
    // release.
    // All user accesses go through the exception-table typed family.
    fn handle_robust_entry(entry: usize, futex_offset: i64, tid: u32, mm_id: usize) {
        use crate::arch::uaccess_safe as ua;
        let uaddr = (entry as i64).wrapping_add(futex_offset) as u64;
        if uaddr & 0x3 != 0 {
            return; // misaligned futex word — skip
        }
        let v: u32 = match ua::read_user::<u32>(uaddr as usize) {
            Some(v) => v,
            None => return, // bad pointer — skip entry
        };
        if v & FUTEX_TID_MASK == tid {
            let _ = ua::write_user::<u32>(uaddr as usize, (v & FUTEX_WAITERS) | FUTEX_OWNER_DIED);
            crate::sync::futex::futex_wake_in_mm(
                uaddr as usize,
                mm_id,
                0, // private futex semantics
                1,
                u32::MAX,
            );
        }
    }

    // The list_op_pending entry with an already-released word (owner==0):
    // the dying thread was killed between the userspace unlock and the
    // FUTEX_WAKE syscall — reference handle_futex_death's pending_op case
    // (700-703) wakes one waiter WITHOUT touching the word.
    // Exception-table user reads (the typed family).
    fn handle_pending_entry(pending: usize, futex_offset: i64, tid: u32, mm_id: usize) {
        use crate::arch::uaccess_safe as ua;
        let uaddr = (pending as i64).wrapping_add(futex_offset) as u64;
        if uaddr & 0x3 != 0 {
            return;
        }
        let v: u32 = match ua::read_user::<u32>(uaddr as usize) {
            Some(v) => v,
            None => return,
        };
        if v & FUTEX_TID_MASK == 0 {
            crate::sync::futex::futex_wake_in_mm(uaddr as usize, mm_id, 0, 1, u32::MAX);
        }
    }

    // Walk a robust_list chain starting at `start` (an entry address);
    // the chain ends when next == head (circular) or is unreadable.
    // Bounded walk, exception-table user reads (the typed family).
    fn walk_robust_chain(start: usize, head: usize, futex_offset: i64, tid: u32, mm_id: usize) {
        use crate::arch::uaccess_safe as ua;
        let mut entry = start;
        let mut count = 0u32;
        while entry != 0 && entry != head && count < ROBUST_LIST_LIMIT {
            handle_robust_entry(entry, futex_offset, tid, mm_id);
            entry = match ua::read_user::<u64>(entry) {
                Some(next) => next as usize,
                None => break,
            };
            count += 1;
        }
    }

    // Main list: first entry hangs off head+0. Bounded, exception-table
    // reads (the typed family).
    {
        if let Some(first) = ua::read_user::<u64>(head) {
            walk_robust_chain(first as usize, head, futex_offset, my_tid, mm_id);
        }
        // Pending list (held-when-killed locks): pointer at head+16; the
        // pointed-to node is itself an entry. Its word may already be
        // released (the unlock-syscall race) — handle it with the
        // reference's pending_op semantics instead of the list walk.
        if let Some(pending) = ua::read_user::<u64>(head + 16) {
            if pending as usize != 0 {
                handle_pending_entry(pending as usize, futex_offset, my_tid, mm_id);
            }
        }
    }
}

/// CLONE_CHILD_CLEARTID exit hook (Linux mm_release): write 0 to the
/// registered tid word and futex-wake one waiter — this is what makes
/// pthread_join return.
///
/// Must run BEFORE the exiting task drops its mm reference, and the wake
/// key must be the EXITING task's mm identity (all CLONE_VM threads share
/// it, so the joiner's wait key matches).
///
/// Contract: `task` is the exiting task; its mm still lives.
pub fn exit_clear_child_tid(t: &mut Task) {
    // The caller (do_exit) passes the exiting task; the mm is dropped
    // strictly after this runs.
    let tid_ptr = t.clear_child_tid();
    if tid_ptr.is_null() {
        return;
    }
    let mm_id = crate::sync::futex::task_futex_mm_id(t);
    #[cfg(feature = "debug-futex-trace")]
    {
        use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
        taskdump_raw_line(b"FTX CLEARTID-EXIT pid=");
        taskdump_dec(t.pid() as u64);
        taskdump_raw_line(b" u=");
        taskdump_dec(tid_ptr as usize as u64);
        taskdump_raw_line(b"\n");
    }

    // Write 0 to the tid pointer in user memory (the typed
    // exception-table copy path).
    let _ = crate::arch::uaccess_safe::write_user::<i32>(tid_ptr as usize, 0);
    // Wake any thread waiting on this futex (FUTEX_WAKE, 1 waiter).
    // Private semantics + the exiting task's mm as the key identity.
    // musl pthread_exit holds __thread_list_lock across SYS_exit and
    // passes &__thread_list_lock as the clone ctid — THIS wake is what
    // "unlocks" it for every later exiter/joiner (see futex_hash: the
    // wake must reach waiters that parked with shared semantics too).
    crate::sync::futex::futex_wake_in_mm(
        tid_ptr as usize,
        mm_id,
        0, // private futex
        1,
        u32::MAX,
    );
    t.set_clear_child_tid(core::ptr::null_mut());
}

// ============================================================================
// Thread group teardown
// ============================================================================

/// Force SIGKILL every OTHER live member of the caller's thread group.
/// Idempotent; the caller then waits via wait_for_thread_group_death.
pub fn kill_other_threads(current: &Task) {
    // Collect member pids under the ring lock; deliver outside it.
    let mut victims: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    // current is a valid, running task; its ring is well-formed (the
    // walker's contract — it takes the ring lock itself).
    let cur = current;
    cur.for_each_group_member(|m| {
            let m_raw = m as *const Task as *mut Task;
            if m_raw as *const Task != current as *const Task {
                let st = m.state();
                if !st.is_dead() {
                    victims.push(m.pid());
                }
            }
            // nanosleep-loss conviction (wwcensus-gated): every member's
            // terminal anatomy at exit-group time — last pick phase,
            // ownership qword, linkage, timed-sleep shadow, pending
            // signals, and the saved kernel resume address (names the
            // park site).
            if crate::trace::EVENT_WWCENSUS.armed()
                && m_raw as *const Task != current as *const Task
            {
                use crate::debug::taskdump::putc_brief as putchar;
                fn pd(mut v: u64) {
                    use crate::debug::taskdump::putc_brief as p;
                    let mut buf = [0u8; 20];
                    let mut n = 0;
                    if v == 0 { p(b'0'); return; }
                    while v > 0 && n < buf.len() {
                        buf[n] = b'0' + (v % 10) as u8; v /= 10; n += 1;
                    }
                    for k in (0..n).rev() { p(buf[k]); }
                }
                static XG_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if XG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 24 {
                    // SAFETY: diagnostic volatile read of a linked ring
                    // member's field (bounded, read-only).
                    let q = crate::r#unsafe::read_volatile_u64(
                        core::ptr::addr_of!(m.ti_on_cpu) as *const u64,
                    );
                    let (ph, ph_ago) =
                        crate::sched::hotpath::pick_trace_peek(m_raw).unwrap_or((0, 0));
                    let tid = m
                        .timed_sleep_timer_id
                        .load(core::sync::atomic::Ordering::Relaxed);
                    let kind = m
                        .timed_sleep_kind
                        .load(core::sync::atomic::Ordering::Relaxed);
                    let tdl = m
                        .timed_sleep_deadline
                        .load(core::sync::atomic::Ordering::Relaxed);
                    let nowt = crate::timer::read_time();
                    let linked = match m.policy() {
                        crate::task::SchedPolicy::Fifo
                        | crate::task::SchedPolicy::Rr => false,
                        _ => m.sched_entity().timeline_key().is_some(),
                    };
                    #[cfg(feature = "x86_64")]
                    let ra = m.thread.callee.ret_addr as u64;
                    #[cfg(not(feature = "x86_64"))]
                    let ra = m.thread.ra as u64;
                    for &b in b"\nXGM pid=" { putchar(b); }
                    pd(m.pid() as u64);
                    for &b in b" ph=" { putchar(b); }
                    pd(ph as u64);
                    for &b in b"/" { putchar(b); }
                    pd(ph_ago);
                    for &b in b" q=0x" { putchar(b); }
                    pd(q);
                    for &b in b" pk=" { putchar(b); }
                    pd(m.pick_state() as u64);
                    for &b in b" lnk=" { putchar(b); }
                    pd(linked as u64);
                    for &b in b" st=0x" { putchar(b); }
                    pd(m.state().bits() as u64);
                    for &b in b" ts=" { putchar(b); }
                    pd(kind as u64);
                    for &b in b"/tid=" { putchar(b); }
                    pd(tid);
                    for &b in b"/dl-" { putchar(b); }
                    if tdl > nowt {
                        pd((tdl - nowt) / 10000);
                    } else {
                        putchar(b'-');
                        pd((nowt - tdl) / 10000);
                    }
                    for &b in b"ms pend=0x" { putchar(b); }
                    pd(m.pending.get_all());
                    for &b in b" ra=0x" { putchar(b); }
                    pd(ra);
                    // per-pid LAST fire (collision-free FWT table)
                    for k in 0..32 {
                        if crate::timer::FWT_PID[k].load(
                            core::sync::atomic::Ordering::Relaxed,
                        ) == m.pid()
                        {
                            for &b in b" fw(ok" { putchar(b); }
                            pd(crate::timer::FWT_OK[k].load(
                                core::sync::atomic::Ordering::Relaxed,
                            ) as u64);
                            for &b in b",rq" { putchar(b); }
                            pd(crate::timer::FWT_ONRQ[k].load(
                                core::sync::atomic::Ordering::Relaxed,
                            ) as u64);
                            for &b in b",key" { putchar(b); }
                            pd(crate::timer::FWT_KEY[k].load(
                                core::sync::atomic::Ordering::Relaxed,
                            ) as u64);
                            for &b in b",st" { putchar(b); }
                            pd(crate::timer::FWT_ST[k].load(
                                core::sync::atomic::Ordering::Relaxed,
                            ) as u64);
                            for &b in b")j-ago=" { putchar(b); }
                            pd(crate::timer::get_jiffies().saturating_sub(
                                crate::timer::FWT_JIF[k].load(
                                    core::sync::atomic::Ordering::Relaxed,
                                ),
                            ));
                            break;
                        }
                    }
                    if tid != 0 {
                        for &b in b" del=" { putchar(b); }
                        match crate::timer::del_lookup(tid) {
                            Some((line, ago)) => {
                                for &b in b"L" { putchar(b); }
                                pd(line);
                                for &b in b"/" { putchar(b); }
                                pd(ago);
                            }
                            None => {
                                for &b in b"none" { putchar(b); }
                            }
                        }
                        for &b in b" tp=" { putchar(b); }
                        pd(crate::timer::timer_pending(tid) as u64);
                        // fire-side: did this entry ever fire?
                        {
                            let d = tid as usize & 0x3FFF;
                            if crate::timer::FWHOW_ID[d].load(
                                core::sync::atomic::Ordering::Relaxed,
                            ) == tid
                            {
                                for &b in b" FIRED ok=" { putchar(b); }
                                pd(crate::timer::FWHOW_OK[d].load(
                                    core::sync::atomic::Ordering::Relaxed,
                                ) as u64);
                                for &b in b"/" { putchar(b); }
                                pd(crate::timer::get_jiffies().saturating_sub(
                                    crate::timer::FWHOW_JIF[d].load(
                                        core::sync::atomic::Ordering::Relaxed,
                                    ),
                                ));
                                for &b in b"j-ago" { putchar(b); }
                            } else {
                                for &b in b" NOFIRE" { putchar(b); }
                            }
                        }
                    }
                    putchar(b'\n');
                }
            }
        });
        // gdm bring-up r9-r11: a silent mass SIGKILL took out the whole
        // daemon set with no print at any loglevel; every other kernel
        // SIGKILL fan-out path is either printed or was excluded by
        // instrumentation. If this fires with victims OUTSIDE the caller's
        // thread group, the thread ring is corrupt — this print names the
        // caller and victims directly.
        // OH boot lane 2026-11-12: debug-gated (sigtrace) — the hunt is
        // closed (the 100th decoded the KOT-KILL lists as unnamed worker
        // THREADS sharing the parent comm, not cross-process kills), and
        // the OH churn runs kill_other_threads on every FFRT group
        // teardown: the victim-name walk + pr_err rode that hot path
        // (873 samples through acquire_emit_lock in the 100th's record).
    if !victims.is_empty()
        && crate::trace::EVENT_SIGTRACE.armed()
    {
        let ccomm = current.comm_str();
        let mut vnames: alloc::vec::Vec<alloc::string::String> =
            alloc::vec::Vec::new();
        for v in victims.iter().take(20) {
            if let Some(t) = crate::find_task_by_pid(*v) {
                vnames.push(alloc::string::String::from(t.comm_str()));
            }
        }
        crate::pr_debug!(
                            "KOT-KILL: pid={} comm='{}' kills {} group members: {:?}",
            current.pid(),
            ccomm,
            victims.len(),
            vnames
        );
    }
    for pid in victims {
        // Reuse the KILL path of send_signal: unignorable, wakes sleepers.
        let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGKILL as i32);
    }
}

/// Spin-yield until every other member of the group has exited
/// (nr_threads converges to 1).
///
/// SIGKILL'd members reach do_exit, drop their mm refs, and leave the
/// ring — the decrement happens BEFORE their final schedule, so once
/// nr_threads == 1 all their mm Arc references are already released.
/// A member stuck in UNINTERRUPTIBLE kernel sleep will wedge us here
/// (same trade-off as Linux's exit_group on D-state threads is NOT
/// taken there — documented as a known risk).
///
/// `killable` gates the fatal-signal escape: the EXEC (de_thread)
/// caller passes true — its own pending SIGKILL aborts the drain
/// (returns false, drain incomplete) so the exec can unwind with EINTR
/// before anything is installed; the do_exit caller passes false — an
/// exiting leader CANNOT unwind, and the shared-mm-dependent teardown
/// below it requires the drain, so the wait parks regardless of its
/// own pending signals (the pre-existing contract).
pub fn wait_for_thread_group_death(current: &Task, killable: bool) -> bool {
    crate::arch::cpu::restore_irq(true);
    // timer-gap lane (pi_stress -g 2 shutdown wedge): the wait must PARK,
    // not spin. A RUNNING leader's schedule() requeues and re-picks it
    // every pass — at FIFO priority above the SIGKILL'd members it
    // starves the very threads it is waiting for (observed: leader
    // pinned cpu0 looping here for 6983 jiffies while three
    // ownership-free, SIGKILL-pending members never ran once; RT
    // throttling rescued CFS but never the lower-RT members). Park
    // INTERRUPTIBLE: the pick skips a sleeping leader, the members run
    // their fatal-signal exits, and thread_group_leave's decrement-side
    // wake (see task.rs) brings us back to re-check nr_threads.
    //
    // FATAL-SIGNAL ESCAPE, killable arm only (oh-launch round 3): a
    // member that cannot exit (stuck in an unkillable in-kernel wait —
    // the round-2 foundation wedge: one open() parked forever blocked
    // the group drain, which parked the exec'ing thread's execveat
    // forever and silenced the whole service) used to park THIS wait
    // forever too. The exec'ing task's own pending SIGKILL aborts the
    // drain and unwinds the exec with EINTR — safe there because
    // de_thread runs BEFORE any state is installed (no new mm, no
    // fdtable switch); the already-SIGKILLed members keep dying on
    // their own, and the abandoning thread returns to userspace where
    // its SIGKILL is delivered and it exits normally.
    // current stays valid (it is the running task).
    while current.nr_threads() > 1 {
        if killable && crate::signal::fatal_signal_pending() {
            return false;
        }
        current.set_state(crate::task::TaskState::new(
            crate::task::TaskState::INTERRUPTIBLE,
        ));
        crate::sched::hotpath::schedule();
        current.set_state(crate::task::TaskState::new(
            crate::task::TaskState::RUNNING,
        ));
    }
    true
}

/// de_thread — exec-time group teardown (Linux de_thread):
/// kill every other thread and wait for the group to drain before the
/// new image is installed. Called from the execve path AFTER all
/// fail-able preparation (a failed exec leaves the process single-
/// threaded, matching Linux's point-of-no-return behavior).
/// Returns false when the drain was aborted by the exec'ing task's
/// own fatal signal (the caller unwinds the exec with EINTR — see
/// wait_for_thread_group_death).
pub fn de_thread(current: &Task) -> bool {
    // Caller (execve path) passes the running task.
    if current.nr_threads() > 1 {
        kill_other_threads(current);
        return wait_for_thread_group_death(current, true);
    }
    true
}

/// Free one exited group member whose context switch-out has completed.
///
/// Mirrors release_task's tail for a member: the member already removed
/// itself from the pid hash, freed its PID, and dropped its mm/fdtable
/// Arcs at exit time — here we only wait out on_cpu, drop remaining Arcs
/// (signal/fs), free the kernel stack, and put the Task slot.
///
/// R87 (oh-launch round 10, the scribbler producer): returns FALSE —
/// freeing NOTHING — when the bounded wait's deadline adjudication could
/// not confirm with positive knowledge that no owner remains (a slot or
/// a SWITCHING_OUT marker of any age still names the member; see
/// member_release_stale_ownership). The caller must then RE-PARK the
/// member on the leader's dead_threads list for the next sweep. The old
/// shape had exactly two failure modes, both fatal somewhere: spin
/// forever (r16type/r20type — one CPU lost per wedged member), or
/// force-release marks an aged-but-live switch still depends on and free
/// the stack the switching CPU was still executing on (the recycled-
/// frame scribbler behind the string-byte hijacks and the 0xb9 PTE
/// face). Deferral is the recovery-path shape: skip, never corrupt —
/// the sweeper returns immediately, the member frees on a later sweep
/// when its switch-out truly completes, and a genuinely lost release
/// leaks the stack loudly (FDM-DEFER, bounded) instead of corrupting.
///
/// Contract: `member` is a DEAD task parked on a leader's dead_threads
/// list (callers guarantee the DEAD state — see sweep_dead_threads /
/// release_task).
/// Round-B extension: the member's storage-free obligation enters as a
/// CLAIM (minted at the take-out under the dead-list lock — the
/// exactly-once take). Exits: `None` = burned (the free ran; the claim
/// was consumed), `Some(claim)` = re-park (the R87/R86 adjudication
/// deferred; the caller pushes `claim.into_parked()` back on the list).
fn free_dead_member(claim: crate::sched::types::ReapClaim) -> Option<crate::sched::types::ReapClaim> {
    // The caller parked `member` DEAD on the dead list; nothing else can
    // reach it while we hold the list's discipline (INV-PROC-BORROW-1's
    // dead-list arm). The claim owns the free — this body is its burn
    // (or its deferral, handed back).
    let member = claim.as_ptr();
    let m = crate::r#unsafe::borrow_task_mut(member);
    // The member's final context switch-out may still be in flight —
    // on_cpu is cleared exactly when __switch_to has saved its context
    // (see release_task R9 for the full rationale).
    crate::arch::cpu::restore_irq(true);
    // DEBUG (4-thread hang hunt): report a stuck wait loudly instead of
    // spinning silently — a member whose on_cpu never clears wedges every
    // later exiter (and their CPUs) here.
    //
    // R86 (GNOME no-light wedge): the wait is now BOUNDED. A legitimate
    // final switch-out clears on_cpu in micro-to-milliseconds even under
    // TCG storms; 300 jiffies (3 s) is ~1000x that margin. Past the
    // deadline the ownership is adjudicated against scheduler truth
    // (member_release_stale_ownership): a DEAD, unlinked member named by
    // no CPU slot and no in-flight switch-out is in the R85 lost-release
    // terminal state — nothing will ever clear on_cpu, and the old
    // forever-spin here cost this CPU for the rest of the boot (r16type/
    // r20type: both SMP2 CPUs lost mid shell-init, screen dark 2700 s,
    // PREEMPT-HEAL 255/466 firings, kernel otherwise healthy).
    //
    // R87: the adjudication is now POSITIVE-KNOWLEDGE and ONE-SHOT. The
    // heal fires only on the pure lost-release face (named by no slot
    // and no SWITCHING_OUT marker of ANY age); every other shape — an
    // aged-but-possibly-live switch-out window included — returns false
    // and this call DEFERS the free to the next sweep (returns false;
    // the caller re-parks the member). No re-arm, no forever-spin: the
    // sweeper is back on the runqueue within the one deadline window.
    const FDM_STALE_RELEASE_JIFFIES: u64 = 300;
    let mut deferred = false;
    {
        // LTP-open lane (cve-2017-17052 wedge, 2026-11-14): the wait used
        // to be a pure spin_loop. on_cpu clears on the MEMBER's OWN CPU
        // when its exit tail reaches the final context switch — nothing
        // this CPU does advances that — and under a fork/exit storm the
        // exit tails queue behind allocator and lock contention for
        // SECONDS. Every sweeper then burned a whole CPU spinning
        // (wakewatch: the runner's wake→pick strands 1-3s behind these
        // monopolist windows; FDM-WAIT churn with climbing member pids is
        // the wedge face). Bounded spin batches keep the µs-latency fast
        // path (a legitimate final switch-out clears in micro-to-ms); a
        // schedule() between batches returns the CPU to the runqueue so
        // the very exit tails we are waiting ON can make progress.
        // Same discipline as wait_for_thread_group_death (a schedule()
        // loop in the same exit context); IRQs are already enabled here.
        const FDM_SPIN_BATCH: u32 = 100_000;
        let mut spins: u32 = 0;
        let mut armed_at = crate::timer::get_jiffies();
        // R87 (tail window): the wait must ALSO cover the SWITCHING_OUT
        // marker. on_cpu (the asm release store) clears BEFORE the
        // register restore — the switching CPU still executes its final
        // loads, push and ret ON THE MEMBER'S STACK with on_cpu already
        // false, and under KVM a host deschedule right there lets the
        // freeing CPU win the race (RCU poll completes, stack freed,
        // recycled) before the switcher's ret lands. The marker clears
        // only in the incoming frame's post-switch tail, after prev's
        // stack is abandoned for good.
        // R11 (terminal-release window): neither condition covers the
        // gap BEFORE the marker publish: the terminal deschedule loop
        // releases the ownership pair BEFORE calling schedule() (the
        // sc191 v2 fix), leaving {on_cpu=false, no marker} while the
        // task still executes its own schedule chain on its stack —
        // every abort/retry cycle of that loop re-opens the window.
        // terminal_switch_done is the positive knowledge: set only at
        // the real-switch retire sites, re-armed false by the loop each
        // round — {flag set AND on_cpu false AND no marker} is the full
        // no-references condition.
        while m.on_cpu()
            || crate::sched::hotpath::switching_out_names(member)
            || !m
                .terminal_switch_done
                .load(core::sync::atomic::Ordering::Acquire)
        {
            if spins == 0 {
                let cur_pid = crate::sched::hotpath::current().map(|c| c.pid()).unwrap_or(0);
                crate::debug::taskdump::taskdump_raw_line(b"FDM-WAIT sweeper=");
                crate::debug::taskdump::taskdump_dec(cur_pid as u64);
                crate::debug::taskdump::taskdump_raw_line(b" member=");
                crate::debug::taskdump::taskdump_dec(m.pid() as u64);
                crate::debug::taskdump::taskdump_raw_line(b" state=0x");
                let st = m.state().bits() as u64;
                let mut sh: i32 = 32;
                while sh > 0 {
                    sh -= 4;
                    let nb = ((st >> sh) & 0xF) as u8;
                    crate::debug::taskdump::taskdump_raw_line(&[(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 })]);
                }
                crate::debug::taskdump::taskdump_raw_line(b"\n");
            }
            if crate::timer::get_jiffies()
                .wrapping_sub(armed_at)
                >= FDM_STALE_RELEASE_JIFFIES
            {
                // R86/R87: past the deadline — ONE adjudication per call.
                // A true heal (pure lost-release, marks released) makes
                // on_cpu false and exits the loop; anything else is a
                // live or ambiguous owner — DEFER to the next sweep,
                // never free what a live owner claims.
                if !crate::sched::hotpath::member_release_stale_ownership(member) {
                    deferred = true;
                    break;
                }
                armed_at = crate::timer::get_jiffies();
            }
            spins = spins.wrapping_add(1);
            if spins % FDM_SPIN_BATCH == 0 {
                crate::sched::hotpath::schedule();
            } else {
                core::hint::spin_loop();
            }
        }
    }
    if deferred {
        // Bounded, cold-path report on the raw-uart channel (same as
        // FDM-WAIT): the free was DEFERRED, the member stays parked.
        static FDM_DEFER_SEEN: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = FDM_DEFER_SEEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 64 {
            let claim = m
                .running_on_cpu
                .load(core::sync::atomic::Ordering::Acquire);
            crate::debug::taskdump::taskdump_raw_line(b"FDM-DEFER #");
            crate::debug::taskdump::taskdump_dec((n + 1) as u64);
            crate::debug::taskdump::taskdump_raw_line(b" member=");
            crate::debug::taskdump::taskdump_dec(m.pid() as u64);
            crate::debug::taskdump::taskdump_raw_line(b" claim=");
            crate::debug::taskdump::taskdump_dec(claim as u64);
            crate::debug::taskdump::taskdump_raw_line(b" (owner live/ambiguous; free deferred to next sweep)\n");
        }
        return Some(claim);
    }
    // BOUNDED (lb125 decode): monopolist windows stalled this wait for
    // tens of seconds, freezing the exit — and every parent behind it.
    crate::sync::rcu::synchronize_rcu_bounded_pollfirst(1500, "thread-stack-free");

    m.free_kernel_stack();

    // Heap PtRegs from the legacy fork path (unused by the live path).
    crate::r#unsafe::free_fork_pt_regs(m);

    // Drop remaining Arc references (mm/fdtable were dropped at exit).
    m.set_address_space(None);
    m.set_fdtable(None);
    m.signal = None;
    m.set_fs(None);

    crate::task::Task::task_put(member);
    // The burn: the obligation is consumed (the free above ran; the
    // claim dies with it — no re-park path holds it).
    core::mem::forget(claim);
    None
}

/// Opportunistic sweep of a leader's dead-members list: free everyone
/// whose context switch-out completed. The just-pushed member typically
/// still shows on_cpu and stays parked; older entries are reclaimed,
/// bounding the list across create/join churn.
///
/// 4-thread-hang root cause (fixed): this used to compute `pending` as
/// `mem::replace(&mut *dl, still_running)` — the replace returns the ENTIRE
/// old list, so every "kept" (still on_cpu) member was ALSO handed to
/// free_dead_member. An exiting worker sweeping its own fresh entry then
/// spun in free_dead_member's `while member.on_cpu()` on ITSELF — a
/// permanent self-deadlock that pinned one CPU per exiting thread (4
/// workers = all 4 CPUs; the linked-but-runnable main went permanently
/// unpicked). Members that did not spin were freed while still linked on
/// the dead list — the freed-while-referenced / double-free engine behind
/// the phantom-running and double-execution morphologies. The partition is
/// now exact: each member is either kept (stays on the list) or pending
/// (freed exactly once, removed from the list), never both.
///
/// Liveness predicate: free only members whose state is DEAD (their do_exit
/// passed the terminal store — a member parked but preempted mid-exit is
/// still RUNNING and must NEVER be freed here; on_cpu alone was a broken
/// proxy for that, since a preempted parked exiter sits queued with
/// on_cpu==false while fully alive) AND whose final context switch-out
/// completed (on_cpu==false — the window between `set_state(DEAD)` and
/// __switch_to's clear).
pub fn sweep_dead_threads(leader: *mut Task) {
    // Leader is a valid task (INV-PROC-BORROW-1: the caller's ring/tree
    // discipline keeps the slot alive through this sweep).
    let leader_ref = crate::r#unsafe::borrow_task(leader);
    let pending: alloc::vec::Vec<*mut Task> = {
        let mut dl = leader_ref.dead_threads.lock();
        let old = core::mem::replace(&mut *dl, alloc::vec::Vec::new());
        let mut still_running: alloc::vec::Vec<*mut Task> =
            alloc::vec::Vec::with_capacity(old.len());
        let mut reclaimable: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();
        for m in old {
            // SAFETY: m is a parked member pointer (pushed by its own do_exit
            // while holding the task alive via the dead-list reference).
            // R11: terminal_switch_done is REQUIRED — the terminal loop's
            // pre-schedule ownership release makes {DEAD, !on_cpu} true
            // while the member still runs its own schedule chain.
            let m_ref = crate::r#unsafe::borrow_task(m);
            let dead_and_off_cpu = m_ref.state().is_dead()
                && !m_ref.on_cpu()
                && m_ref
                    .terminal_switch_done
                    .load(core::sync::atomic::Ordering::Acquire);
            if dead_and_off_cpu {
                reclaimable.push(m);
            } else {
                still_running.push(m);
            }
        }
        // Put the survivors back; `reclaimable` is the exact complement —
        // every member is in exactly one of the two vectors.
        *dl = still_running;
        reclaimable
    };
    for m in pending {
        // SAFETY: m came from the leader's dead list (DEAD, hash-removed,
        // final switch-out completed). The take-out above (under the
        // list lock) is the mint point — the exactly-once free is typed
        // from here to the burn.
        let freed = match crate::sched::types::ReapClaim::mint(m) {
            Some(c) => free_dead_member(c).is_none(),
            None => false,
        };
        if !freed {
            // R87: heal-fail defers the free — re-park for the next
            // sweep (the channel boundary: the obligation returns to
            // the lock-typed list). Lifetime: this sweep's only caller
            // is a member's own do_exit BEFORE its terminal DEAD store,
            // so the leader's group-death wait is still blocked on us
            // and the leader cannot have been reaped or freed
            // mid-sweep.
            let mut dl = leader_ref.dead_threads.lock();
            dl.push(m);
        }
        // Perf gate (2026-10-20): the sweep frees stacks, PtRegs and Arc
        // references per member under the OH thread-churn load; without
        // a yield point a long sweep monopolizes this CPU (steady
        // WAKE-STRANDED family). Safe point: no locks held here.
        crate::sched::hotpath::cond_resched();
    }
    // 69th (105-family fix) — deferred-release finisher. When this
    // group's leader was reaped while a member (possibly the caller
    // itself) was still mid-exit, release_task marked it
    // release_deferred and returned without freeing its storage. The
    // LAST dying member finishes that free here, in its own context:
    // once the parked list holds only the caller, nobody else can ever
    // touch the leader again (the caller itself does not touch it after
    // its terminal schedule). The lock- guarded len==1 check makes the
    // "last" decision exactly-once even with concurrent exiters.
    if leader_ref
        .release_deferred
        .load(core::sync::atomic::Ordering::Acquire)
    {
        if let Some(cur) = crate::sched::hotpath::current() {
            let cur = cur;
            // SAFETY: leader is a valid task (its storage free is
            // exactly what this finisher arbitrates); cur is the
            // running task.
            let last = {
                let mut dl = leader_ref.dead_threads.lock();
                let last = dl.len() == 1 && dl[0] == cur;
                if last {
                    dl.clear();
                }
                last
            };
            if last {
                static FINISH_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = FINISH_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 64 {
                    crate::pr_warn!(
                        "RELEASE-FINISH #{} leader={} freed by last member {}",
                        n,
                        leader_ref.pid(),
                        cur.pid()
                    );
                }
                // SAFETY: the leader is dead, tree/pid-hash detached,
                // its dead_threads list empty, and no other member can
                // reach it (the caller was the last and is about to
                // leave it forever). The len==1 take under the list
                // lock above is the mint point — the population-
                // selected handoff is lock-arbitrated exactly-once, so
                // the post-take obligation is linear like every other.
                if let Some(c) = crate::sched::types::ReapClaim::mint(leader) {
                    release_task_storage(c);
                }
            }
        }
    }
}

// ============================================================================
// Reaping
// ============================================================================

/// Budget for release_task's dead-members drain (69th, the 105-family
/// fix): 300 jiffies (3 s at HZ=100), the same margin free_dead_member's
/// R86 bounded on_cpu wait uses. Past the budget the storage free is
/// deferred to the last dying member instead of blocking the reaper.
const RELEASE_DRAIN_JIFFIES: u64 = 300;

/// Release task resources when being reaped by parent
///
/// This function is called by do_wait() when reaping a zombie child.
/// It frees all resources associated with the task:
/// - Kernel stack
/// - Address space (Arc reference)
/// - File descriptor table (Arc reference)
/// - Signal struct (Arc reference)
/// - Filesystem info (Arc reference)
/// - PID
///
/// # Safety
/// Caller must ensure task is in ZOMBIE state and not currently running
// Contract: caller guarantees task is a zombie and no CPU is running it.
// The RCU grace period in synchronize_rcu() ensures no readers hold stale
// references.
pub fn release_task(task: *mut Task) {
    // SAFETY: zombie per the contract above; the reaper owns the release.
    let t = crate::r#unsafe::borrow_task_mut(task);
    // A reaped leader first drains its dead-members list — every exited
    // group member is fully released before the leader's own storage
    // goes away (their group_leader pointers name `task`).
    //
    // A member still mid-exit (parked but preempted before its terminal
    // DEAD store — RUNNING/queued, fully alive) must NOT be freed. The
    // leader's ZOMBIE flow (wait_for_thread_group_death) normally
    // guarantees every member is DEAD by now, but apply the same exact
    // partition as sweep_dead_threads anyway: only genuinely DEAD members
    // are freed; live stragglers stay parked on the list and are retried.
    //
    // 69th (the 105-family root cause, ohw79/ohw80): this drain used to
    // be an UNBOUNDED hot spin. A member whose exit tail wedges (fdreap
    // grace stall, scheduler strand) never reaches DEAD, and the reaping
    // parent — init itself, inside its SIGCHLD handler's
    // `while (waitpid(-1, &st, WNOHANG) > 0);` loop — hot-spun here
    // forever at 100% CPU: the LoopEvent died, every later
    // ohos.ctl.start SET timed out with PARAM_CODE_TIMEOUT (105), and
    // the whole on-demand ladder stalled. Reference release_task NEVER
    // waits on another thread's exit context. The drain is now a
    // BOUNDED SLEEPING wait (3 s, the R86 free_dead_member budget): if
    // members remain, the reap completes its SEMANTICS (pid removal,
    // reparenting, PID free), the storage free is DEFERRED to the last
    // dying member's sweep_dead_threads (see the finisher there), and
    // the reaper RETURNS. A wedged member then leaks its group (loudly
    // printed) instead of wedging pid 1.
    let deadline = crate::timer::get_jiffies() + RELEASE_DRAIN_JIFFIES;
    let mut deferred = false;
    loop {
        // One try_lock'd pass: free the DEAD members in place, leaving
        // stragglers parked. try_lock (not lock) so a WEDGED
        // dead_threads lock (the ohw80 DEADLOCK variant) cannot park
        // the reaper inside the acquisition itself.
        let drained = match t.dead_threads.try_lock() {
            Some(mut dl) => {
                let mut reclaim: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();
                dl.retain(|m| {
                    // SAFETY: m is a parked member pointer owned by this list.
                    // R87: require the final switch-out to have completed
                    // (!on_cpu) — the same exact partition as
                    // sweep_dead_threads — so a re-parked deferral (whose
                    // on_cpu is the ambiguous live mark) is never re-fed
                    // to free_dead_member's bounded wait on every drain
                    // round; the loop below then defers the reap at ITS
                    // deadline instead.
                    // R11: terminal_switch_done required for the same
                    // reason as the sweep's predicate.
                    {
                        let m_ref = crate::r#unsafe::borrow_task(*m);
                        if m_ref.state().is_dead()
                            && !m_ref.on_cpu()
                            && m_ref
                                .terminal_switch_done
                                .load(core::sync::atomic::Ordering::Acquire)
                        {
                            reclaim.push(*m);
                            false
                        } else {
                            true
                        }
                    }
                });
                let empty = dl.is_empty();
                drop(dl);
                let mut deferred_any = false;
                for m in reclaim {
                    // SAFETY: m was parked DEAD with its final switch-out
                    // complete; the list no longer references it (the
                    // retain above was the take-out — the mint point).
                    let freed = match crate::sched::types::ReapClaim::mint(m) {
                        Some(c) => free_dead_member(c).is_none(),
                        None => false,
                    };
                    if freed {
                        continue;
                    }
                    // R87: heal-fail defers the free — re-park on this
                    // leader (alive in the reaper's own context) so a
                    // later sweep retries once the switch-out truly
                    // completes (the channel boundary). `empty &&
                    // !deferred_any` keeps the reap from completing
                    // while a deferral is parked.
                    deferred_any = true;
                    let mut dl = t.dead_threads.lock();
                    dl.push(m);
                }
                empty && !deferred_any
            }
            None => {
                // Lock contention (a member's park/sweep holds it):
                // fall through to the yield/deadline check below.
                false
            }
        };
        if drained {
            break;
        }
        if crate::timer::get_jiffies() >= deadline {
            // Budget spent: DEFER the storage free. The members stay
            // parked on this task's dead_threads list; the finisher is
            // the last dying member's sweep (and if a member never
            // dies, the group leaks — printed — but the reaper lives).
            deferred = true;
            t.release_deferred.store(true, core::sync::atomic::Ordering::Release);
            {
                static DEFER_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = DEFER_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 64 {
                    let mut line: alloc::string::String = alloc::format!(
                        "RELEASE-DEFER #{} leader={} reaper_pid={} parked:",
                        n,
                        t.pid(),
                        crate::current_pid()
                    );
                    // Read-only diagnostics of parked member tasks.
                    if let Some(dl) = t.dead_threads.try_lock() {
                        for m in dl.iter().take(6) {
                            let m_ref = crate::r#unsafe::borrow_task(*m);
                            line.push_str(&alloc::format!(
                                " [{}:{} st={:#x} on_cpu={}]",
                                m_ref.pid(),
                                m_ref.tgid(),
                                m_ref.state().bits(),
                                m_ref.on_cpu() as u8
                            ));
                        }
                    }
                    crate::pr_warn!("{}", line);
                }
            }
            break;
        }
        // Sleeping yield (wait_for_thread_group_death's pattern), NOT a
        // hot spin: the stragglers need CPU to finish dying, and the
        // reaper's CPU must stay available to the rest of the system.
        crate::arch::cpu::restore_irq(true);
        crate::sched::hotpath::schedule();
    }

    // ===== RUSAGE_CHILDREN fold at reap (reference wait_task_zombie) =====
    // The reaped child's own tick totals PLUS the totals it accumulated
    // from ITS reaped children roll into the real parent's cutime/cstime;
    // ru_maxrss takes the max over (child's own exit rss, child's own
    // children-hiwater). Runs before any child state is torn down.
    {
        use core::sync::atomic::Ordering::Relaxed;
        let u = t.utime_ticks.load(Relaxed);
        let s = t.stime_ticks.load(Relaxed);
        let cu = t.ext.cutime_ticks.load(Relaxed);
        let cs = t.ext.cstime_ticks.load(Relaxed);
        let rss = t.ext.exit_maxrss_kb.load(Relaxed);
        let crss = t.ext.cmaxrss_kb.load(Relaxed);
        // release_task only ever runs in the REAPING parent's wait
        // context (do_wait/do_waitid, including init reaping orphans);
        // the child's own parent pointer is already None here (the
        // E8-REPAR claim), so the fold targets the caller.
        if let Some(parent) = crate::sched::hotpath::current() {
            (*parent).ext.cutime_ticks.fetch_add(u + cu, Relaxed);
            (*parent).ext.cstime_ticks.fetch_add(s + cs, Relaxed);
            let _ = (*parent).ext.cmaxrss_kb.fetch_max(rss.max(crss), Relaxed);
        }
    }

    // Remove from PID hash table before freeing resources
    crate::pid_hash::pid_hash_remove(t.pid());

    // Detach from parent's children list (must happen before freeing task memory)
    // E8-REPAR: screen the parent pointer first — do_wait claimed the
    // child under the tree lock (parent forced None), so a Some here is
    // either the normal pre-claim path or a stale pointer planted by
    // older corruption; never dereference an implausible one.
    let parent_ptr = t.parent_ptr();
    if let Some(parent) = parent_ptr {
        if Task::is_plausible_task_ptr(parent) {
            // Screened plausible above; remove_child takes the tree
            // lock itself.
            crate::r#unsafe::borrow_task(parent as *mut Task)
                .remove_child(task);
        }
    }

    // Leg-29A release-time heal: every child still linked on this dying
    // task's children list at release was missed by every earlier
    // reparent (the PF_EXITING-dest race's residue, or any future hole).
    // Its Task page is about to be poisoned and reused — move the
    // survivors to init NOW or their parent pointers dangle into freed
    // memory forever (the TREE-PARENT-DEAD / sc304 chain class). Should
    // stay empty on a fixed kernel; a TREE-RELEASE-HEAL line is the
    // always-on detector that a hole reopened.
    {
        let mut stranded: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();
        t.collect_children_pinned(&mut stranded);
        if !stranded.is_empty() {
            let init = crate::sched::hotpath::find_task_by_pid(1);
            if !init.is_null() {
                static HEAL_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if HEAL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    crate::pr_info!(
                        "TREE-RELEASE-HEAL: pid {} released with {} children still linked; moved to init",
                        t.pid(),
                        stranded.len()
                    );
                }
                for child in stranded {
                    let ok = t.move_child(child, init);
                    if !ok {
                        Task::task_put(child);
                        continue;
                    }
                    // A healed ZOMBIE child must be signalled so init
                    // reaps it (same contract as the reparent mover:
                    // full child_exit siginfo, child's exit_signal).
                    // `child` is pinned (collect_children_pinned) and
                    // still linked.
                    let c = crate::r#unsafe::borrow_task(child);
                    if c.state()
                        == crate::task::TaskState::new(
                            crate::task::TaskState::ZOMBIE,
                        )
                    {
                        let mut info = crate::signal::SigInfo::child_exit(
                            c.pid(),
                            c.cred().uid,
                            c.exit_code(),
                            c.core_dumped(),
                        );
                        let orphan_sig = c.get_exit_signal();
                        if orphan_sig != 0 {
                            if orphan_sig != crate::signal::Signal::SIGCHLD as u8 {
                                info.si_signo = orphan_sig as i32;
                            }
                            let _ = crate::signal::send_signal_with_info(1, info);
                        }
                    }
                    Task::task_put(child);
                }
            }
        }
    }

    // Free PID — belongs to the reap SEMANTICS (the number returns to
    // the pool even when the storage free is deferred).
    crate::pid::free_pid(t.pid());

    if deferred {
        // 69th: the storage free (RCU grace + stack/Arc/Task free) is
        // finished by the LAST dying member's sweep_dead_threads; if a
        // member never dies the group leaks (named in RELEASE-DEFER)
        // but the reaper — init and its param service — lives.
        return;
    }
    // P4 async-reap: the storage free (switch markers + RCU grace +
    // stack/Arc/Task free) belongs to the tick drainer now — the
    // reaper NEVER waits. Measured base: every reap burned the full
    // 3 s RTS deadline (the terminal marker never lands for the
    // single-threaded shape), so 1000 reaps = 3000 s and P4 could
    // never finish. Over cap (wedged drainer) the old bounded
    // synchronous path stands as the fallback.
    if !crate::sched::hotpath::defer_reap_storage(task, t.parent_ptr().map(|p: *const Task| crate::r#unsafe::borrow_task(p as *mut Task).pid()).unwrap_or(0)) {
        // Over-cap fallback: the obligation the drainer could not take
        // is minted here and burned synchronously (the cold path's
        // exactly-once, typed the same way).
        if let Some(c) = crate::sched::types::ReapClaim::mint(task) {
            release_task_storage(c);
        }
    }
}

/// The storage half of release_task: everything that actually returns
/// memory (RCU grace for pid_hash readers, on_cpu wait, kernel stack,
/// fork PtRegs, Arc references, Task slot). Runs in the reaper's
/// context on the normal path, and in the LAST dying member's context
/// for a deferred-release leader (see sweep_dead_threads's finisher).
///
/// # Safety
/// Caller must ensure `task` is a dead/zombie task no longer reachable
/// through the pid hash or the process tree, that no other task still
/// references it (the deferred path's dead_threads list must be empty),
/// and that no CPU is running it.
/// RTS-DEFER amendment (2026-11-21): when the owner's terminal switch
/// has not completed at the wait deadline, this call returns EARLY with
/// NOTHING freed — the storage release is handed to the reaper's own
/// dead-threads drain (re-run on every reap) / the leader's sweep, and
/// the safety contract above is re-established by the drain's own
/// predicate before any retry touches the page.
/// Round-B extension: the storage-free obligation enters as a CLAIM.
/// Callers mint at their lock-arbitrated take: the reaper's over-cap
/// fallback (release_task's tail), and the deferred-leader finisher
/// (sweep_dead_threads's tail — its len==1 take is exactly-once under
/// the dead-list lock). Exits: the full free (burn — the claim is
/// consumed), or the RTS-DEFER early return (the obligation re-parks
/// on the reaper's dead_threads via the channel boundary — the raw
/// pointer the early-return pushes IS the un-minted claim).
fn release_task_storage(claim: crate::sched::types::ReapClaim) {
    let task = claim.as_ptr();
    // SAFETY: unreachable through the pid hash / process tree (the
    // release contract); this call owns the storage.
    let t = crate::r#unsafe::borrow_task_mut(task);
    // Wait for any RCU readers that may still be traversing this task's
    // pid_hash_links node to finish before we free the task memory.
    // BOUNDED (lb125 decode): pid_hash read sections are preempt-disable
    // and microseconds long; an unbounded wait here parked init's reaping
    // behind monopolist windows. 1.5 s is ~1000x any real reader.
    crate::sync::rcu::synchronize_rcu_bounded_pollfirst(1500, "release-task");

    // R9 (NEW2 engine #2): do_exit sets ZOMBIE BEFORE its final schedule(),
    // so a reaping parent could scan the early ZOMBIE and free this task's
    // kernel stack and Task while it is STILL EXECUTING its exit tail on
    // another CPU. The stack/Task would be reused (and zeroed) by the next
    // fork, and the dying task's remaining stores — including __switch_to's
    // context save — would land in live objects (the corrupted wait4
    // statuses, EBADF, zeroed children-list nodes). on_cpu is cleared by
    // __switch_to exactly when this task's context is saved: wait for that
    // before freeing.
    // R9-16 (actually landed, round 10): spin with interrupts enabled —
    // this CPU must keep reporting RCU quiescent states and taking ticks
    // while waiting (the round-9 commit claimed this but never landed).
    //
    // R87 (tail window, oh-launch round 10): on_cpu is NOT sufficient —
    // the asm release store precedes the register restore, so the dying
    // task's CPU still executes its final loads, push and ret ON THIS
    // STACK with on_cpu already false; a KVM host deschedule of the
    // switcher right there let the reaper win the race to the free (the
    // recycled-frame scribbler's early-strike face at 1.2-1.3 s on the
    // armed battery boots). ALSO wait out the SWITCHING_OUT marker (it
    // clears only in the incoming frame's tail, after this stack is
    // abandoned for good — switching_out_names), and BOUND the wait
    // (same 300-jiffy budget class as FDM/FTS): past the deadline the
    // stack is WITHHELD (leaked, printed) — never a spinning reaper
    // (this loop's unbounded shape was the leader-path face of the
    // r16type wedge class) and never a freed live stack (FTS's
    // leak-don't-dangle discipline applied to the stack).
    // R11 (terminal-release window): the leader's terminal deschedule
    // runs the same schedule_dead_never_returns loop (the sc191 pre-
    // schedule ownership release), so {on_cpu=false, no marker} is also
    // reachable HERE while the leader still executes its own schedule
    // chain — terminal_switch_done (set only at the real-switch retire
    // sites, re-armed by the loop each round) is the required positive
    // knowledge.
    crate::arch::cpu::restore_irq(true);
    const RTS_SWITCH_WAIT_JIFFIES: u64 = 300;
    let rts_armed_at = crate::timer::get_jiffies();
    while t.on_cpu()
        || crate::sched::hotpath::switching_out_names(task)
        || !t
            .terminal_switch_done
            .load(core::sync::atomic::Ordering::Acquire)
    {
        if crate::timer::get_jiffies()
            .wrapping_sub(rts_armed_at)
            >= RTS_SWITCH_WAIT_JIFFIES
        {
            // RTS-DEFER (the sys_wait4+892 NULL children-walk root cause,
            // 2026-11-21): the OLD deadline path withheld only the STACK
            // and then CONTINUED freeing the Task struct — but the owner
            // is still executing its terminal deschedule loop on that
            // page (re-arming terminal_switch_done=false, releasing pick
            // ownership, re-asserting DEAD). task_put → free_task_slot's
            // gate passes (no per-CPU current names it, no SWITCHING_OUT
            // marker — the real switch has not even STARTED), the page
            // goes back to the allocator mid-fork-storm, and the next
            // fork's place_task window (Task::new's NULL ListHeads, init
            // three instructions later) is exactly the NULL children.next
            // the child-scan walk dereferences. The dying task keeps
            // writing the freed page (the free-while-executing shape the
            // FDM lane convicted years ago).
            // FIX (source-first protocol, leak-don't-dangle — the same
            // discipline free_dead_member already applies to members):
            // the deadline path DEFERS THE WHOLE STORAGE FREE. The Task
            // page (and its stack) stays untouched; the release is
            // retried by the reaper's own dead-threads drain (release_task
            // drains on every reap) once the terminal switch truly
            // completes. One page held per wedged switch, never a page
            // freed under its running owner.
            static RTS_DEFER_SEEN: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = RTS_DEFER_SEEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 64 {
                crate::debug::taskdump::taskdump_raw_line(
                    b"RTS-DEFER (storage free deferred; owner still switching) pid=",
                );
                crate::debug::taskdump::taskdump_dec(t.pid() as u64);
                crate::debug::taskdump::taskdump_raw_line(b" on_cpu=");
                crate::debug::taskdump::taskdump_dec(t.on_cpu() as u64);
                crate::debug::taskdump::taskdump_raw_line(b" swout=");
                crate::debug::taskdump::taskdump_dec(
                    crate::sched::hotpath::switching_out_names(task) as u64,
                );
                crate::debug::taskdump::taskdump_raw_line(b" tsd=");
                crate::debug::taskdump::taskdump_dec(
                    t.terminal_switch_done.load(core::sync::atomic::Ordering::Acquire) as u64,
                );
                crate::debug::taskdump::taskdump_raw_line(b" st=");
                crate::debug::taskdump::taskdump_dec(t.state().bits() as u64);
                crate::debug::taskdump::taskdump_raw_line(b" slot=");
                let mut slot_pid = u32::MAX;
                for c in 0..crate::config::MAX_CPUS {
                    let cur = crate::sched::hotpath::cpu_state(c).current;
                    if cur == task {
                        slot_pid = c as u32;
                        break;
                    }
                }
                crate::debug::taskdump::taskdump_dec(slot_pid as u64);
                crate::debug::taskdump::taskdump_raw_line(b"\n");
            }
            // Hand the retry the reaper's ownership: re-park the task on
            // the reaping task's dead_threads list. The drain ladder
            // (release_task's drain, run on every reap; sweep_dead_
            // threads on the leader side) re-runs release_task_storage
            // when the terminal flag settles. The reaper's own reference
            // (the one this call was about to drop via the final
            // task_put) is transferred to the list — the count is
            // unchanged and the page outlives the owner's switch.
            if let Some(reaper) = crate::sched::hotpath::current() {
                let rp = reaper as *mut Task as *mut Task;
                if rp != task {
                    crate::r#unsafe::borrow_task_mut(rp)
                        .dead_threads
                        .lock()
                        .push(claim.into_parked());
                }
            }
            return;
        }
        core::hint::spin_loop();
    }

    // Free kernel stack
    t.free_kernel_stack();

    // Free heap-allocated PtRegs from fork.
    // copy_thread() allocates PtRegs on the heap for the child's first
    // context switch; clear_fork_child() keeps the pointer so we can free it here.
    crate::r#unsafe::free_fork_pt_regs(t);

    // Clear Arc references (this will decrement reference counts)
    t.set_address_space(None);
    t.set_fdtable(None);
    t.signal = None;
    t.set_fs(None);

    // Free Task struct back to kernel heap — via task_put (R12-5): any
    // outstanding lookup pins delay the actual free to their own put.
    crate::task::Task::task_put(task);
    // The burn: the obligation is consumed (this is the synchronous
    // path's exactly-once free; the claim dies with it).
    core::mem::forget(claim);
}

/// Process exit
///
/// Called when a process terminates (sys_exit, fatal signal, etc.).
/// This function never returns.
///
/// Thread-group semantics:
/// - NON-LEADER thread: robust list + clear_child_tid + per-thread resource
///   drops, ring detach, then deferred release (DEAD state, parked on the
///   leader's dead_threads list). No ZOMBIE — threads are never waited on.
/// - LEADER (or single-threaded): first force SIGKILL any remaining group
///   members and wait for the group to drain, then the classic ZOMBIE flow
///   (parent wait4 reaps via release_task).
pub fn reparent_children_to_init(dying: &Task) {
    use crate::task::TaskState;

    // init task (PID 1); falls back to no-op when absent (early boot).
    // SAFETY: pid 1's Task lives until system shutdown.
    let init = crate::sched::hotpath::find_task_by_pid(1);
    if init.is_null() {
        return;
    }

    // PR_SET_CHILD_SUBREAPER: prefer the nearest ancestor marked as a
    // subreaper over init (prctl stores the flag on the shared
    // SignalStruct). Bounded walk to stay safe against parent loops.
    let mut dest = init;
    // E8-REPAR: a non-init dest (group member or subreaper ancestor) can
    // die and be freed between its selection here and the moves below —
    // pin it (task_refcnt) so its Task outlives every add_child; the
    // final8 kernel panic walked a parent pointer that had been written
    // into exactly such a freed dest.
    let mut dest_pin: *mut Task = core::ptr::null_mut();
    let dying_p = dying as *const Task as *mut Task;
    // A dying THREAD's children go to a surviving member of its own
    // thread group first (Linux find_new_reaper): the group's remaining
    // threads keep group-wide wait visibility, so wait4/waitid callers
    // in this process can still reap them. Only when no sibling
    // survives do we fall through to the subreaper/init walk.
    {
        let _lock = crate::task::PROCESS_TREE_LOCK.lock();
        let mut member = dying.next_thread_ptr();
        let mut guard = 0u32;
        while (member as *const Task) != (dying_p as *const Task)
            && !member.is_null()
            && guard < 65536
        {
            // Skip members that are already gone from the scheduler's
            // view; anything still linked in the ring is pre-leave and
            // therefore valid memory. ALSO skip PF_EXITING members
            // (leg-29A root cause): do_exit sets PF_EXITING FIRST but
            // runs reparent_children_to_init mid-exit — a member past
            // its own reparent, still pre-ZOMBIE, accepted children
            // moved here that its already-run reparent would never move
            // again. The freed-then-reaped dest left every moved child
            // (and its thread-group members) naming freed memory — the
            // class the retired treehunt probe drew as TREE-PARENT-DEAD
            // (with test/subreap_churn.c), and the ancestor sc304's
            // subreaper walk faulted into.
            let m_ref = crate::r#unsafe::borrow_task(member);
            let st = m_ref.state();
            let member_exiting = m_ref.ti_flags()
                & crate::task::task_flags::TaskFlags::PF_EXITING.bits()
                != 0;
            if !member_exiting
                && !st.is_dead()
                && !st.contains(crate::task::TaskState::ZOMBIE)
            {
                // Pin INSIDE the lock hold: the member cannot leave the
                // ring (thread_group_leave takes this lock) and a pinned
                // member cannot be freed after leaving, so this fetch_add
                // can never land in freed memory.
                m_ref
                    .task_refcnt
                    .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                dest = member;
                dest_pin = member;
                break;
            }
            member = m_ref.next_thread_ptr();
            guard += 1;
        }
    }
    if dest as *const Task == init as *const Task {
        // E8-REPAR: a non-leader thread's `parent` field is an UNLINKED
        // snapshot copied from the leader at thread_group_join time — the
        // leader gets reparented later, the copy does not, so it can name
        // an ancestor that was reaped and freed long ago. Walk from the
        // LEADER's parent field instead (the maintained one; a zombie
        // leader's memory stays until reaped, and a leader cannot be
        // released while any member — us — is still alive).
        let leader = dying.group_leader_ptr();
        let chain_root = if leader.is_null()
            || leader as *const Task == dying as *const Task
        {
            dying_p
        } else {
            leader
        };
        let mut p = crate::r#unsafe::borrow_task(chain_root).parent_ptr();
        let mut hops = 0;
        // The whole walk under the tree lock: every release_task of a
        // chain member has to pass remove_child under this lock first,
        // so no hop can be freed mid-walk.
        let _lock = crate::task::PROCESS_TREE_LOCK.lock();
        while let Some(pp) = p {
            let pp = pp as *mut Task;
            // E8-REPAR: validate EVERY hop before dereferencing it
            // (kernel-range + TASK_POISON, the wake_up guard family).
            // The final8 panic died on `(*pp).signal` of a freed
            // ancestor at exit.rs:954; a stale hop now bails to init
            // instead of faulting.
            if !Task::is_plausible_task_ptr(pp) {
                break;
            }
            if pp == dying_p || pp == init || hops > 64 {
                break;
            }
            let is_reaper = crate::r#unsafe::borrow_task(pp)
                .signal
                .as_ref()
                .and_then(|s| {
                    // E8-REPAR follow-up (sc304 Oops): a recycled
                    // ancestor's Task passed is_plausible_task_ptr but
                    // its `signal` Arc slot held a USER-range garbage
                    // pointer — AtomicBool::load faulted at
                    // 0x7ff08b80f820 under this lock and the Oops-killed
                    // task wedged the boot holding PROCESS_TREE_LOCK.
                    // Validate the SignalStruct pointer's RANGE (same
                    // linear-map window as the task check; SignalStruct
                    // is heap-allocated inside it) before the atomic
                    // load; garbage => treat the hop as non-reaper and
                    // stop the walk here (dest stays init).
                    const MAP_BASE: usize = crate::arch::mm::memory_layout::PAGE_OFFSET;
                    const MAP_SPAN: usize = 0x10_0000_0000; // 64 GiB
                    let raw = alloc::sync::Arc::as_ptr(s) as usize;
                    if raw < MAP_BASE || raw >= MAP_BASE + MAP_SPAN {
                        return None;
                    }
                    Some(s.is_child_subreaper.load(core::sync::atomic::Ordering::Acquire))
                })
                .unwrap_or(false);
            let pp_exiting = crate::r#unsafe::borrow_task(pp)
                .ti_flags()
                & crate::task::task_flags::TaskFlags::PF_EXITING.bits()
                != 0;
            if is_reaper && !pp_exiting {
                dest = pp;
                // Pin the subreaper dest under the same lock hold (same
                // argument as the member pin above).
                crate::r#unsafe::borrow_task(pp)
                    .task_refcnt
                    .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                dest_pin = pp;
                break;
            }
            p = crate::r#unsafe::borrow_task(pp).parent_ptr();
            hops += 1;
        }
    }
    if dest as *const Task == dying as *const Task {
        // dest == dying happens only when dying IS init (the member walk
        // skips dying; the subreaper walk breaks on it) — release the pin
        // before bailing.
        if !dest_pin.is_null() {
            Task::task_put(dest_pin);
        }
        return; // dying IS init / the only candidate — nothing to do
    }

    // Collect children PINNED (same lock hold as the list walk): a
    // sibling thread of dying's group may be in wait4 reaping these very
    // children (for_each_group_child scans every member's list); without
    // the pin, release_task could free a collected child before the move
    // loop links it — writing through freed sibling pointers and
    // planting freed nodes into dest's children list.
    let mut moved: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();
    dying.collect_children_pinned(&mut moved);

    for child in moved {
        // E8-REPAR: atomic validate+unlink+relink. move_child skips a
        // child that a concurrent reaper already unlinked (claimed or
        // released) — after a false return the child may be freed memory
        // we must not touch beyond our own task_put.
        let ok = dying.move_child(child, dest);
        if !ok {
            Task::task_put(child);
            continue;
        }

        // PR_SET_PDEATHSIG: the old parent just died — deliver the signal
        // the child asked for (Linux: it fires on reparenting, before the
        // child could observe the orphaning).
        let pdeath = crate::r#unsafe::borrow_task(child).pdeath_signal;
        if pdeath != 0 {
            // OH bring-up forensics: the *_host services all die of a
            // kernel-origin SIGHUP; this print names pdeathsig as the
            // source (or rules it out) directly on the console.
            static PDEATH_TRACE_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if PDEATH_TRACE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 100 {
                crate::pr_info!(
                    "pdeath: pid {} tgid {} gets sig {} (parent pid {} tgid {} dying)",
                    crate::r#unsafe::borrow_task(child).pid(),
                    crate::r#unsafe::borrow_task(child).tgid(),
                    pdeath,
                    dying.pid(),
                    dying.tgid()
                );
            }
            let _ = crate::signal::send_signal(
                crate::r#unsafe::borrow_task(child).pid(),
                pdeath as i32,
            );
        }

        // If the orphan is already a zombie, the new parent must be
        // notified so it reaps it — otherwise it would sit unreapable
        // forever.
        let child_ref = crate::r#unsafe::borrow_task(child);
        if child_ref.state() == TaskState::new(TaskState::ZOMBIE) {
            let dest_pid = crate::r#unsafe::borrow_task(dest).pid();
            // Full do_notify_parent siginfo: the new parent's SA_SIGINFO
            // handlers decode the orphan zombie's outcome from
            // si_code/si_status.
            let mut info = crate::signal::SigInfo::child_exit(
                child_ref.pid(),
                child_ref.cred().uid,
                child_ref.exit_code(),
                child_ref.core_dumped(),
            );
            // do_notify_parent delivers the child's clone exit_signal.
            let orphan_sig = child_ref.get_exit_signal();
            if orphan_sig != 0 {
                if orphan_sig != crate::signal::Signal::SIGCHLD as u8 {
                    info.si_signo = orphan_sig as i32;
                }
                let _ = crate::signal::send_signal_with_info(dest_pid, info);
            }
            // init's default SIGCHLD disposition is SIG_IGN, so the signal
            // path neither pends nor wakes anything. Wake the new parent's
            // child-exit wait queue directly (same queue the deferred
            // exit-notify path uses) or a blocked wait4 there never
            // re-checks its children and the orphaned zombie leaks.
            // Group-wide: any member of dest's group may hold the
            // blocking wait (Linux wait_chldexit is group-shared).
            if let Some(dest_handle) = crate::task_ref::TaskRef::lookup(dest_pid) {
                // The handle owns the pin for the wake.
                crate::exit::wake_group_chldexit(&dest_handle);
            }
        }
        // Drop the collector's pin now that the move completed.
        Task::task_put(child);
    }
    // Drop the dest pin (init was never pinned — it outlives shutdown).
    if !dest_pin.is_null() {
        Task::task_put(dest_pin);
    }
}
